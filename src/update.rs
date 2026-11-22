//! Self-update from GitHub Releases.
//!
//! Every release carries three files:
//!
//! * `seventy3.exe`: the program;
//! * `release.json`: a manifest naming the version and every file with its
//!   size and SHA-256;
//! * `release.json.sig`: an Ed25519 signature of `release.json`'s exact bytes.
//!
//! The public keys Seventy3 trusts are compiled in ([`TRUSTED_KEYS`]); the
//! private key is a GitHub Actions secret, so only the release workflow can
//! sign. An update is installed only if the signature verifies, the signed
//! version matches the release tag and is newer than the running build, and
//! the downloaded program matches the signed size and hash. Nothing unsigned
//! is ever run.
//!
//! Installing swaps files by rename next to the running program
//! (`seventy3.exe` → `seventy3.old.exe`, download → `seventy3.exe`), which
//! Windows allows for a running executable, then starts the new version. The
//! old file is deleted when the new version starts.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::rig::Notify;

pub const RELEASES_PAGE: &str = "https://github.com/KK4ODA/Seventy3/releases";
pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/KK4ODA/Seventy3/releases/latest";
pub const MANIFEST_NAME: &str = "release.json";
pub const SIGNATURE_NAME: &str = "release.json.sig";
pub const MANIFEST_FORMAT: &str = "seventy3-release";
pub const MANIFEST_FORMAT_VERSION: u32 = 1;

/// Public keys (base64) whose release signatures are trusted. More than one
/// allows rotation: ship a release that trusts both keys, then switch the
/// workflow to the new one.
pub const TRUSTED_KEYS: &[&str] = &[
    // Seventy3 release key 1 (created 2026-11-22).
    "BxsyrzvbzNjGVOauOOOh2cQAkrtqDPcgwTSd4kkaoZg=",
];

/// The program file in a release.
pub fn program_name() -> &'static str {
    if cfg!(windows) { "seventy3.exe" } else { "seventy3" }
}

/// The signed description of a release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    pub version: String,
    /// Release date, YYYY-MM-DD.
    pub released: String,
    /// Release notes (Markdown).
    pub notes: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub name: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

impl Manifest {
    pub fn file(&self, name: &str) -> Option<&ManifestFile> {
        self.files.iter().find(|f| f.name == name)
    }
}

// ---- versions -------------------------------------------------------------

/// A `major.minor.patch[-pre]` version. A pre-release sorts before the
/// release with the same numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub numbers: [u64; 3],
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let t = text.trim().trim_start_matches(['v', 'V']);
        let (core, pre) = match t.split_once('-') {
            Some((c, p)) => (c, Some(p.to_owned())),
            None => (t, None),
        };
        let core = core.split('+').next()?;
        let mut parts = core.split('.');
        let mut numbers = [0u64; 3];
        for n in &mut numbers {
            *n = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Version { numbers, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.numbers.cmp(&other.numbers).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
        })
    }
}

/// True if `candidate` is a newer version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

// ---- hashing and signatures -------------------------------------------------

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Size and SHA-256 of a file, read in chunks.
pub fn hash_file(path: &Path) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((size, hex(&hasher.finalize())))
}

pub fn parse_keys(keys: &[&str]) -> Vec<VerifyingKey> {
    keys.iter()
        .filter_map(|k| BASE64.decode(k.trim()).ok())
        .filter_map(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
        .filter_map(|b| VerifyingKey::from_bytes(&b).ok())
        .collect()
}

pub fn trusted_keys() -> Vec<VerifyingKey> {
    parse_keys(TRUSTED_KEYS)
}

/// Reads a private key: the base64 of its 32-byte seed.
pub fn parse_signing_key(text: &str) -> Result<SigningKey, String> {
    let bytes = BASE64.decode(text.trim()).map_err(|e| format!("signing key is not base64: {e}"))?;
    let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| "signing key must be 32 bytes".to_owned())?;
    Ok(SigningKey::from_bytes(&seed))
}

pub fn generate_signing_key() -> Result<SigningKey, String> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("no system randomness: {e}"))?;
    Ok(SigningKey::from_bytes(&seed))
}

