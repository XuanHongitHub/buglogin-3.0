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
import { resolve } from "node:path";

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
