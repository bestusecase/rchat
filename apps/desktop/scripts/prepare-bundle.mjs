// Build the engine and stage everything the Tauri bundle expects.
//
// Kept as one script so a developer cannot accidentally ship a shell with a
// stale engine or a runtime that only exists on their machine.

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, statSync, chmodSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..");
const repoRoot = resolve(appRoot, "..", "..");
const tauriRoot = join(appRoot, "src-tauri");

function fail(message) {
  console.error(`\n  rchat bundle: ${message}\n`);
  process.exit(1);
}

function run(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (result.status !== 0) {
    fail(`${command} ${args.join(" ")} failed with code ${result.status}`);
  }
}

const engine = join(repoRoot, "target", "release", "rchatd");
console.log("building rchatd (release)…");
run("cargo", ["build", "--release", "-p", "rchatd"], repoRoot);

if (!existsSync(engine)) {
  fail(`engine was not produced at ${engine}`);
}

const resources = join(tauriRoot, "resources");
mkdirSync(resources, { recursive: true });
const staged = join(resources, "rchatd");
copyFileSync(engine, staged);
chmodSync(staged, 0o755);
console.log(`staged engine: ${staged} (${(statSync(staged).size / 1e6).toFixed(1)} MB)`);

const runtime = join(tauriRoot, "runtime");
const python = join(runtime, "py", "bin", "python3");
const sitePackages = join(runtime, "site-packages");
if (!existsSync(python)) {
  fail(`bundled Python is missing at ${python}`);
}
if (!existsSync(sitePackages)) {
  fail(`bundled Python packages are missing at ${sitePackages}`);
}

const probe = spawnSync(python, ["-c", "import RNS, LXMF; print(RNS.__version__, LXMF.__version__)"], {
  env: { ...process.env, PYTHONPATH: sitePackages },
  encoding: "utf8",
});
if (probe.status !== 0) {
  fail(`bundled runtime cannot import RNS/LXMF:\n${probe.stderr || probe.stdout}`);
}
console.log(`bundled runtime: RNS/LXMF ${probe.stdout.trim()}`);

const bridge = join(repoRoot, "sidecar", "bridge.py");
if (!existsSync(bridge)) {
  fail(`Reticulum bridge is missing at ${bridge}`);
}

const icons = ["icon.png", "32x32.png", "128x128.png", "128x128@2x.png", "tray.png", "tray-unread.png"];
const chime = join(tauriRoot, "assets", "chime.wav");
const missing = icons.filter((name) => !existsSync(join(tauriRoot, "icons", name)));
if (missing.length > 0 || !existsSync(chime)) {
  console.log("generating icons and chime…");
  run("python3", [join(tauriRoot, "scripts", "make-assets.py")], tauriRoot);
}

console.log("bundle inputs ready.\n");
