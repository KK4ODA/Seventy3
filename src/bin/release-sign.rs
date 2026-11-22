//! Maintainer tool: creates and signs Seventy3 release manifests.
//!
//! ```text
//! release-sign keygen --out seventy3-release.key
//!     Makes a new signing key. Prints the public key for update::TRUSTED_KEYS.
//! release-sign manifest --version 0.1.0 --released 2026-11-22 --notes-file notes.md --out release.json FILE...
//!     Describes the release: every FILE with its size and SHA-256.
//! release-sign sign --in release.json --out release.json.sig [--key-file KEY]
//!     Signs the manifest. Without --key-file the key is read from the
//!     SEVENTY3_SIGNING_KEY environment variable (how CI does it).
//! release-sign verify --in release.json --sig release.json.sig
//!     Checks the signature against the keys built into this Seventy3, so a
//!     release signed with the wrong key fails before it is published.
//! ```
//!
//! Private keys are the base64 of a 32-byte Ed25519 seed. Keep a copy
//! offline: a lost key means users must install the next version by hand.

use std::collections::HashMap;
use std::path::Path;

use seventy3::update::{self, MANIFEST_FORMAT, MANIFEST_FORMAT_VERSION, Manifest, ManifestFile};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("keygen") => keygen(&args[1..]),
        Some("manifest") => manifest(&args[1..]),
        Some("sign") => sign(&args[1..]),
        Some("verify") => verify(&args[1..]),
        _ => Err("usage: release-sign keygen|manifest|sign|verify (see the source for options)".into()),
    };
    if let Err(e) = result {
        eprintln!("release-sign: {e}");
        std::process::exit(1);
    }
}

/// Splits `--name value` pairs from positional arguments.
fn options(args: &[String]) -> Result<(HashMap<String, String>, Vec<String>), String> {
    let mut opts = HashMap::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
            opts.insert(name.to_owned(), value.clone());
        } else {
            rest.push(a.clone());
        }
    }
    Ok((opts, rest))
}

fn need<'a>(opts: &'a HashMap<String, String>, name: &str) -> Result<&'a str, String> {
    opts.get(name).map(String::as_str).ok_or_else(|| format!("missing --{name}"))
}

fn keygen(args: &[String]) -> Result<(), String> {
    let (opts, _) = options(args)?;
    let out = need(&opts, "out")?;
    if Path::new(out).exists() {
        return Err(format!("{out} already exists; not overwriting a key"));
    }
    let key = update::generate_signing_key()?;
    std::fs::write(out, update::encode_key(&key.to_bytes()) + "\n").map_err(|e| format!("{out}: {e}"))?;
    println!("Private key written to {out}. Keep it secret and keep a copy offline.");
    println!("Public key (add to update::TRUSTED_KEYS):");
    println!("{}", update::encode_key(&key.verifying_key().to_bytes()));
    Ok(())
}

fn manifest(args: &[String]) -> Result<(), String> {
    let (opts, files) = options(args)?;
    let version = need(&opts, "version")?.trim_start_matches('v').to_owned();
    if update::Version::parse(&version).is_none() {
        return Err(format!("{version} is not a version like 1.2.3"));
    }
    let notes = match opts.get("notes-file") {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => String::new(),
    };
    if files.is_empty() {
        return Err("no files to list".into());
    }
    let mut listed = Vec::new();
    for path in &files {
        let (size, sha256) = update::hash_file(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
        let name = Path::new(path).file_name().and_then(|n| n.to_str()).ok_or("bad file name")?.to_owned();
        listed.push(ManifestFile { name, size, sha256 });
    }
    let m = Manifest {
        format: MANIFEST_FORMAT.into(),
        format_version: MANIFEST_FORMAT_VERSION,
        version,
        released: opts.get("released").cloned().unwrap_or_default(),
        notes: notes.trim().to_owned(),
        files: listed,
    };
    let out = need(&opts, "out")?;
    let json = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())? + "\n";
    std::fs::write(out, json).map_err(|e| format!("{out}: {e}"))?;
    println!("Wrote {out}");
    Ok(())
}

fn sign(args: &[String]) -> Result<(), String> {
    let (opts, _) = options(args)?;
    let key_text = match opts.get("key-file") {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => std::env::var("SEVENTY3_SIGNING_KEY")
            .map_err(|_| "no --key-file and SEVENTY3_SIGNING_KEY is not set".to_owned())?,
    };
    let key = update::parse_signing_key(&key_text)?;
    let input = need(&opts, "in")?;
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let out = need(&opts, "out")?;
    std::fs::write(out, update::sign(&bytes, &key)).map_err(|e| format!("{out}: {e}"))?;
    println!("Signed {input} with key {}", update::encode_key(&key.verifying_key().to_bytes()));
    Ok(())
}

fn verify(args: &[String]) -> Result<(), String> {
    let (opts, _) = options(args)?;
    let input = need(&opts, "in")?;
    let sig_path = need(&opts, "sig")?;
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let sig = std::fs::read_to_string(sig_path).map_err(|e| format!("{sig_path}: {e}"))?;
    let m = update::verify(&bytes, &sig, &update::trusted_keys())?;
    // The files listed must be the ones next to the manifest.
    let dir = Path::new(input).parent().unwrap_or(Path::new("."));
    for f in &m.files {
        let (size, sha256) = update::hash_file(&dir.join(&f.name)).map_err(|e| format!("{}: {e}", f.name))?;
        if size != f.size || sha256 != f.sha256 {
            return Err(format!("{} doesn't match the manifest", f.name));
        }
    }
    println!("{input}: valid signature for Seventy3 {}, {} file(s) match", m.version, m.files.len());
    Ok(())
}