pub fn encode_key(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

/// The signature file's contents for `manifest_bytes`.
pub fn sign(manifest_bytes: &[u8], key: &SigningKey) -> String {
    BASE64.encode(key.sign(manifest_bytes).to_bytes()) + "\n"
}

/// Checks the signature and returns the manifest it covers.
pub fn verify(manifest_bytes: &[u8], signature_text: &str, keys: &[VerifyingKey]) -> Result<Manifest, String> {
    if keys.is_empty() {
        return Err("this build has no update signing key, so it can't verify updates".into());
    }
    let sig_bytes =
        BASE64.decode(signature_text.trim()).map_err(|_| "the release signature is malformed".to_owned())?;
    let signature = Signature::from_slice(&sig_bytes).map_err(|_| "the release signature is malformed".to_owned())?;
    if !keys.iter().any(|k| k.verify_strict(manifest_bytes, &signature).is_ok()) {
        return Err("the release signature doesn't match Seventy3's signing key; the update was refused".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).map_err(|e| format!("the release manifest is unreadable: {e}"))?;
    if manifest.format != MANIFEST_FORMAT || manifest.format_version != MANIFEST_FORMAT_VERSION {
        return Err("the release manifest is in a format this version doesn't know; update by hand".into());
    }
    Ok(manifest)
}

// ---- GitHub -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// A verified, newer release.
#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub manifest: Manifest,
    pub page_url: String,
    program_url: String,
    program: ManifestFile,
}

impl Available {
    pub fn version(&self) -> &str {
        &self.manifest.version
    }
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .user_agent(concat!("Seventy3/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get_bytes(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent
        .get(url)
        .header("Accept", "application/vnd.github+json, application/octet-stream, */*")
        .call()
        .map_err(|e| format!("couldn't reach GitHub: {e}"))?;
    resp.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| format!("download failed: {e}"))
}

/// Looks for a release newer than `current`. `Ok(None)` means up to date.
pub fn check(
    agent: &ureq::Agent,
    api_url: &str,
    current: &str,
    keys: &[VerifyingKey],
) -> Result<Option<Available>, String> {
    let body = get_bytes(agent, api_url, 1 << 20)?;
    let release: GhRelease =
        serde_json::from_slice(&body).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    if release.draft || release.prerelease || !is_newer(&release.tag_name, current) {
        return Ok(None);
    }
    let asset = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| format!("release {} has no {name}; update by hand", release.tag_name))
    };
    let manifest_bytes = get_bytes(agent, &asset(MANIFEST_NAME)?.browser_download_url, 1 << 20)?;
    let signature = get_bytes(agent, &asset(SIGNATURE_NAME)?.browser_download_url, 4096)?;
    let manifest = verify(&manifest_bytes, &String::from_utf8_lossy(&signature), keys)?;

    // The signed version must be the tagged one, so an old signed manifest
    // can't be replayed under a newer tag.
    if Version::parse(&manifest.version) != Version::parse(&release.tag_name) {
        return Err(format!("release {} carries a manifest for {}", release.tag_name, manifest.version));
    }
    let program = manifest
        .file(program_name())
        .cloned()
        .ok_or_else(|| format!("release {} has no {} for this system", manifest.version, program_name()))?;
    let program_asset = asset(program_name())?;
    if program_asset.size != program.size {
        return Err("the program on GitHub doesn't match the signed manifest".into());
    }
    Ok(Some(Available {
        program_url: program_asset.browser_download_url.clone(),
        page_url: release.html_url,
        program,
        manifest,
    }))
}

