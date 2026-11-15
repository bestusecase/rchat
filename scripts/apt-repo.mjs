#!/usr/bin/env node
// Build (or refresh) the signed apt repository that the .deb installs.
//
// Layout produced under --out:
//   pool/main/r/rchat/rchat_<version>_amd64.deb
//   dists/stable/Release, InRelease, Release.gpg
//   dists/stable/main/binary-amd64/Packages(.gz)
//   rchat-archive-keyring.gpg
//
// The release file is signed with the archive key (minisign is for the AppImage
// updater instead; the two keys are deliberately separate).

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tauriDir = path.join(repoRoot, "apps/desktop/src-tauri");

function arg(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  return index >= 0 && process.argv[index + 1] ? process.argv[index + 1] : fallback;
}

function fail(message) {
  console.error(`apt-repo: ${message}`);
  process.exit(1);
}

function run(command, args, options = {}) {
  return execFileSync(command, args, { encoding: "utf8", ...options });
}

const debPath = path.resolve(arg("deb", ""));
if (!debPath || !fs.existsSync(debPath)) fail("--deb <path to .deb> is required");
const version = arg("version", null);
if (!version) fail("--version is required");
const out = path.resolve(arg("out", path.join(repoRoot, "dist/repo")));
const keep = Number(arg("keep", "3"));

const gnupg = process.env.GNUPGHOME || path.join(os.homedir(), ".config/rchat/release/gnupg");
const keyId = process.env.RCHAT_APT_KEY || arg("key", null);
if (!keyId) fail("set RCHAT_APT_KEY (or --key) to the archive key fingerprint");

// The archive key is passphrase-protected. Prefer GPG_PASSPHRASE (set by
// release.mjs from ~/.config/rchat/release/apt.pass) and feed it over stdin so
// it never appears in argv. Without one, gpg falls back to its pinentry.
const passphrase =
  process.env.GPG_PASSPHRASE ??
  process.env.RCHAT_APT_PASSPHRASE ??
  "";
const gpgPassArgs = passphrase ? ["--pinentry-mode", "loopback", "--passphrase-fd", "0"] : [];
const gpgInput = passphrase ? `${passphrase}\n` : undefined;

const pool = path.join(out, "pool/main/r/rchat");
const dists = path.join(out, "dists/stable");
const binaryDir = path.join(dists, "main/binary-amd64");
fs.mkdirSync(pool, { recursive: true });
fs.mkdirSync(binaryDir, { recursive: true });

const target = path.join(pool, `rchat_${version}_amd64.deb`);
fs.copyFileSync(debPath, target);
console.log(`apt-repo: staged ${path.relative(out, target)}`);

// Keep the pool small: GitHub Pages has a soft 1 GB limit and every release is
// ~80 MB. Old versions stay installable until they fall out of this window.
const versions = fs
  .readdirSync(pool)
  .filter((name) => /^rchat_.+_amd64\.deb$/.test(name))
  .map((name) => ({ name, version: name.slice("rchat_".length, -"_amd64.deb".length) }))
  .sort((a, b) => compareVersions(b.version, a.version));
for (const old of versions.slice(keep)) {
  fs.rmSync(path.join(pool, old.name));
  console.log(`apt-repo: pruned ${old.name}`);
}

run("sh", ["-c", `apt-ftparchive packages pool > ${quote(path.join(binaryDir, "Packages"))}`], {
  cwd: out,
});
run("gzip", ["-kf", path.join(binaryDir, "Packages")]);

const releaseConf = path.join(out, "release.conf");
fs.writeFileSync(
  releaseConf,
  `APT::FTPArchive::Release {
  Origin "rchat";
  Label "rchat";
  Suite "stable";
  Codename "stable";
  Architectures "amd64";
  Components "main";
  Description "rchat desktop packages";
};
`,
);

const releasePath = path.join(dists, "Release");
// Generate to a temporary file outside dists/ so the Release entry does not list
// itself (apt-ftparchive hashes every file it finds under the tree).
const stagedRelease = path.join(out, ".Release.tmp");
fs.rmSync(releasePath, { force: true });
run(
  "sh",
  ["-c", `apt-ftparchive -c release.conf release dists/stable > ${quote(stagedRelease)}`],
  { cwd: out },
);
fs.renameSync(stagedRelease, releasePath);
fs.rmSync(releaseConf);

const gpg = ["--batch", "--yes", ...gpgPassArgs, "--local-user", keyId];
run("gpg", [...gpg, "--clearsign", "-o", path.join(dists, "InRelease"), releasePath], {
  env: { ...process.env, GNUPGHOME: gnupg },
  input: gpgInput,
});
run(
  "gpg",
  [...gpg, "--detach-sign", "--armor", "-o", path.join(dists, "Release.gpg"), releasePath],
  { env: { ...process.env, GNUPGHOME: gnupg }, input: gpgInput },
);

fs.copyFileSync(
  path.join(tauriDir, "packaging/deb/rchat-archive-keyring.gpg"),
  path.join(out, "rchat-archive-keyring.gpg"),
);

// A tiny landing page makes https://<user>.github.io/rchat/deb/ self-explaining
// instead of a bare directory listing.
fs.writeFileSync(
  path.join(out, "index.html"),
  `<!doctype html>
<meta charset="utf-8">
<title>rchat apt repository</title>
<h1>rchat apt repository</h1>
<p>Add it once on Debian or Ubuntu:</p>
<pre>curl -fsSL https://bestusecase.github.io/rchat/deb/rchat-archive-keyring.gpg \\
  | sudo tee /usr/share/keyrings/rchat-archive-keyring.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/rchat-archive-keyring.gpg] https://bestusecase.github.io/rchat/deb stable main" \\
  | sudo tee /etc/apt/sources.list.d/rchat.list
sudo apt update &amp;&amp; sudo apt install rchat</pre>
<p>Installed packages upgrade automatically, or run <code>sudo apt upgrade rchat</code>.</p>
`,
);

const packages = fs.readFileSync(path.join(binaryDir, "Packages"), "utf8");
if (!packages.includes(`Version: ${version}`)) fail("Packages file does not contain the version");
const inRelease = fs.readFileSync(path.join(dists, "InRelease"), "utf8");
if (!inRelease.includes("BEGIN PGP SIGNED MESSAGE")) fail("InRelease is not signed");

console.log(`apt-repo: wrote ${path.relative(repoRoot, out)} (version ${version})`);
console.log(`apt-repo: sha256 ${sha256(target)}`);

function quote(value) {
  return `'${value.replace(/'/g, "'\\''")}'`;
}

function sha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function compareVersions(a, b) {
  const parse = (v) => v.split(/[.~-]/).map((part) => (/^\d+$/.test(part) ? Number(part) : part));
  const left = parse(a);
  const right = parse(b);
  for (let i = 0; i < Math.max(left.length, right.length); i += 1) {
    const l = left[i] ?? 0;
    const r = right[i] ?? 0;
    if (l === r) continue;
    if (typeof l === "number" && typeof r === "number") return l - r;
    return String(l).localeCompare(String(r));
  }
  return 0;
}
