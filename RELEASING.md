# Releasing rchat

Two update channels, two keys, one build:

| Channel | Who replaces the binary | Proof |
| --- | --- | --- |
| AppImage | `tauri-plugin-updater` inside the app | minisign (updater key) |
| `.deb` | apt + the `rchat-update.timer` shipped in the package | GPG (archive key) |

Never mix the keys up. Losing the updater key means existing AppImage installs
can never accept another update; replacing it requires a manual reinstall.

## Keys

Both private keys live in `~/.config/rchat/release/` (mode 600) and are
passphrase-protected. The passphrases are in `apt.pass` and `updater.pass` in
the same directory (mode 600); `scripts/release.mjs` reads them automatically.
For CI, pass them as `GPG_PASSPHRASE` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
instead of shipping the files. Back up both key files **and** both passphrases
somewhere offline — losing either half means no further signed releases.

Keep the release directory out of any backup that leaves the machine unless the
backup is encrypted, since key and passphrase sit side by side.

To recreate the keys from scratch (new fingerprint/new pubkey — see the warning
below):

```bash
# Updater signing key (minisign).
cd apps/desktop
cargo tauri signer generate -w ~/.config/rchat/release/updater.key
# The public half goes verbatim into plugins.updater.pubkey in tauri.conf.json.

# Apt archive key (GPG). Its public half is committed as a keyring.
export GNUPGHOME=~/.config/rchat/release/gnupg
mkdir -p "$GNUPGHOME" && chmod 700 "$GNUPGHOME"
gpg --quick-generate-key 'rchat archive <archive@rchat.network>' rsa4096 sign 5y
gpg --export > apps/desktop/src-tauri/packaging/deb/rchat-archive-keyring.gpg
gpg --list-keys --with-colons | awk -F: '/^fpr:/{print $10; exit}'
```

Put the fingerprint in `RCHAT_APT_KEY`.

**Re-keying the updater is a breaking change.** Every existing AppImage carries
the old public key and will reject the new signature, so each one must be
replaced manually once. The current key was rotated at 0.1.3; AppImage 0.1.2
installs cannot update themselves and must download 0.1.3 by hand. Debian
installs are unaffected because the archive keypair itself did not change, only
its passphrase.

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

   The script reads both passphrases from the `*.pass` files described above, so
   the signing steps run unattended. That produces `dist/latest.json`,
   `dist/SHA256SUMS`, and a signed apt tree in `dist/repo` (pool + Packages +
   InRelease).

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
