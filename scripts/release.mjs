#!/usr/bin/env node
// Cut a release: build both bundles, sign the updater artifact, write the
// updater manifest, refresh the apt repository, and optionally publish.
//
//   node scripts/release.mjs                     # build + stage under dist/
//   node scripts/release.mjs --upload            # also publish a GitHub release
//   node scripts/release.mjs --upload --pages    # and push the apt repo to gh-pages
//
// Environment:
//   TAURI_SIGNING_PRIVATE_KEY(+_PASSWORD)  updater signing key (path or literal)
//   RCHAT_APT_KEY                           archive key fingerprint for the apt repo
//   GPG_PASSPHRASE                          archive key passphrase
//   GITHUB_TOKEN                            token for --upload/--pages (or token file)
//
// Passphrases default to ~/.config/rchat/release/{updater,apt}.pass (mode 600).
//
// The two keys are independent: minisign proves the AppImage came from us, the
// GPG key proves apt metadata came from us.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const desktopDir = path.join(repoRoot, "apps/desktop");
const tauriDir = path.join(desktopDir, "src-tauri");
const distDir = path.join(repoRoot, "dist");
const owner = process.env.RCHAT_REPO_OWNER || "bestusecase";
const repo = process.env.RCHAT_REPO_NAME || "rchat";

const upload = process.argv.includes("--upload");
const pages = process.argv.includes("--pages");
const skipBuild = process.argv.includes("--no-build");
const notesFile = valueFor("--notes");
const pagesOnly = process.argv.includes("--pages-only");

const conf = JSON.parse(fs.readFileSync(path.join(tauriDir, "tauri.conf.json"), "utf8"));
const version = conf.version;
const tag = `v${version}`;

const releaseKey =
  process.env.TAURI_SIGNING_PRIVATE_KEY ||
  path.join(os.homedir(), ".config/rchat/release/updater.key");

// The keys are passphrase-protected. Keep the passphrases out of argv: read
// them from the environment or from mode-600 files next to the keys.
function secret(envName, fileName) {
  const fromEnv = process.env[envName];
  if (fromEnv) return fromEnv.trim();
  const file = path.join(os.homedir(), ".config/rchat/release", fileName);
  return fs.existsSync(file) ? fs.readFileSync(file, "utf8").trim() : "";
}

