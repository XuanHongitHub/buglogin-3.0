import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, createWriteStream, promises as fs } from "node:fs";
import path from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { parseArgs } from "node:util";

const { values } = parseArgs({ options: { directory: { type: "string" } } });
assert.ok(values.directory, "--directory is required");
const directory = path.resolve(values.directory);
const version = "148.0.7778.215";
const releases = {
  win32: {
    name: "ungoogled-chromium_148.0.7778.215-1.1_windows_x64.zip",
    sha256: "9ef3f471b7a6641b4224532522b29141ce3746e27d55788d88e2fd951f362579",
  },
  darwin: {
    name: "ungoogled-chromium_148.0.7778.215-1.1_macos.dmg",
    sha256: "b72f091e2e1a7583eed389c4b8e3534ed355e568af8c8bbf8fc30a25e23ca679",
  },
};
const release = releases[process.platform];
assert.ok(release, `No approved native smoke package for ${process.platform}`);
await fs.mkdir(directory, { recursive: true });
const archive = path.join(directory, release.name);
const url = `https://github.com/adryfish/fingerprint-chromium/releases/download/${version}/${release.name}`;
const response = await fetch(url);
assert.equal(response.status, 200, "official engine package download");
await pipeline(
  Readable.fromWeb(response.body),
  createWriteStream(archive, { flags: "wx" }),
);
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
if (process.platform === "win32") {
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
