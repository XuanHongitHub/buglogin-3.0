#!/usr/bin/env node

import { access, cp, lstat } from "node:fs/promises";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBuildTargetDir } from "./build-target.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const desktopRoot = resolve(repositoryRoot, "apps/desktop");
const source = resolve(
  resolveBuildTargetDir(process.env.CARGO_TARGET_DIR),
  "desktop-dist",
);
const destinationValue = process.env.BUGLOGIN_TAURI_FRONTEND_DIST_STAGE?.trim();
if (!destinationValue) {
  throw new Error("BUGLOGIN_TAURI_FRONTEND_DIST_STAGE is required");
}

const destination = resolve(destinationValue);
const relativeDestination = relative(desktopRoot, destination);
if (
  !relativeDestination ||
  relativeDestination === ".." ||
  relativeDestination.startsWith(`..${sep}`)
) {
  throw new Error("frontend staging must remain inside apps/desktop");
}
if (!/^\.tauri-frontend-dist-[0-9a-f-]{36}$/i.test(relativeDestination)) {
  throw new Error("frontend staging path is not a generated Tauri build path");
}

const sourceIndex = resolve(source, "index.html");
await access(sourceIndex);
try {
  await lstat(destination);
  throw new Error("refusing to overwrite existing frontend staging directory");
} catch (error) {
  if (error.code !== "ENOENT") throw error;
}

await cp(source, destination, {
  recursive: true,
  errorOnExist: true,
  force: false,
});
console.log("Staged Tauri frontend under a relative config path");
