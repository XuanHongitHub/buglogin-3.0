import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, createWriteStream, promises as fs } from "node:fs";
import path from "node:path";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import { parseArgs } from "node:util";

const { values } = parseArgs({
  options: {
    directory: { type: "string" },
    engine: { type: "string", default: "adryfish" },
  },
});
assert.ok(values.directory, "--directory is required");
const directory = path.resolve(values.directory);
assert.ok(["adryfish", "fury"].includes(values.engine), "known test engine");
const fury = values.engine === "fury";
const version = fury ? "155.0.8059.39" : "148.0.7778.215";
const releases = fury
  ? {
      win32: {
        name: "fury-core-0.4.16-windows-x64.tar.xz",
        sha256:
          "3918ae93063bcbc20ab60b176230e11f8f7d762be3460a8f50f935f92beb91ad",
      },
      darwin: {
        name: "fury-core-0.4.16-macos-arm64.tar.xz",
        sha256:
          "2c63b05cc9f95a11bcf0cccf893f29bdee35b8d2ff306a683261fba7a7ef14f6",
      },
    }
  : {
      win32: {
        name: "ungoogled-chromium_148.0.7778.215-1.1_windows_x64.zip",
        sha256:
          "9ef3f471b7a6641b4224532522b29141ce3746e27d55788d88e2fd951f362579",
      },
      darwin: {
        name: "ungoogled-chromium_148.0.7778.215-1.1_macos.dmg",
        sha256:
          "b72f091e2e1a7583eed389c4b8e3534ed355e568af8c8bbf8fc30a25e23ca679",
      },
    };
const release = releases[process.platform];
assert.ok(release, `No approved native smoke package for ${process.platform}`);
await fs.mkdir(directory, { recursive: true });
const archive = path.join(directory, release.name);
const url = fury
  ? `https://github.com/furyteamtop/fury-antidetect-browser/releases/download/v0.4.16/${release.name}`
  : `https://github.com/adryfish/fingerprint-chromium/releases/download/${version}/${release.name}`;
console.log(`Downloading ${release.name} from ${url}`);
const controller = new AbortController();
let idleTimer;
const resetIdleTimer = () => {
  clearTimeout(idleTimer);
  idleTimer = setTimeout(
    () => controller.abort(new Error("No download data received for 120 seconds")),
    120_000,
  );
  idleTimer.unref?.();
};
resetIdleTimer();
const response = await fetch(url, { signal: controller.signal });
assert.equal(response.status, 200, "official engine package download");
assert.ok(response.body, "official engine package response body");
let downloadedBytes = 0;
let nextProgressBytes = 16 * 1024 * 1024;
const progress = new Transform({
  transform(chunk, _encoding, callback) {
    downloadedBytes += chunk.length;
    resetIdleTimer();
    if (downloadedBytes >= nextProgressBytes) {
      console.log(`Downloaded ${(downloadedBytes / 1024 / 1024).toFixed(0)} MiB`);
      nextProgressBytes += 16 * 1024 * 1024;
    }
    callback(null, chunk);
  },
});
try {
  await pipeline(
    Readable.fromWeb(response.body),
    progress,
    createWriteStream(archive, { flags: "wx" }),
  );
} finally {
  clearTimeout(idleTimer);
}
console.log(`Download complete: ${(downloadedBytes / 1024 / 1024).toFixed(1)} MiB`);
async function sha256(file) {
  const hash = createHash("sha256");
  for await (const bytes of createReadStream(file)) hash.update(bytes);
  return hash.digest("hex");
}
assert.equal(
  await sha256(archive),
  release.sha256,
  "official GitHub release archive digest",
);
const extracted = path.join(directory, "engine");
await fs.mkdir(extracted);
let binary;
if (fury) {
  console.log("Extracting Fury package");
  execFileSync("tar", ["-xf", archive, "-C", extracted], {
    stdio: "inherit",
    timeout: 15 * 60 * 1000,
  });
  console.log("Fury package extracted");
  if (process.platform === "win32") {
    binary = path.join(extracted, "Fury", "chrome.exe");
  } else {
    const apps = (await fs.readdir(extracted)).filter((entry) =>
      entry.endsWith(".app"),
    );
    assert.equal(apps.length, 1, "expected one core app");
    const app = path.join(extracted, apps[0]);
    const executable = execFileSync(
      "/usr/libexec/PlistBuddy",
      [
        "-c",
        "Print :CFBundleExecutable",
        path.join(app, "Contents", "Info.plist"),
      ],
      { encoding: "utf8" },
    ).trim();
    assert.ok(executable && path.basename(executable) === executable);
    binary = path.join(app, "Contents", "MacOS", executable);
  }
} else if (process.platform === "win32") {
  // Paths travel via environment variables, never interpolated as shell code.
  execFileSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "Expand-Archive -LiteralPath $env:BUGLOGIN_SMOKE_ARCHIVE -DestinationPath $env:BUGLOGIN_SMOKE_ENGINE",
    ],
    {
      windowsHide: true,
      env: {
        ...process.env,
        BUGLOGIN_SMOKE_ARCHIVE: archive,
        BUGLOGIN_SMOKE_ENGINE: extracted,
      },
    },
  );
  const entries = await fs.readdir(extracted);
  assert.equal(entries.length, 1, "expected one engine directory");
  binary = path.join(extracted, entries[0], "chrome.exe");
} else {
  const mount = path.join(directory, "mount");
  await fs.mkdir(mount);
  execFileSync(
    "hdiutil",
    [
      "attach",
      "-quiet",
      "-readonly",
      "-nobrowse",
      "-mountpoint",
      mount,
      archive,
    ],
    { timeout: 60000 },
  );
  try {
    const apps = (await fs.readdir(mount)).filter((entry) =>
      entry.endsWith(".app"),
    );
    assert.equal(apps.length, 1, "expected one browser app");
    const destination = path.join(extracted, apps[0]);
    execFileSync("ditto", [path.join(mount, apps[0]), destination]);
    const executable = execFileSync(
      "/usr/libexec/PlistBuddy",
      [
        "-c",
        "Print :CFBundleExecutable",
        path.join(destination, "Contents", "Info.plist"),
      ],
      { encoding: "utf8" },
    ).trim();
    assert.ok(
      executable && path.basename(executable) === executable,
      "valid bundle executable",
    );
    binary = path.join(destination, "Contents", "MacOS", executable);
  } finally {
    execFileSync("hdiutil", ["detach", "-quiet", mount], { timeout: 30000 });
  }
}
const metadata = {
  engine: values.engine,
  version,
  binary,
  sha256: await sha256(binary),
  archiveSha256: release.sha256,
  source: url,
};
await fs.writeFile(
  path.join(directory, "engine-metadata.json"),
  `${JSON.stringify(metadata, null, 2)}\n`,
);
console.log(JSON.stringify(metadata, null, 2));
