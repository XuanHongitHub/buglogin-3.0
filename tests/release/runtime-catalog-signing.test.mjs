import assert from "node:assert/strict";
import {
  createHash,
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  verify,
} from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  buildSignedRuntimeCatalog,
  runtimeReleasePayload,
} from "../../scripts/runtime/sign-catalog.mjs";

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "buglogin-runtime-signing-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const artifactPath = join(root, "runtime.zip");
  const privateKeyPath = join(root, "runtime-signing-key.pem");
  const artifact = Buffer.from("synthetic runtime archive");
  const seed = Buffer.alloc(32, 7);
  const privateKey = createPrivateKey({
    key: Buffer.concat([
      Buffer.from("302e020100300506032b657004220420", "hex"),
      seed,
    ]),
    type: "pkcs8",
    format: "der",
  });
  const publicKey = createPublicKey(privateKey);
  await writeFile(artifactPath, artifact);
  await writeFile(
    privateKeyPath,
    privateKey.export({ type: "pkcs8", format: "pem" }),
    { mode: 0o600 },
  );
  return {
    root,
    artifact,
    artifactPath,
    privateKeyPath,
    privateKey,
    publicKey,
  };
}

test("signed runtime metadata matches artifact bytes and Ed25519 payload", async (t) => {
  const files = await fixture(t);
  const { catalog, trustKeys, release } = await buildSignedRuntimeCatalog({
    artifactPath: files.artifactPath,
    privateKeyPath: files.privateKeyPath,
    keyId: "runtime-test-2026",
    runtime: "wayfern",
    version: "154.0.8037.57",
    platform: "windows",
    architecture: "x64",
    relativePath: "chrome.exe",
    packageFormat: "zip",
    archivePrefix: "wayfern",
    source: "https://download.example.test/wayfern.zip",
  });

  assert.equal(catalog.schemaVersion, 2);
  assert.equal(catalog.releases.length, 1);
  assert.equal(
    release.sha256,
    createHash("sha256").update(files.artifact).digest("hex"),
  );
  assert.equal(release.sizeBytes, files.artifact.length);
  assert.equal(trustKeys.length, 1);
  assert.equal(trustKeys[0].keyId, release.keyId);
  assert.equal(
    verify(
      null,
      runtimeReleasePayload(release),
      files.publicKey,
      Buffer.from(release.signature, "hex"),
    ),
    true,
  );
});

test("signer accepts a macOS Wayfern DMG and binds its package format", async (t) => {
  const files = await fixture(t);
  const { release } = await buildSignedRuntimeCatalog({
    artifactPath: files.artifactPath,
    privateKeyPath: files.privateKeyPath,
    keyId: "runtime-test-2026",
    runtime: "wayfern",
    version: "154.0.8037.57",
    platform: "macos",
    architecture: "arm64",
    relativePath: "Wayfern.app/Contents/MacOS/Wayfern",
    packageFormat: "dmg",
    source: "https://download.example.test/wayfern.dmg",
  });
  assert.equal(release.packageFormat, "dmg");
  assert.equal(
    verify(
      null,
      runtimeReleasePayload(release),
      files.publicKey,
      Buffer.from(release.signature, "hex"),
    ),
    true,
  );
  await assert.rejects(
    buildSignedRuntimeCatalog({
      artifactPath: files.artifactPath,
      privateKeyPath: files.privateKeyPath,
      keyId: "runtime-test-2026",
      runtime: "wayfern",
      version: "154.0.8037.57",
      platform: "windows",
      architecture: "x64",
      relativePath: "Wayfern.app/Contents/MacOS/Wayfern",
      packageFormat: "dmg",
      source: "https://download.example.test/wayfern.dmg",
    }),
    /DMG packages require a macOS Wayfern executable inside an \.app bundle/,
  );
});

test("catalog update preserves verified releases and refuses a reused key ID", async (t) => {
  const files = await fixture(t);
  const base = {
    artifactPath: files.artifactPath,
    privateKeyPath: files.privateKeyPath,
    keyId: "runtime-test-2026",
    runtime: "wayfern",
    version: "154.0.8037.57",
    platform: "windows",
    architecture: "x64",
    relativePath: "chrome.exe",
    packageFormat: "zip",
    source: "https://download.example.test/wayfern.zip",
  };
  const first = await buildSignedRuntimeCatalog(base);
  const catalogPath = join(files.root, "catalog.json");
  const trustPath = join(files.root, "trust-keys.json");
  await writeFile(catalogPath, JSON.stringify(first.catalog));
  await writeFile(trustPath, JSON.stringify(first.trustKeys));

  const second = await buildSignedRuntimeCatalog({
    ...base,
    version: "154.0.8038.1",
    platform: "macos",
    architecture: "arm64",
    relativePath: "Contents/MacOS/chrome",
    existingCatalogPath: catalogPath,
    existingTrustKeysPath: trustPath,
  });
  assert.equal(second.catalog.releases.length, 2);
  assert.equal(second.trustKeys.length, 1);
  assert.deepEqual(
    JSON.parse(await readFile(trustPath, "utf8")),
    first.trustKeys,
  );

  const otherKeyPath = join(files.root, "other-key.pem");
  const { privateKey: otherPrivateKey } = generateKeyPairSync("ed25519");
  await writeFile(
    otherKeyPath,
    otherPrivateKey.export({ type: "pkcs8", format: "pem" }),
    { mode: 0o600 },
  );
  await assert.rejects(
    buildSignedRuntimeCatalog({
      ...base,
      privateKeyPath: otherKeyPath,
      existingTrustKeysPath: trustPath,
    }),
    /key ID already exists with a different public key/,
  );
});

test("catalog signer rejects unsafe paths and insecure artifact sources", async (t) => {
  const files = await fixture(t);
  const base = {
    artifactPath: files.artifactPath,
    privateKeyPath: files.privateKeyPath,
    keyId: "runtime-test-2026",
    runtime: "wayfern",
    version: "1",
    platform: "windows",
    architecture: "x64",
    relativePath: "chrome.exe",
    source: "https://download.example.test/runtime.zip",
  };
  await assert.rejects(
    buildSignedRuntimeCatalog({ ...base, relativePath: "../chrome.exe" }),
    /runtime executable path is unsafe/,
  );
  await assert.rejects(
    buildSignedRuntimeCatalog({
      ...base,
      source: "http://download.example.test/runtime.zip",
    }),
    /source must be an HTTPS URL/,
  );
});

test("existing trust keys reject fields the Rust API would reject", async (t) => {
  const files = await fixture(t);
  const trustPath = join(files.root, "trust-keys.json");
  await writeFile(
    trustPath,
    JSON.stringify([
      { keyId: "old-key", publicKey: "00".repeat(32), active: true },
    ]),
  );
  await assert.rejects(
    buildSignedRuntimeCatalog({
      artifactPath: files.artifactPath,
      privateKeyPath: files.privateKeyPath,
      keyId: "runtime-test-2026",
      runtime: "wayfern",
      version: "1",
      platform: "windows",
      architecture: "x64",
      relativePath: "chrome.exe",
      source: "https://download.example.test/runtime.zip",
      existingTrustKeysPath: trustPath,
    }),
    /existing trust keys contain an invalid or duplicate key ID/,
  );
});
