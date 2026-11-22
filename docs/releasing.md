# Releasing Seventy3

## Cutting a release

1. Set the new version in `Cargo.toml` (`version = "0.2.0"`) and run
   `cargo build` so `Cargo.lock` follows.
2. Add a `## [0.2.0] - YYYY-MM-DD` section to `CHANGELOG.md`. It becomes the
   GitHub release notes and the "What's new" text in the app.
3. Merge to `main`.
4. Start the **Release** workflow: either push a tag

   ```bash
   git tag v0.2.0 && git push origin v0.2.0
   ```

   or run it from the Actions tab with version `0.2.0` (it creates the tag on
   the branch you run it from).

The workflow builds `seventy3.exe` on Windows, writes `release.json` (version,
notes, size and SHA-256 of the exe), signs it, checks the signature against
the keys built into that exe, and publishes the release with
`seventy3.exe`, `release.json` and `release.json.sig`.

Installed copies check `releases/latest` at startup and once a day, and
offer the update with one click.

## The signing key

Updates are accepted only if `release.json` is signed by a key listed in
`TRUSTED_KEYS` in `src/update.rs`. The private key is the
`SEVENTY3_SIGNING_KEY` repository secret (Settings → Secrets and variables →
Actions): the base64 of a 32-byte Ed25519 seed, one line.

- **Keep an offline copy** of the private key (a password manager is fine).
  GitHub never shows a secret again after it's saved.
- **Missing secret:** the workflow still builds, but leaves the release as a
  draft with no signature. Add the secret and run the workflow again with
  the same version; it replaces the files, signs, and publishes.
- **Signing by hand** (for example, to keep the key off GitHub entirely):
  download the draft's `release.json` and `seventy3.exe` into one folder,
  then

  ```bash
  cargo run --bin release-sign -- sign --key-file seventy3-release.key --in release.json --out release.json.sig
  cargo run --bin release-sign -- verify --in release.json --sig release.json.sig
  ```

  upload `release.json.sig` to the draft, and publish it.

### Rotating the key

1. `cargo run --bin release-sign -- keygen --out new.key` prints the new
   public key.
2. Add it to `TRUSTED_KEYS` next to the old one, and release that version
   signed with the **old** key, so existing installs accept it.
3. Replace the secret with the new key for the releases after that. Remove
   the old public key once everyone has moved on.

If the old private key is lost, copies that only trust it can't verify new
releases. Their users have to download the new version by hand once.
