import { createHash, createPublicKey, verify as verifySignature } from "node:crypto";
import { readdir, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const directory = resolve(process.argv[2] ?? "dist-release");
const requireUpdater = process.argv.includes("--require-updater");

if (process.argv[2] === "--help") {
  console.log("usage: verify-bundle.mjs <dist-release> [--require-updater]");
} else if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))) {
  await verifyBundle(directory, { requireUpdater });
  console.log(`[release-bundle] verified ${directory}`);
}

export async function verifyBundle(root, { requireUpdater: requireUpdaterMetadata = false } = {}) {
  const names = new Set(await readdir(root));
  for (const required of ["dist.spdx.json", "SHA256SUMS.txt", "provenance.json"]) {
    if (!names.has(required)) throw new Error(`[release-bundle] missing ${required}`);
  }

  const sbom = JSON.parse(await readFile(join(root, "dist.spdx.json"), "utf8"));
  verifySpdx(sbom);
  const provenance = JSON.parse(await readFile(join(root, "provenance.json"), "utf8"));
  const sums = parseChecksums(await readFile(join(root, "SHA256SUMS.txt"), "utf8"));
  for (const [name, expected] of sums) {
    const actual = sha256(await readFile(join(root, name)));
    if (actual !== expected) throw new Error(`[release-bundle] checksum mismatch: ${name}`);
  }
  verifyProvenance(provenance, sums);

  const updater = names.has("updater-manifest.json");
  const updaterSignature = names.has("updater-manifest.sig.json");
  if (requireUpdaterMetadata && (!updater || !updaterSignature)) {
    throw new Error("[release-bundle] signed updater manifest and rollback metadata are required");
  }
  if (updater || updaterSignature) {
    if (!updater || !updaterSignature) throw new Error("[release-bundle] updater manifest and detached signature must be paired");
    await verifyUpdater(root, sums);
  }

  const ignored = new Set(["SHA256SUMS.txt", "provenance.json", "updater-manifest.sig.json"]);
  const subjectNames = [...names].filter((name) => !ignored.has(name)).sort();
  for (const name of subjectNames) {
    if (!sums.has(name)) throw new Error(`[release-bundle] unsigned bundle file: ${name}`);
    const actual = sha256(await readFile(join(root, name)));
    if (actual !== sums.get(name)) throw new Error(`[release-bundle] checksum mismatch: ${name}`);
  }
  for (const name of sums.keys()) {
    if (!names.has(name)) throw new Error(`[release-bundle] checksum references missing file: ${name}`);
  }
  return { sbom, provenance, sums };
}

function verifySpdx(value) {
  if (value.spdxVersion !== "SPDX-2.3" || value.SPDXID !== "SPDXRef-DOCUMENT" || !Array.isArray(value.packages) || value.packages.length === 0) {
    throw new Error("[release-bundle] invalid SPDX-2.3 SBOM");
  }
  const ids = new Set();
  for (const item of value.packages) {
    if (!item.SPDXID || ids.has(item.SPDXID) || !item.name || !item.versionInfo || !Array.isArray(item.externalRefs) || !item.externalRefs.some((ref) => ref.referenceType === "purl")) {
      throw new Error("[release-bundle] SBOM package is incomplete");
    }
    ids.add(item.SPDXID);
  }
}

function parseChecksums(text) {
  const result = new Map();
  for (const line of text.trim().split(/\r?\n/)) {
    const match = line.match(/^([0-9a-f]{64})  (\S+)$/);
    if (!match || result.has(match[2])) throw new Error("[release-bundle] invalid SHA256SUMS.txt");
    result.set(match[2], match[1]);
  }
  if (result.size === 0) throw new Error("[release-bundle] empty SHA256SUMS.txt");
  return result;
}

