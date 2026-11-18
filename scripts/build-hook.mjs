import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const isRelease = process.argv.includes("--release");
const target = process.env.TAURI_ENV_TARGET_TRIPLE;
const isUniversalMac = target === "universal-apple-darwin";
// Tauri builds all Cargo binaries for a single target, but only merges the main
// binary when producing a universal macOS bundle.
if (isRelease && !isUniversalMac) process.exit(0);

const tauriDir = path.resolve("src-tauri");
const cargoBinDir = path.join(os.homedir(), ".cargo", "bin");
const env = {
  ...process.env,
  PATH: `${cargoBinDir}${path.delimiter}${process.env.PATH || ""}`,
};

const args = ["build", "--bin", "agent-ing-hook"];
if (isRelease) args.push("--release", "--features", "tauri/custom-protocol");

const isWindows = target ? target.includes("windows") : process.platform === "win32";
const binName = isWindows ? "agent-ing-hook.exe" : "agent-ing-hook";
const profile = isRelease ? "release" : "debug";
const targetDir = path.resolve(tauriDir, process.env.CARGO_TARGET_DIR || "target");

function buildHook(triple) {
  const targetArgs = triple ? ["--target", triple] : [];
  execFileSync("cargo", [...args, ...targetArgs], { cwd: tauriDir, stdio: "inherit", env });
  return path.join(targetDir, ...(triple ? [triple] : []), profile, binName);
}

if (isUniversalMac) {
  const arm64 = buildHook("aarch64-apple-darwin");
  const x64 = buildHook("x86_64-apple-darwin");
  const universalDir = path.join(targetDir, target, profile);
  const universalHook = path.join(universalDir, binName);
  fs.mkdirSync(universalDir, { recursive: true });
  execFileSync("lipo", ["-create", arm64, x64, "-output", universalHook], {
    stdio: "inherit",
  });
  execFileSync("codesign", ["--force", "--sign", "-", universalHook], { stdio: "inherit" });
} else if (target) {
  const hook = buildHook(target);
  const nativeDir = path.join(targetDir, profile);
  fs.mkdirSync(nativeDir, { recursive: true });
  fs.copyFileSync(hook, path.join(nativeDir, binName));
} else {
  buildHook();
}
