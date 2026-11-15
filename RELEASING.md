# Releasing rchat

Two update channels, two keys, one build:

| Channel | Who replaces the binary | Proof |
| --- | --- | --- |
| AppImage | `tauri-plugin-updater` inside the app | minisign (updater key) |
| `.deb` | apt + the `rchat-update.timer` shipped in the package | GPG (archive key) |

Never mix the keys up. Losing the updater key means existing AppImage installs
can never accept another update; replacing it requires a manual reinstall.

## One-time setup

```bash
# Updater signing key (minisign). Keep the private half offline or in CI secrets.
cd apps/desktop
cargo tauri signer generate -w ~/.config/rchat/release/updater.key

# Apt archive key (GPG). Its public half is committed as a keyring.
export GNUPGHOME=~/.config/rchat/release/gnupg
mkdir -p "$GNUPGHOME" && chmod 700 "$GNUPGHOME"
gpg --batch --passphrase '' --quick-generate-key \
  'rchat archive <archive@rchat.network>' rsa4096 sign 5y
gpg --export > apps/desktop/src-tauri/packaging/deb/rchat-archive-keyring.gpg
gpg --list-keys --with-colons | awk -F: '/^fpr:/{print $10; exit}'
```

Put the fingerprint in `RCHAT_APT_KEY`. Both keys were generated without a
passphrase during the first implementation — protect them before real use:

```bash
gpg --passwd <fingerprint>
cargo tauri signer generate -w ~/.config/rchat/release/updater.key   # re-key once
```

Re-keying the updater means bumping to a version that users install manually;
that is why it must happen before the first public update, not after.

## Cut a release

1. Bump the version in **three** places and commit:
   - `apps/desktop/src-tauri/tauri.conf.json`
   - `apps/desktop/src-tauri/Cargo.toml`
   - `apps/desktop/package.json`
2. Build, sign, write the manifest and the apt repo:

   ```bash
   export RCHAT_APT_KEY=<fingerprint>
   node scripts/release.mjs --notes notes.md
   ```

   That produces `dist/latest.json`, `dist/SHA256SUMS`, and a signed apt tree in
   `dist/repo` (pool + Packages + InRelease).

3. Publish (GitHub release + `gh-pages` apt repo):

   ```bash
   node scripts/release.mjs --notes notes.md --upload --pages
   ```

   `--upload` creates `v<version>` from the current commit if the tag is missing,
   then uploads the `.deb`, `.AppImage`, `.AppImage.sig`, `latest.json` and
   checksums. `--pages` pushes `dist/repo` to the orphan `gh-pages` branch.
   The token comes from `GITHUB_TOKEN` or `~/.rchat-release-token`.

4. Verify:

   ```bash
   cargo test --test update_artifacts      # in apps/desktop/src-tauri
   curl -sL https://github.com/bestusecase/rchat/releases/latest/download/latest.json
   ```

   Then run the previous AppImage and update it from the Settings panel.

## How each channel updates

**AppImage.** The shell checks `releases/latest/download/latest.json` shortly
after launch and every six hours. The manifest carries the version, notes and a
base64 minisign signature for the artifact; `requireSignedVersion` in
`tauri.conf.json` ties the announced version to the one inside the signature, so
a stale, validly signed older build cannot be replayed as a newer one. Downloads
are user-confirmed, then applied on restart.

**Deb.** The package installs
`/etc/apt/sources.list.d/rchat.list`, a trusted keyring, and
`rchat-update.{service,timer}`. The timer runs once a day (randomised within six
hours) and executes `/usr/lib/rchat/rchat-update`, which refreshes **only** that
source, skips if the candidate equals the installed version, and otherwise runs
`apt-get install --only-upgrade rchat`. A failure (offline, apt busy elsewhere)
exits 0 so the unit does not collect red states. Logs:

```bash
journalctl -u rchat-update --since '2 days ago'
systemctl list-timers rchat-update.timer
```

After an upgrade the running process still holds the old binary; the app detects
that `dpkg-query` reports a newer version, and Settings offers "Restart to
update".

## Constraints worth remembering

- The apt repository lives on `gh-pages`; GitHub Pages has a ~1 GB site limit and
  each deb is ~80 MB, so `apt-repo.mjs` keeps only the last three versions.
- Published version numbers are immutable: `requireSignedVersion` and apt both
  assume a given version maps to exactly one artifact.
- Metro/offline machines: the timer fails quietly and retries the next day.
- Don't rebuild a released tag. Cut a new version instead.
