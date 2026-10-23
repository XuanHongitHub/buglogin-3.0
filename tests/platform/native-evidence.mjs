import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { execFileSync } from "node:child_process";

const root = resolve(import.meta.dirname, "../..");
const platform = process.platform === "win32" ? "windows" : process.platform === "darwin" ? "macos" : process.platform;
const platformTest = platform === "windows" ? "platform_windows" : platform === "macos" ? "platform_macos" : null;
const results = [];

function record(name, status, detail) {
  results.push({ name, status, detail });
}

function run(name, args) {
  try {
    execFileSync("cargo", args, { cwd: root, stdio: "pipe", encoding: "utf8" });
    record(name, "passed", "native command completed");
  } catch (error) {
    record(name, "failed", `native command failed with status ${error.status ?? "unknown"}`);
  }
}

function sourceContains(path, pattern) {
  return existsSync(path) && pattern.test(readFileSync(path, "utf8"));
}

if (!platformTest) {
  record("native-vault", "not_run", `unsupported host platform: ${platform}`);
  record("process-cleanup", "not_run", `unsupported host platform: ${platform}`);
} else {
  run("native-vault", ["test", "-p", "buglogin-storage", "--test", "vault", "--", "--nocapture"]);
  run("process-cleanup", ["test", "-p", "buglogin-browser", "--test", platformTest, "--", "--nocapture"]);
}

const desktopCargo = resolve(root, "apps/desktop/src-tauri/Cargo.toml");
const desktopLib = resolve(root, "apps/desktop/src-tauri/src/lib.rs");
const cargoText = existsSync(desktopCargo) ? readFileSync(desktopCargo, "utf8") : "";
const libText = existsSync(desktopLib) ? readFileSync(desktopLib, "utf8") : "";
const deepLinkConfigured = cargoText.includes("tauri-plugin-deep-link") || /deep[_-]link/.test(libText);
const trayConfigured = cargoText.includes("tray-icon") || /TrayIcon|setup_system_tray|tray_by_id/.test(libText);

record(
  "deep-link",
  deepLinkConfigured ? "passed" : "blocked",
  deepLinkConfigured ? "desktop source/config contains deep-link integration" : "new desktop source/config has no deep-link integration",
);
record(
  "tray",
  trayConfigured ? "passed" : "blocked",
  trayConfigured ? "desktop source/config contains tray integration" : "new desktop source/config has no tray integration",
);

const summary = {
  schema: "buglogin.platform-native-evidence.v1",
  platform,
  checks: results,
  passed: results.filter(({ status }) => status === "passed").length,
  blocked: results.filter(({ status }) => status === "blocked").length,
  failed: results.filter(({ status }) => status === "failed").length,
};
console.log(JSON.stringify(summary, null, 2));
process.exitCode = summary.failed || summary.blocked ? 1 : 0;