/// Downloads the program to `dest`, checking it against the signed size and
/// hash. Nothing is left at `dest` unless it verified.
pub fn download(
    agent: &ureq::Agent,
    update: &Available,
    dest: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let part = dest.with_extension("part");
    let result = (|| -> Result<(), String> {
        let resp = agent.get(&update.program_url).call().map_err(|e| format!("download failed: {e}"))?;
        let mut reader = resp.into_body().into_reader();
        let mut out = File::create(&part).map_err(|e| {
            format!("can't write next to Seventy3 ({e}); download the new version from the releases page instead")
        })?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let total = update.program.size;
        let mut done = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > total {
                return Err("the download is larger than the signed size".into());
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| format!("can't save the download: {e}"))?;
            progress(done, total);
        }
        out.sync_all().map_err(|e| format!("can't save the download: {e}"))?;
        if done != total || hex(&hasher.finalize()) != update.program.sha256 {
            return Err("the download doesn't match the signed release; it was discarded".into());
        }
        Ok(())
    })();
    match result {
        Ok(()) => std::fs::rename(&part, dest).map_err(|e| format!("can't save the download: {e}")),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// `seventy3.exe` → `seventy3.<tag>.exe` in the same folder.
fn sibling(exe: &Path, tag: &str) -> PathBuf {
    let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("seventy3");
    let name = match exe.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.{tag}.{ext}"),
        None => format!("{stem}.{tag}"),
    };
    exe.with_file_name(name)
}

pub fn staged_path(exe: &Path) -> PathBuf {
    sibling(exe, "new")
}

pub fn old_path(exe: &Path) -> PathBuf {
    sibling(exe, "old")
}

