import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(
  new URL(
    "../../scripts/release/assemble-tauri-updater-feed.mjs",
    import.meta.url,
  ),
);
const downloadsVerifier = fileURLToPath(
  new URL("../../scripts/release/verify-downloads-manifest.mjs", import.meta.url),
);
const targets = [
  ["windows-x64", "windows-x86_64", "BugLogin_1.2.3_x64-setup.exe"],
  ["macos-arm64", "darwin-aarch64", "BugLogin_1.2.3_aarch64.app.tar.gz"],
];

async function fixture(root, version = "1.2.3") {
  for (const [platform, , artifact] of targets) {
    const directory = join(root, `buglogin-v${version}-${platform}-signed`);
    await mkdir(directory, { recursive: true });
    await writeFile(join(directory, artifact), `signed bundle ${platform}`);
    await writeFile(
      join(directory, `${artifact}.sig`),
      `untrusted-fixture-signature-${platform}`,
    );
    if (platform === "macos-arm64") {
      await writeFile(
        join(directory, `BugLogin_${version}_aarch64.dmg`),
        "notarized installer fixture",
      );
    }
  }
}

test("updater feed binds each supported target to a signed immutable release asset", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-updater-feed-"));
  try {
    const incoming = join(root, "incoming");
    const output = join(root, "output");
    await fixture(incoming);
    const result = spawnSync(
      process.execPath,
      [script, incoming, output, "1.2.3", "keyduc91/Malvanut-Login"],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(
      await readFile(join(output, "latest.json"), "utf8"),
    );
    const downloads = JSON.parse(
      await readFile(join(output, "downloads.json"), "utf8"),
    );
    assert.equal(manifest.version, "1.2.3");
    assert.equal(Object.hasOwn(manifest, "downloads"), false);
    assert.deepEqual(
      Object.keys(manifest.platforms).sort(),
      targets.map(([, key]) => key).sort(),
    );
    assert.equal(
      downloads.version,
      manifest.version,
    );
    assert.equal(
      downloads.platforms.windows.url,
      manifest.platforms["windows-x86_64"].url,
    );
    assert.match(
      downloads.platforms.macos.url,
      /BugLogin_1\.2\.3_aarch64\.dmg$/,
    );
    for (const [, key, artifact] of targets) {
      assert.match(
        manifest.platforms[key].url,
        new RegExp(
          `/releases/download/v1\\.2\\.3/${artifact.replaceAll(".", "\\.")}$`,
        ),
      );
      assert.match(
        manifest.platforms[key].signature,
        /^untrusted-fixture-signature-/,
      );
      await readFile(join(output, artifact));
      await readFile(join(output, `${artifact}.sig`));
    }
    await readFile(join(output, "BugLogin_1.2.3_aarch64.dmg"));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("updater feed refuses missing signatures and ambiguous platform artifacts", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-updater-feed-"));
  try {
    const incoming = join(root, "incoming");
    await fixture(incoming);
    await rm(
      join(
        incoming,
        "buglogin-v1.2.3-windows-x64-signed",
        "BugLogin_1.2.3_x64-setup.exe.sig",
      ),
    );
    const result = spawnSync(
      process.execPath,
      [
        script,
        incoming,
        join(root, "output"),
        "1.2.3",
        "keyduc91/Malvanut-Login",
      ],
      { encoding: "utf8" },
    );
    assert.notEqual(result.status, 0);
    await writeFile(
      join(incoming, "buglogin-v1.2.3-windows-x64-signed", "extra.exe"),
      "second artifact",
    );
    const duplicate = spawnSync(
      process.execPath,
      [
        script,
        incoming,
        join(root, "output-duplicate"),
        "1.2.3",
        "keyduc91/Malvanut-Login",
      ],
      { encoding: "utf8" },
    );
    assert.notEqual(duplicate.status, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("updater feed can point signed assets at an immutable BugLogin Spaces version path", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-updater-spaces-"));
  try {
    const incoming = join(root, "incoming");
    const output = join(root, "output");
    await fixture(incoming);
    const base =
      "https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3/artifacts/v1.2.3/12345/";
    const result = spawnSync(
      process.execPath,
      [script, incoming, output, "1.2.3", "keyduc91/Malvanut-Login", base],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(
      await readFile(join(output, "latest.json"), "utf8"),
    );
    for (const [, key, artifact] of targets)
      assert.equal(
        manifest.platforms[key].url,
        `${base}${encodeURIComponent(artifact)}`,
      );
    const invalid = spawnSync(
      process.execPath,
      [
        script,
        incoming,
        join(root, "invalid"),
        "1.2.3",
        "keyduc91/Malvanut-Login",
        "http://spaces.example.test/v1.2.3/",
      ],
      { encoding: "utf8" },
    );
    assert.notEqual(invalid.status, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("manual download manifest accepts only the Windows and macOS installers from the same immutable release", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-downloads-manifest-"));
  try {
    const manifestPath = join(root, "downloads.json");
    const artifactRoot =
      "https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3/artifacts/v3.0.6/12345/";
    const manifest = {
      version: "3.0.6",
      platforms: {
        windows: { url: `${artifactRoot}BugLogin_3.0.6_x64-setup.exe` },
        macos: { url: `${artifactRoot}BugLogin_3.0.6_aarch64.dmg` },
      },
    };
    await writeFile(manifestPath, JSON.stringify(manifest));
    const valid = spawnSync(
      process.execPath,
      [downloadsVerifier, manifestPath, "3.0.6", artifactRoot],
      { encoding: "utf8" },
    );
    assert.equal(valid.status, 0, valid.stderr);

    manifest.platforms.macos.url = manifest.platforms.macos.url.replace(
      "/v3.0.6/12345/",
      "/v3.0.5/12344/",
    );
    await writeFile(manifestPath, JSON.stringify(manifest));
    const wrongRelease = spawnSync(
      process.execPath,
      [downloadsVerifier, manifestPath, "3.0.6", artifactRoot],
      { encoding: "utf8" },
    );
    assert.notEqual(wrongRelease.status, 0);
    assert.match(wrongRelease.stderr, /outside the immutable release/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