const updaterPassword = secret("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "updater.pass");
const aptPassphrase = secret("GPG_PASSPHRASE", "apt.pass");

function valueFor(flag) {
  const index = process.argv.indexOf(flag);
  return index >= 0 ? process.argv[index + 1] : null;
}

function log(message) {
  console.log(`release: ${message}`);
}

if (pagesOnly) {
  publishPages(path.join(distDir, "repo"));
  process.exit(0);
}

function fail(message) {
  console.error(`release: ${message}`);
  process.exit(1);
}

function sha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function token() {
  if (process.env.GITHUB_TOKEN) return process.env.GITHUB_TOKEN.trim();
  const file = path.join(os.homedir(), ".rchat-release-token");
  if (fs.existsSync(file)) return fs.readFileSync(file, "utf8").trim();
  fail("--upload/--pages needs GITHUB_TOKEN or ~/.rchat-release-token");
}

// ---------------------------------------------------------------- build

if (!skipBuild) {
  if (!fs.existsSync(releaseKey)) {
    fail(`signing key not found at ${releaseKey}; run: cargo tauri signer generate -w ${releaseKey}`);
  }
  log(`building ${tag}`);
  execFileSync("cargo", ["tauri", "build"], {
    cwd: desktopDir,
    stdio: "inherit",
    env: {
      ...process.env,
      TAURI_SIGNING_PRIVATE_KEY: releaseKey,
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: updaterPassword,
    },
  });
}

const bundle = path.join(tauriDir, "target/release/bundle");
const artifacts = {
  deb: path.join(bundle, `deb/rchat_${version}_amd64.deb`),
  appimage: path.join(bundle, `appimage/rchat_${version}_amd64.AppImage`),
  signature: path.join(bundle, `appimage/rchat_${version}_amd64.AppImage.sig`),
};
for (const [name, file] of Object.entries(artifacts)) {
  if (!fs.existsSync(file)) fail(`missing ${name}: ${file}`);
}

fs.mkdirSync(distDir, { recursive: true });

// ------------------------------------------------------- updater manifest

const manifest = {
  version,
  notes: notesFile
    ? fs.readFileSync(path.resolve(notesFile), "utf8")
    : `rchat ${version}`,
  pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  platforms: {
    "linux-x86_64": {
      signature: fs.readFileSync(artifacts.signature, "utf8").trim(),
      url: `https://github.com/${owner}/${repo}/releases/download/${tag}/rchat_${version}_amd64.AppImage`,
    },
  },
};
const manifestPath = path.join(distDir, "latest.json");
fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
log(`wrote ${path.relative(repoRoot, manifestPath)}`);

// ----------------------------------------------------------- apt repository

const repoDir = path.join(distDir, "repo");
if (!process.env.RCHAT_APT_KEY) {
  fail("set RCHAT_APT_KEY to the archive key fingerprint before building the apt repo");
}
execFileSync(
  "node",
  [
    path.join(repoRoot, "scripts/apt-repo.mjs"),
    "--deb",
    artifacts.deb,
    "--version",
    version,
    "--out",
    repoDir,
  ],
  {
    stdio: "inherit",
    env: { ...process.env, GPG_PASSPHRASE: aptPassphrase },
  },
);

// ------------------------------------------------------------- checksums

const sums = path.join(distDir, "SHA256SUMS");
fs.writeFileSync(
  sums,
  [
    `${sha256(artifacts.deb)}  rchat_${version}_amd64.deb`,
    `${sha256(artifacts.appimage)}  rchat_${version}_amd64.AppImage`,
  ].join("\n") + "\n",
);
log(`wrote ${path.relative(repoRoot, sums)}`);

if (!upload) {
  log("staged only; pass --upload to publish");
  process.exit(0);
}

// --------------------------------------------------------------- publish

const auth = token();

async function github(pathname, options = {}) {
  const response = await fetch(`https://api.github.com${pathname}`, {
    ...options,
    headers: {
      Authorization: `Bearer ${auth}`,
      Accept: "application/vnd.github+json",
      "User-Agent": "rchat-release",
      ...(options.body ? { "Content-Type": "application/json" } : {}),
      ...options.headers,
    },
  });
  const text = await response.text();
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = null;
  }
  return { ok: response.ok, status: response.status, json, text };
}

async function must(pathname, options) {
  const result = await github(pathname, options);
  if (!result.ok) {
    fail(`${options?.method || "GET"} ${pathname} -> ${result.status}: ${result.json?.message || result.text.slice(0, 300)}`);
  }
  return result.json;
}

const existing = await github(`/repos/${owner}/${repo}/releases/tags/${tag}`);

// Create the tag before the release: without it GitHub would create the tag
// from the default branch (which is not where rchat lives).
if (!spawnQuiet("git", ["rev-parse", "--verify", `refs/tags/${tag}`], repoRoot)) {
  execFileSync("git", ["tag", "-a", tag, "-m", `rchat ${tag}`], { cwd: repoRoot, stdio: "inherit" });
  log(`created tag ${tag}`);
  execFileSync("git", ["push", "origin", `refs/tags/${tag}`], {
    cwd: repoRoot,
    stdio: "inherit",
    env: {
      ...process.env,
      GIT_ASKPASS: askpassScript(),
      GIT_TERMINAL_PROMPT: "0",
      GITHUB_TOKEN: auth,
    },
  });
}

