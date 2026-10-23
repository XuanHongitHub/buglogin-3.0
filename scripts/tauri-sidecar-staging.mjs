import { randomUUID } from "node:crypto";
import {
  cp,
  lstat,
  mkdir,
  readdir,
  rename,
  rmdir,
  unlink,
} from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";

const TERMINATION_SIGNALS = ["SIGINT", "SIGTERM", "SIGHUP"];

// Keeps the launcher alive on Ctrl-C, kill or terminal close so the child's
// exit handler can restore the binaries directory; the child gets the signal.
export function forwardTerminationSignals(child, launcher = process) {
  for (const signal of TERMINATION_SIGNALS) {
    launcher.on(signal, () => {
      if (child.exitCode === null) child.kill(signal);
    });
  }
}

// Restores a binaries directory left staged by a launcher that died before
// its restore ran (for example a killed terminal).
export async function recoverTauriSidecarDirectory({
  sourceBinaries,
  sidecarName,
}) {
  const parent = dirname(sourceBinaries);
  const prefix = `${basename(sourceBinaries)}.build-restore-`;
  const backups = (await readdir(parent)).filter((entry) =>
    entry.startsWith(prefix),
  );
  if (backups.length === 0) return false;
  if (backups.length > 1) {
    throw new Error(
      `Multiple Tauri binaries backups found next to ${sourceBinaries}; restore one manually`,
    );
  }
  await restoreTauriSidecarDirectory({
    sourceBinaries,
    backupBinaries: resolve(parent, backups[0]),
    sidecarName,
  });
  return true;
}

export async function restoreTauriSidecarDirectory({
  sourceBinaries,
  backupBinaries,
  sidecarName,
}) {
  try {
    const sourceStat = await lstat(sourceBinaries);
    if (sourceStat.isSymbolicLink()) {
      await unlink(sourceBinaries);
    } else if (sourceStat.isDirectory()) {
      const entries = await readdir(sourceBinaries);
      const unexpectedEntries = entries.filter(
        (entry) => entry !== ".gitkeep" && entry !== sidecarName,
      );
      if (unexpectedEntries.length > 0) {
        throw new Error(
          "Refusing to remove unexpected Tauri binaries directory contents",
        );
      }
      for (const entry of entries) {
        await unlink(resolve(sourceBinaries, entry));
      }
      await rmdir(sourceBinaries);
    } else {
      throw new Error("Refusing to replace unexpected Tauri binaries path");
    }
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  await rename(backupBinaries, sourceBinaries);
}

export async function stageTauriSidecarDirectory({
  sourceBinaries,
  sidecarStage,
  sidecarName,
}) {
  const entries = await readdir(sourceBinaries);
  const unexpectedEntries = entries.filter(
    (entry) => entry !== ".gitkeep" && entry !== sidecarName,
  );
  if (unexpectedEntries.length > 0) {
    throw new Error(
      `Refusing to replace unexpected Tauri binaries entries: ${unexpectedEntries.join(", ")}`,
    );
  }

  const backupBinaries = `${sourceBinaries}.build-restore-${randomUUID()}`;
  await rename(sourceBinaries, backupBinaries);
  try {
    await mkdir(sourceBinaries);
    await cp(
      resolve(sidecarStage, sidecarName),
      resolve(sourceBinaries, sidecarName),
    );
  } catch (error) {
    try {
      const entries = await readdir(sourceBinaries);
      if (entries.every((entry) => entry === sidecarName)) {
        for (const entry of entries)
          await unlink(resolve(sourceBinaries, entry));
        await rmdir(sourceBinaries);
      }
    } catch {
      // Preserve the original error; leave any unexpected path for inspection.
    }
    await rename(backupBinaries, sourceBinaries);
    throw error;
  }

  let staged = true;
  return async () => {
    if (!staged) return;
    await restoreTauriSidecarDirectory({
      sourceBinaries,
      backupBinaries,
      sidecarName,
    });
    staged = false;
  };
}
