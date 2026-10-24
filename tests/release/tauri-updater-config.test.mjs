import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import test from "node:test";

const script = fileURLToPath(new URL("../../scripts/release/create-tauri-updater-config.mjs", import.meta.url));
const publicKey = Buffer.from(
  "untrusted comment: minisign public key: test\nRWQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n",
).toString("base64");

test("release updater config pins HTTPS feed and signed-version policy", async () => {
  const directory = await mkdtemp(join(tmpdir(), "buglogin-updater-config-"));
  try {
    const output = join(directory, "updater.json");
    const result = spawnSync(process.execPath, [script, output], {
      encoding: "utf8",
      env: {
        ...process.env,
        BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY: publicKey,
        TAURI_SIGNING_PRIVATE_KEY: "test-only-secret-not-written-to-config",
        BUGLOGIN_TAURI_UPDATER_ENDPOINT: "https://updates.example.test/stable/latest.json",
      },
    });
    assert.equal(result.status, 0, result.stderr);
    const config = JSON.parse(await readFile(output, "utf8"));
    assert.equal(config.bundle.createUpdaterArtifacts, true);
    assert.deepEqual(config.plugins.updater.endpoints, ["https://updates.example.test/stable/latest.json"]);
    assert.equal(config.plugins.updater.pubkey, publicKey);
    assert.equal(config.plugins.updater.requireSignedVersion, true);
    assert.equal(JSON.stringify(config).includes("test-only-secret"), false);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("release updater config fails closed without signing credentials", async () => {
  const result = spawnSync(process.execPath, [script, "unused.json"], {
    encoding: "utf8",
    env: { PATH: process.env.PATH },
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY/);
});

test("release updater config rejects insecure endpoints", async () => {
  const directory = await mkdtemp(join(tmpdir(), "buglogin-updater-config-"));
  try {
    const result = spawnSync(process.execPath, [script, join(directory, "updater.json")], {
      encoding: "utf8",
      env: {
        ...process.env,
        BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY: publicKey,
        TAURI_SIGNING_PRIVATE_KEY: "test-only",
        BUGLOGIN_TAURI_UPDATER_ENDPOINT: "http://updates.example.test/latest.json",
      },
    });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /HTTPS URL/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
