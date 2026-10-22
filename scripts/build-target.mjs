import { tmpdir } from "node:os";
import { resolve, win32 } from "node:path";

export const WINDOWS_CARGO_TARGET_DIR = "E:\\buglogin-target";

function canonicalWindowsPath(value) {
  const withoutDevicePrefix = value.replace(/^\\\\\?\\/, "");
  return win32.resolve(withoutDevicePrefix).replace(/[\\/]+$/, "").toLowerCase();
}

export function resolveBuildTargetDir(candidate, platform = process.platform) {
  if (platform === "win32") {
    if (
      candidate &&
      canonicalWindowsPath(candidate) !==
        canonicalWindowsPath(WINDOWS_CARGO_TARGET_DIR)
    ) {
      throw new Error(
        `BugLogin builds must use the single target ${WINDOWS_CARGO_TARGET_DIR}`,
      );
    }
    return WINDOWS_CARGO_TARGET_DIR;
  }

  return candidate
    ? resolve(candidate)
    : resolve(tmpdir(), "buglogin-target");
}
