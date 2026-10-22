import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { access } from "node:fs/promises";
import { resolve } from "node:path";
import process from "node:process";
import { resolveBuildTargetDir } from "../../scripts/build-target.mjs";

const root = resolve(import.meta.dirname, "../..");
const target =
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  process.env.RUST_TARGET ||
  process.env.TARGET ||
  execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(
    /^host: (.+)$/m,
  )?.[1];
if (!target) throw new Error("Rust target triple unavailable");

const cargoTargetDir = resolveBuildTargetDir(process.env.CARGO_TARGET_DIR);
const cargoArgs = [
  "build",
  "-p",
  "buglogin-migration",
  "--bin",
  "buglogin-migrate",
  "--release",
];
if (
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  process.env.RUST_TARGET ||
  process.env.TARGET
)
  cargoArgs.push("--target", target);
process.env.CARGO_TARGET_DIR = cargoTargetDir;
execFileSync("cargo", cargoArgs, { cwd: root, stdio: "inherit" });

const targetDir = cargoTargetDir;
const targetRoot =
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  process.env.RUST_TARGET ||
  process.env.TARGET
    ? resolve(targetDir, target, "release")
    : resolve(targetDir, "release");
const extension = process.platform === "win32" ? ".exe" : "";
const source = resolve(targetRoot, `buglogin-migrate${extension}`);
if (!existsSync(source)) throw new Error(`Migration binary missing: ${source}`);
await access(source);
console.log(`Migration sidecar built at ${source}`);