/// Puts the verified download in place of the running program, keeping the
/// previous one as `*.old` until the next start. Rolls back on failure.
pub fn install(staged: &Path, exe: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))?;
    }
    let old = old_path(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(staged, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

/// Deletes the previous version left by an update. The old process may
/// still be exiting, so this retries for a while in the background.
pub fn clean_up_after_update(exe: PathBuf) {
    let old = old_path(&exe);
    let staged = staged_path(&exe);
    if !old.exists() && !staged.exists() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..40 {
            let _ = std::fs::remove_file(&staged);
            match std::fs::remove_file(&old) {
                Ok(()) => {
                    log::info!("Removed {}", old.display());
                    return;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return,
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    });
}

// ---- background updater -----------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Available),
    Downloading {
        version: String,
        done: u64,
        total: u64,
    },
    /// The new version is in place; restart to run it.
    Installed {
        version: String,
    },
    Failed(String),
}

enum Command {
    Check,
    Install,
}

pub struct UpdaterConfig {
    pub api_url: String,
    pub current_version: String,
    pub keys: Vec<VerifyingKey>,
    /// The running program, captured at startup.
    pub exe: Option<PathBuf>,
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        UpdaterConfig {
            // For testing against a stand-in server. Signatures are still
            // checked against the built-in keys, so this can't be used to
            // install anything unsigned.
            api_url: std::env::var("SEVENTY3_UPDATE_URL").unwrap_or_else(|_| LATEST_RELEASE_API.into()),
            current_version: env!("CARGO_PKG_VERSION").into(),
            keys: trusted_keys(),
            exe: std::env::current_exe().ok(),
        }
    }
}

pub struct Updater {
    tx: Sender<Command>,
    state: Arc<Mutex<UpdateState>>,
    last_check: Arc<Mutex<Option<Instant>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Updater {
    pub fn spawn(config: UpdaterConfig, notify: Notify) -> Updater {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(UpdateState::Idle));
        let last_check = Arc::new(Mutex::new(None));
        let (s, l) = (state.clone(), last_check.clone());
        std::thread::Builder::new()
            .name("update".into())
            .spawn(move || run(rx, s, l, config, notify))
            .expect("spawn update thread");
        Updater { tx, state, last_check }
    }

    pub fn state(&self) -> UpdateState {
        lock(&self.state).clone()
    }

    pub fn last_check(&self) -> Option<Instant> {
        *lock(&self.last_check)
    }

    pub fn check_now(&self) {
        let _ = self.tx.send(Command::Check);
    }

    /// Downloads and installs the available update.
    pub fn install(&self) {
        let _ = self.tx.send(Command::Install);
    }
}

fn run(
    rx: Receiver<Command>,
    state: Arc<Mutex<UpdateState>>,
    last_check: Arc<Mutex<Option<Instant>>>,
    config: UpdaterConfig,
    notify: Notify,
) {
    let agent = agent();
    let set = |s: UpdateState| {
        *lock(&state) = s;
        notify();
    };
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Check => {
                if matches!(lock(&state).clone(), UpdateState::Downloading { .. } | UpdateState::Installed { .. }) {
                    continue;
                }
                set(UpdateState::Checking);
                let result = check(&agent, &config.api_url, &config.current_version, &config.keys);
                *lock(&last_check) = Some(Instant::now());
                match result {
                    Ok(Some(a)) => {
                        log::info!("Update available: {}", a.version());
                        set(UpdateState::Available(a));
                    }
                    Ok(None) => set(UpdateState::UpToDate),
                    Err(e) => {
                        log::warn!("Update check: {e}");
                        set(UpdateState::Failed(e));
                    }
                }
            }
            Command::Install => {
                let UpdateState::Available(update) = lock(&state).clone() else { continue };
                let Some(exe) = config.exe.clone() else {
                    set(UpdateState::Failed("can't tell where Seventy3 is installed".into()));
                    continue;
                };
                let version = update.version().to_owned();
                let staged = staged_path(&exe);
                let mut last_notify = Instant::now();
                let result = download(&agent, &update, &staged, |done, total| {
                    *lock(&state) = UpdateState::Downloading { version: version.clone(), done, total };
                    if last_notify.elapsed() > Duration::from_millis(100) {
                        notify();
                        last_notify = Instant::now();
                    }
                })
                .and_then(|()| {
                    install(&staged, &exe).map_err(|e| {
                        let _ = std::fs::remove_file(&staged);
                        format!("couldn't replace {}: {e}", exe.display())
                    })
                });
                match result {
                    Ok(()) => {
                        log::info!("Installed Seventy3 {version}");
                        set(UpdateState::Installed { version });
                    }
                    Err(e) => {
                        log::warn!("Update failed: {e}");
                        set(UpdateState::Failed(e));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn manifest_for(version: &str, program: &[u8]) -> Vec<u8> {
        let m = Manifest {
            format: MANIFEST_FORMAT.into(),
            format_version: MANIFEST_FORMAT_VERSION,
            version: version.into(),
            released: "2026-11-22".into(),
            notes: "Notes".into(),
            files: vec![ManifestFile {
                name: program_name().into(),
                size: program.len() as u64,
                sha256: sha256_hex(program),
            }],
        };
        serde_json::to_vec_pretty(&m).unwrap()
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.2", "0.1.0"));
    }

    #[test]
    fn signatures_verify_and_tampering_is_caught() {
        let k = key(1);
        let keys = vec![k.verifying_key()];
        let m = manifest_for("0.2.0", b"program");
        let sig = sign(&m, &k);
        assert_eq!(verify(&m, &sig, &keys).unwrap().version, "0.2.0");

        let mut tampered = m.clone();
        let i = tampered.iter().position(|&b| b == b'2').unwrap();
        tampered[i] = b'3';
        assert!(verify(&tampered, &sig, &keys).is_err());
        assert!(verify(&m, &sign(&m, &key(2)), &keys).is_err(), "other key");
        assert!(verify(&m, "not base64!", &keys).is_err());
        assert!(verify(&m, &sig, &[]).is_err(), "no trusted keys");
        // Rotation: any trusted key will do.
        assert!(verify(&m, &sig, &[key(9).verifying_key(), k.verifying_key()]).is_ok());
    }

    #[test]
    fn keys_round_trip_through_base64() {
        let k = generate_signing_key().unwrap();
        let private = encode_key(&k.to_bytes());
        let public = encode_key(&k.verifying_key().to_bytes());
        assert_eq!(parse_signing_key(&private).unwrap().to_bytes(), k.to_bytes());
        assert_eq!(parse_keys(&[public.as_str()]), vec![k.verifying_key()]);
        assert!(parse_keys(&["junk"]).is_empty());
        assert!(parse_signing_key("c2hvcnQ=").is_err());
    }

    #[test]
    fn built_in_keys_parse() {
        assert_eq!(trusted_keys().len(), TRUSTED_KEYS.len(), "every built-in key must be valid");
    }

    #[test]
    fn install_swaps_and_rolls_back() {
        let dir = std::env::temp_dir().join(format!("seventy3-install-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(program_name());
        std::fs::write(&exe, b"old").unwrap();
        let staged = staged_path(&exe);
        std::fs::write(&staged, b"new").unwrap();
        install(&staged, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"old");
        assert!(!staged.exists());

        // A missing download must leave the current program in place.
        std::fs::remove_file(old_path(&exe)).unwrap();
        assert!(install(&staged, &exe).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_siblings() {
        let exe = Path::new("C:/Radio/seventy3.exe");
        assert_eq!(old_path(exe), Path::new("C:/Radio/seventy3.old.exe"));
        assert_eq!(staged_path(exe), Path::new("C:/Radio/seventy3.new.exe"));
        assert_eq!(old_path(Path::new("/opt/seventy3")), Path::new("/opt/seventy3.old"));
    }

    fn local_agent() -> ureq::Agent {
        ureq::Agent::config_builder().proxy(None).build().into()
    }

    /// Stands in for GitHub: a one-thread HTTP server with a "latest
    /// release" pointing at a signed manifest and a program file. Returns
    /// the API URL.
    fn fake_github(version: &str, program: &[u8], signer: &SigningKey, served_program: &[u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let manifest = manifest_for(version, program);
        let release = serde_json::json!({
            "tag_name": format!("v{version}"),
            "html_url": format!("{base}/page"),
            "draft": false,
            "prerelease": false,
            "assets": [
                {"name": MANIFEST_NAME, "browser_download_url": format!("{base}/release.json"), "size": manifest.len()},
                {"name": SIGNATURE_NAME, "browser_download_url": format!("{base}/release.json.sig"), "size": 90},
                {"name": program_name(), "browser_download_url": format!("{base}/program"), "size": served_program.len()},
            ]
        });
        let files: Vec<(String, Vec<u8>)> = vec![
            ("/latest".into(), serde_json::to_vec(&release).unwrap()),
            ("/release.json.sig".into(), sign(&manifest, signer).into_bytes()),
            ("/release.json".into(), manifest),
            ("/program".into(), served_program.to_vec()),
        ];
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                // Skip the headers.
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                    header.clear();
                }
                let mut stream = &stream;
                match files.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(body);
                    }
                    None => {
                        let _ = stream
                            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            }
        });
        format!("{base}/latest")
    }

    #[test]
    fn finds_downloads_and_verifies_a_release() {
        let signer = key(7);
        let keys = vec![signer.verifying_key()];
        let program = b"the new seventy3".to_vec();
        let api = fake_github("0.2.0", &program, &signer, &program);
        let agent = local_agent();

        assert_eq!(check(&agent, &api, "0.2.0", &keys).unwrap(), None, "same version");
        let update = check(&agent, &api, "0.1.0", &keys).unwrap().expect("newer release");
        assert_eq!(update.version(), "0.2.0");

        let dir = std::env::temp_dir().join(format!("seventy3-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("seventy3.new.exe");
        let mut seen = 0;
        download(&agent, &update, &dest, |done, _| seen = done).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), program);
        assert_eq!(seen, program.len() as u64);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_unsigned_or_tampered_releases() {
        let agent = local_agent();
        let program = b"the new seventy3".to_vec();

        // Signed by a key we don't trust.
        let api = fake_github("0.2.0", &program, &key(8), &program);
        let err = check(&agent, &api, "0.1.0", &[key(7).verifying_key()]).unwrap_err();
        assert!(err.contains("signature"), "{err}");

        // Correctly signed manifest, but the program on the server was swapped.
        let mut evil = program.clone();
        evil[0] ^= 1;
        let api = fake_github("0.2.0", &program, &key(7), &evil);
        let update = check(&agent, &api, "0.1.0", &[key(7).verifying_key()]).unwrap().unwrap();
        let dir = std::env::temp_dir().join(format!("seventy3-evil-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("seventy3.new.exe");
        assert!(download(&agent, &update, &dest, |_, _| {}).is_err());
        assert!(!dest.exists() && !dest.with_extension("part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