let release;
if (existing.ok) {
  release = await must(`/repos/${owner}/${repo}/releases/${existing.json.id}`, {
    method: "PATCH",
    body: JSON.stringify({ name: `rchat ${tag}`, body: manifest.notes, prerelease: false }),
  });
} else if (existing.status === 404) {
  release = await must(`/repos/${owner}/${repo}/releases`, {
    method: "POST",
    body: JSON.stringify({
      tag_name: tag,
      name: `rchat ${tag}`,
      body: manifest.notes,
      draft: false,
      prerelease: false,
    }),
  });
} else {
  fail(`could not look up ${tag}: ${existing.status} ${existing.text.slice(0, 200)}`);
}
log(`release ${release.html_url}`);

const assetNames = [
  [`rchat_${version}_amd64.deb`, artifacts.deb, "application/vnd.debian.binary-package"],
  [`rchat_${version}_amd64.AppImage`, artifacts.appimage, "application/octet-stream"],
  [`rchat_${version}_amd64.AppImage.sig`, artifacts.signature, "text/plain"],
  ["latest.json", manifestPath, "application/json"],
  ["SHA256SUMS", sums, "text/plain"],
];

for (const [name, file, contentType] of assetNames) {
  const previous = (release.assets || []).find((asset) => asset.name === name);
  if (previous) {
    await must(`/repos/${owner}/${repo}/releases/assets/${previous.id}`, { method: "DELETE" });
  }
  const data = fs.readFileSync(file);
  const response = await fetch(
    `https://uploads.github.com/repos/${owner}/${repo}/releases/${release.id}/assets?name=${encodeURIComponent(name)}`,
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${auth}`,
        Accept: "application/vnd.github+json",
        "User-Agent": "rchat-release",
        "Content-Type": contentType,
      },
      body: data,
    },
  );
  if (!response.ok) {
    const text = await response.text();
    fail(`upload ${name} -> ${response.status}: ${text.slice(0, 300)}`);
  }
  log(`uploaded ${name} (${data.length} bytes)`);
}

if (pages) {
  publishPages(repoDir);
}

function publishPages(sourceDir) {
  if (!fs.existsSync(sourceDir)) fail(`no apt repo to publish at ${sourceDir}`);
  const work = fs.mkdtempSync(path.join(os.tmpdir(), "rchat-pages-"));
  const tokenValue = token();
  const askpass = askpassScript();
  try {
    execFileSync("git", ["init", "-q", "-b", "gh-pages"], { cwd: work });
    fs.cpSync(sourceDir, path.join(work, "deb"), { recursive: true });
    execFileSync("git", ["add", "-A"], { cwd: work });
    execFileSync(
      "git",
      [
        "-c",
        "user.name=rchat",
        "-c",
        "user.email=dev@localhost",
        "commit",
        "-q",
        "-m",
        `apt repo ${tag}`,
      ],
      { cwd: work },
    );
    // The branch is owned by this script; a force push keeps history linear and
    // lets a fresh machine publish without cloning first.
    execFileSync(
      "git",
      ["push", "--force", `https://github.com/${owner}/${repo}.git`, "gh-pages"],
      {
        cwd: work,
        stdio: "inherit",
        env: {
          ...process.env,
          GIT_ASKPASS: askpass,
          GIT_TERMINAL_PROMPT: "0",
          GITHUB_TOKEN: tokenValue,
        },
      },
    );
  } finally {
    fs.rmSync(askpass, { force: true });
    fs.rmSync(work, { recursive: true, force: true });
  }
  log("pushed gh-pages");
}

// A throwaway GIT_ASKPASS helper keeps the token out of argv (visible in ps).
function askpassScript() {
  const file = path.join(distDir, "git-askpass.sh");
  fs.writeFileSync(
    file,
    '#!/bin/sh\ncase "$1" in\n  *sername*) echo x-access-token ;;\n  *) echo "$GITHUB_TOKEN" ;;\nesac\n',
    { mode: 0o700 },
  );
  return file;
}

function spawnQuiet(command, args, cwd) {
  try {
    execFileSync(command, args, { cwd, stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}