function verifyProvenance(value, sums) {
  if (value.schema !== "buglogin.release-provenance.v1" || !/^[0-9a-f]{40}$/.test(value.source?.sha ?? "") || !Array.isArray(value.materials) || !Array.isArray(value.subjects)) {
    throw new Error("[release-bundle] invalid provenance envelope");
  }
  const expectedMaterials = ["Cargo.lock", "pnpm-lock.yaml", ".github/workflows/release.yml", ".github/workflows/signing.yml"];
  if (value.materials.map((item) => item.name).join("|") !== expectedMaterials.join("|")) throw new Error("[release-bundle] provenance materials are incomplete or reordered");
  for (const item of value.materials) if (!/^[0-9a-f]{64}$/.test(item.sha256 ?? "")) throw new Error("[release-bundle] provenance material hash is invalid");
  for (const item of value.subjects) {
    if (!sums.has(item.name) || item.sha256 !== sums.get(item.name)) throw new Error(`[release-bundle] provenance subject mismatch: ${item.name}`);
  }
}

async function verifyUpdater(root, sums) {
  const manifestBytes = await readFile(join(root, "updater-manifest.json"));
  const manifest = JSON.parse(manifestBytes);
  const detached = JSON.parse(await readFile(join(root, "updater-manifest.sig.json"), "utf8"));
  if (manifest.schema !== "buglogin.updater-manifest.v1" || !/^(dev|nightly|rc|stable)$/.test(manifest.channel) || !semver(manifest.version) || !Array.isArray(manifest.artifacts)) throw new Error("[release-bundle] invalid updater manifest");
  if (manifest.rollback?.allowed !== true || manifest.rollback?.dataCompatibility !== "backward-compatible" || !semver(manifest.rollback.previousVersion) || compareSemver(manifest.rollback.previousVersion, manifest.version) >= 0 || !Array.isArray(manifest.rollback.previousArtifacts)) {
    throw new Error("[release-bundle] updater rollback contract is unsafe or incomplete");
  }
  verifyArtifacts(manifest.artifacts, manifest.version, "current");
  verifyArtifacts(manifest.rollback.previousArtifacts, manifest.rollback.previousVersion, "previous");
  const currentTargets = new Set(manifest.artifacts.map((item) => `${item.platform}/${item.arch}`));
  const previousTargets = new Set(manifest.rollback.previousArtifacts.map((item) => `${item.platform}/${item.arch}`));
  if (currentTargets.size === 0 || currentTargets.size !== previousTargets.size || [...currentTargets].some((target) => !previousTargets.has(target))) throw new Error("[release-bundle] rollback target coverage is incomplete");
  if (detached.schema !== "buglogin.updater-signature.v1" || detached.algorithm !== "ed25519" || detached.manifestSha256 !== sha256(manifestBytes) || !detached.publicKeyBase64 || !detached.signatureBase64) throw new Error("[release-bundle] invalid updater signature envelope");
  const publicKey = createPublicKey({ key: Buffer.from(detached.publicKeyBase64, "base64"), format: "der", type: "spki" });
  if (!verifySignature(null, manifestBytes, publicKey, Buffer.from(detached.signatureBase64, "base64"))) throw new Error("[release-bundle] updater manifest signature verification failed");
  if (sums.has("updater-manifest.json") && sums.get("updater-manifest.json") !== sha256(manifestBytes)) throw new Error("[release-bundle] updater manifest checksum mismatch");
}

function verifyArtifacts(items, version, label) {
  for (const item of items) {
    if (!item.platform || !item.arch || item.version !== version || !/^https:\/\//.test(item.url ?? "") || !/^[0-9a-f]{64}$/.test(item.sha256 ?? "") || !item.signature) throw new Error(`[release-bundle] invalid ${label} updater artifact`);
  }
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function semver(value) {
  return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(value ?? "");
}

function compareSemver(left, right) {
  const parse = (value) => value.split("-")[0].split(".").map(Number);
  const a = parse(left);
  const b = parse(right);
  for (let index = 0; index < 3; index += 1) if (a[index] !== b[index]) return a[index] - b[index];
  return 0;
}
