import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { verifyBundle } from "../../scripts/release/verify-bundle.mjs";

test("release bundle verifies SBOM, provenance, signatures, and rollback coverage", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-release-bundle-"));
  try {
    await writeFile(join(root, "BugLogin.exe"), "current", "utf8");
    await writeFile(join(root, "dist.spdx.json"), JSON.stringify({ spdxVersion: "SPDX-2.3", SPDXID: "SPDXRef-DOCUMENT", packages: [{ SPDXID: "SPDXRef-Package-1", name: "BugLogin", versionInfo: "1.2.0", externalRefs: [{ referenceType: "purl" }] }] }));
    const manifest = { schema: "buglogin.updater-manifest.v1", channel: "rc", version: "1.2.0", artifacts: [{ platform: "windows", arch: "x86_64", version: "1.2.0", url: "https://updates.example.invalid/1.2.0/BugLogin.exe", sha256: "a".repeat(64), signature: "signed" }], rollback: { allowed: true, dataCompatibility: "backward-compatible", previousVersion: "1.1.0", previousArtifacts: [{ platform: "windows", arch: "x86_64", version: "1.1.0", url: "https://updates.example.invalid/1.1.0/BugLogin.exe", sha256: "b".repeat(64), signature: "signed" }] } };
    const manifestBytes = Buffer.from(JSON.stringify(manifest));
    await writeFile(join(root, "updater-manifest.json"), manifestBytes);
    const { publicKey, privateKey } = generateKeyPairSync("ed25519");
    const publicKeyBase64 = publicKey.export({ format: "der", type: "spki" }).toString("base64");
    const signatureBase64 = sign(null, manifestBytes, privateKey).toString("base64");
    await writeFile(join(root, "updater-manifest.sig.json"), JSON.stringify({ schema: "buglogin.updater-signature.v1", algorithm: "ed25519", manifestSha256: sha256(manifestBytes), publicKeyBase64, signatureBase64 }));
    await writeFile(join(root, "provenance.json"), JSON.stringify({ schema: "buglogin.release-provenance.v1", source: { sha: "0".repeat(40) }, materials: [{ name: "Cargo.lock", sha256: "0".repeat(64) }, { name: "pnpm-lock.yaml", sha256: "0".repeat(64) }, { name: ".github/workflows/release.yml", sha256: "0".repeat(64) }, { name: ".github/workflows/signing.yml", sha256: "0".repeat(64) }], subjects: [{ name: "BugLogin.exe", sha256: sha256(Buffer.from("current")) }, { name: "dist.spdx.json", sha256: sha256(await readFile(join(root, "dist.spdx.json"))) }, { name: "updater-manifest.json", sha256: sha256(manifestBytes) }] }));
    const checksumLines = await Promise.all(["BugLogin.exe", "dist.spdx.json", "updater-manifest.json"].map(async (name) => `${sha256(await readFile(join(root, name)))}  ${name}`));
    await writeFile(join(root, "SHA256SUMS.txt"), `${checksumLines.join("\n")}\n`);
    await verifyBundle(root, { requireUpdater: true });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("release bundle rejects rollback metadata tampering", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-release-bundle-tamper-"));
  try {
    await writeFile(join(root, "dist.spdx.json"), JSON.stringify({ spdxVersion: "SPDX-2.3", SPDXID: "SPDXRef-DOCUMENT", packages: [{ SPDXID: "SPDXRef-Package-1", name: "BugLogin", versionInfo: "1.2.0", externalRefs: [{ referenceType: "purl" }] }] }));
    await writeFile(join(root, "provenance.json"), JSON.stringify({ schema: "buglogin.release-provenance.v1", source: { sha: "0".repeat(40) }, materials: [{ name: "Cargo.lock", sha256: "0".repeat(64) }, { name: "pnpm-lock.yaml", sha256: "0".repeat(64) }, { name: ".github/workflows/release.yml", sha256: "0".repeat(64) }, { name: ".github/workflows/signing.yml", sha256: "0".repeat(64) }], subjects: [{ name: "dist.spdx.json", sha256: sha256(await readFile(join(root, "dist.spdx.json"))) }] }));
    await writeFile(join(root, "SHA256SUMS.txt"), `${sha256(await readFile(join(root, "dist.spdx.json")))}  dist.spdx.json\n`);
    await assert.rejects(() => verifyBundle(root, { requireUpdater: true }), /signed updater manifest/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("release bundle rejects byte changes hidden behind unchanged metadata", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-release-bundle-bytes-"));
  try {
    await writeFile(join(root, "dist.spdx.json"), JSON.stringify({ spdxVersion: "SPDX-2.3", SPDXID: "SPDXRef-DOCUMENT", packages: [{ SPDXID: "SPDXRef-Package-1", name: "BugLogin", versionInfo: "1.2.0", externalRefs: [{ referenceType: "purl" }] }] }));
    const sbomHash = sha256(await readFile(join(root, "dist.spdx.json")));
    await writeFile(join(root, "provenance.json"), JSON.stringify({ schema: "buglogin.release-provenance.v1", source: { sha: "0".repeat(40) }, materials: [{ name: "Cargo.lock", sha256: "0".repeat(64) }, { name: "pnpm-lock.yaml", sha256: "0".repeat(64) }, { name: ".github/workflows/release.yml", sha256: "0".repeat(64) }, { name: ".github/workflows/signing.yml", sha256: "0".repeat(64) }], subjects: [{ name: "dist.spdx.json", sha256: sbomHash }] }));
    await writeFile(join(root, "SHA256SUMS.txt"), `${"0".repeat(64)}  dist.spdx.json\n`);
    await assert.rejects(() => verifyBundle(root), /checksum mismatch/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
