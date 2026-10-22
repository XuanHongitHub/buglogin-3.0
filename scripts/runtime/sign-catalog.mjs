import {
  createHash,
  createPrivateKey,
  createPublicKey,
  sign,
  verify,
} from "node:crypto";
import { createReadStream } from "node:fs";
import { lstat, mkdir, open, readFile, rm } from "node:fs/promises";
import { dirname, isAbsolute, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const MAX_PACKAGE_BYTES = 4 * 1024 * 1024 * 1024;
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");
const SIGNING_PREFIX = Buffer.from("buglogin.runtime-release.v2\0", "utf8");

function fail(message) {
  throw new Error(message);
}

function field(target, value) {
  const bytes = Buffer.from(value, "utf8");
  const length = Buffer.alloc(8);
  length.writeBigUInt64BE(BigInt(bytes.length));
  target.push(length, bytes);
}

export function runtimeReleasePayload(release) {
  const chunks = [SIGNING_PREFIX];
  for (const value of [
    release.runtime,
    release.version,
    release.platform,
    release.architecture,
    release.relativePath,
    release.packageFormat,
    release.archivePrefix ?? "",
    release.source,
    release.sha256.toLowerCase(),
    release.keyId,
  ]) {
    field(chunks, value);
  }
  const size = Buffer.alloc(8);
  size.writeBigUInt64BE(BigInt(release.sizeBytes));
  chunks.push(size);
  return Buffer.concat(chunks);
}

function safeRelativePath(value) {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    !value.includes("\\") &&
    !value.includes(":") &&
    !value.startsWith("/") &&
    value
      .split("/")
      .every((part) => part !== "" && part !== "." && part !== "..")
  );
}

async function regularFile(path, label, maxBytes = Number.MAX_SAFE_INTEGER) {
  if (!isAbsolute(path)) fail(`${label} path must be absolute`);
  const metadata = await lstat(path).catch(() =>
    fail(`${label} cannot be read`),
  );
  if (
    !metadata.isFile() ||
    metadata.isSymbolicLink() ||
    metadata.size > maxBytes
  ) {
    fail(`${label} must be a regular file within the supported size limit`);
  }
  return metadata;
}

async function sha256File(path) {
  const digest = createHash("sha256");
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest("hex");
}

function rawPublicKey(publicKey) {
  const der = publicKey.export({ type: "spki", format: "der" });
  if (
    der.length !== ED25519_SPKI_PREFIX.length + 32 ||
    !der.subarray(0, ED25519_SPKI_PREFIX.length).equals(ED25519_SPKI_PREFIX)
  ) {
    fail("signing key must be Ed25519");
  }
  return der.subarray(ED25519_SPKI_PREFIX.length);
}

function publicKeyFromHex(value) {
  if (typeof value !== "string" || !/^[0-9a-f]{64}$/i.test(value)) {
    fail("existing trust key is invalid");
  }
  return createPublicKey({
    key: Buffer.concat([ED25519_SPKI_PREFIX, Buffer.from(value, "hex")]),
    type: "spki",
    format: "der",
  });
}

async function readExisting(path, label, maxBytes = 2 * 1024 * 1024) {
  if (!path) return undefined;
  const filePath = resolve(path);
  await regularFile(filePath, label, maxBytes);
  try {
    return JSON.parse(await readFile(filePath, "utf8"));
  } catch {
    fail(`${label} is not valid JSON`);
  }
}

function validateExistingCatalog(catalog, trustedKeys) {
  if (catalog.schemaVersion !== 2 || !Array.isArray(catalog.releases)) {
    fail("existing catalog must use schemaVersion 2 and contain releases");
  }
  const identities = new Set();
  for (const release of catalog.releases) {
    const identity = [
      release.runtime,
      release.version,
      release.platform,
      release.architecture,
    ].join("\0");
    if (identities.has(identity))
      fail("existing catalog contains duplicate release identities");
    identities.add(identity);
    const publicKey = trustedKeys.get(release.keyId);
    if (
      !publicKey ||
      typeof release.signature !== "string" ||
      !/^[0-9a-f]{128}$/i.test(release.signature) ||
      !verify(
        null,
        runtimeReleasePayload(release),
        publicKey,
        Buffer.from(release.signature, "hex"),
      )
    ) {
      fail(
        `existing release signature is invalid: ${identity.replaceAll("\0", "/")}`,
      );
    }
  }
}

export async function buildSignedRuntimeCatalog(options) {
  const {
    artifactPath,
    privateKeyPath,
    keyId,
    runtime,
    version,
    platform,
    architecture,
    relativePath,
    source,
    packageFormat = "raw",
    archivePrefix,
    existingCatalogPath,
    existingTrustKeysPath,
  } = options;

  const artifact = resolve(artifactPath ?? "");
  const privateKeyFile = resolve(privateKeyPath ?? "");
  const artifactMetadata = await regularFile(
    artifact,
    "artifact",
    MAX_PACKAGE_BYTES,
  );
  if (artifactMetadata.size === 0) fail("artifact cannot be empty");
  await regularFile(privateKeyFile, "private signing key", 64 * 1024);
  if (!keyId?.trim() || keyId.length > 128)
    fail("key ID is required and must be at most 128 characters");
  if (!new Set(["wayfern", "bugmini"]).has(runtime))
    fail("runtime must be wayfern or bugmini");
  if (!new Set(["windows", "macos", "linux"]).has(platform))
    fail("platform is unsupported");
  if (!new Set(["x64", "arm64"]).has(architecture))
    fail("architecture is unsupported");
  if (!version?.trim() || version.length > 128)
    fail("runtime version is required and must be at most 128 characters");
  if (!safeRelativePath(relativePath))
    fail("runtime executable path is unsafe");
  if (!new Set(["raw", "zip", "dmg"]).has(packageFormat))
    fail("package format must be raw, zip, or dmg");
  if (
    packageFormat === "dmg" &&
    (runtime !== "wayfern" ||
      platform !== "macos" ||
      !relativePath.split("/")[0]?.endsWith(".app"))
  ) {
    fail("DMG packages require a macOS Wayfern executable inside an .app bundle");
  }
  if (
    archivePrefix !== undefined &&
    (!safeRelativePath(archivePrefix) || packageFormat !== "zip")
  ) {
    fail(
      "archive prefix is allowed only for ZIP packages and must be a safe relative path",
    );
  }
  let sourceUrl;
  try {
    sourceUrl = new URL(source);
  } catch {
    fail("source must be an HTTPS URL");
  }
  if (
    sourceUrl.protocol !== "https:" ||
    sourceUrl.username ||
    sourceUrl.password ||
    sourceUrl.hash
  ) {
    fail("source must be an HTTPS URL without credentials or a fragment");
  }

  let privateKey;
  try {
    privateKey = createPrivateKey(await readFile(privateKeyFile));
  } catch {
    fail("private signing key cannot be loaded");
  }
  if (privateKey.asymmetricKeyType !== "ed25519")
    fail("signing key must be Ed25519");
  const publicKey = createPublicKey(privateKey);
  const publicKeyHex = rawPublicKey(publicKey).toString("hex");
  const trustKeys =
    (await readExisting(
      existingTrustKeysPath,
      "existing trust keys",
      64 * 1024,
    )) ?? [];
  if (!Array.isArray(trustKeys))
    fail("existing trust keys must be a JSON array");
  const keyMap = new Map();
  for (const entry of trustKeys) {
    if (
      !entry ||
      Object.keys(entry).some((key) => !["keyId", "publicKey"].includes(key)) ||
      typeof entry.keyId !== "string" ||
      !entry.keyId.trim() ||
      entry.keyId.length > 128 ||
      keyMap.has(entry.keyId)
    ) {
      fail("existing trust keys contain an invalid or duplicate key ID");
    }
    keyMap.set(entry.keyId, publicKeyFromHex(entry.publicKey));
  }
  const existingKey = trustKeys.find((entry) => entry.keyId === keyId);
  if (existingKey && existingKey.publicKey.toLowerCase() !== publicKeyHex) {
    fail("key ID already exists with a different public key");
  }
  if (!existingKey) {
    trustKeys.push({ keyId, publicKey: publicKeyHex });
    keyMap.set(keyId, publicKey);
  }

  const releases = (await readExisting(
    existingCatalogPath,
    "existing runtime catalog",
  )) ?? {
    schemaVersion: 2,
    releases: [],
  };
  validateExistingCatalog(releases, keyMap);
  const release = {
    runtime,
    version,
    platform,
    architecture,
    relativePath,
    packageFormat,
    ...(archivePrefix === undefined ? {} : { archivePrefix }),
    source: sourceUrl.href,
    sha256: await sha256File(artifact),
    sizeBytes: artifactMetadata.size,
    keyId,
    signature: "",
  };
  release.signature = sign(
    null,
    runtimeReleasePayload(release),
    privateKey,
  ).toString("hex");

  const identity = [runtime, version, platform, architecture].join("\0");
  releases.releases = releases.releases.filter(
    (entry) =>
      [entry.runtime, entry.version, entry.platform, entry.architecture].join(
        "\0",
      ) !== identity,
  );
  releases.releases.push(release);
  releases.releases.sort((left, right) =>
    [left.runtime, left.platform, left.architecture, left.version]
      .join("\0")
      .localeCompare(
        [right.runtime, right.platform, right.architecture, right.version].join(
          "\0",
        ),
      ),
  );
  return { catalog: releases, trustKeys, release };
}

async function writeExclusive(path, value) {
  const target = resolve(path);
  await mkdir(dirname(target), { recursive: true });
  const handle = await open(target, "wx", 0o600).catch(() =>
    fail(`output already exists or cannot be created: ${target}`),
  );
  try {
    await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`, "utf8");
    await handle.sync();
  } catch (error) {
    await handle.close();
    await rm(target, { force: true });
    throw error;
  } finally {
    await handle.close().catch(() => {});
  }
  return target;
}

function parseArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (
      !flag.startsWith("--") ||
      index + 1 >= argv.length ||
      argv[index + 1].startsWith("--")
    ) {
      fail("invalid arguments; run with --help for usage");
    }
    const key = flag.slice(2);
    if (values.has(key)) fail(`duplicate argument: ${flag}`);
    values.set(key, argv[++index]);
  }
  return values;
}

export async function main(argv = process.argv.slice(2)) {
  if (argv.length === 1 && argv[0] === "--help") {
    console.log(
      "Usage: node scripts/runtime/sign-catalog.mjs --artifact FILE --private-key KEY.pem --key-id ID --runtime wayfern|bugmini --version VERSION --platform windows|macos|linux --architecture x64|arm64 --relative-path PATH --source HTTPS_URL --output catalog.json --trust-output trust-keys.json [--package-format raw|zip|dmg] [--archive-prefix PATH] [--existing-catalog FILE] [--existing-trust-keys FILE]",
    );
    return;
  }
  const args = parseArguments(argv);
  const required = [
    "artifact",
    "private-key",
    "key-id",
    "runtime",
    "version",
    "platform",
    "architecture",
    "relative-path",
    "source",
    "output",
    "trust-output",
  ];
  for (const key of required)
    if (!args.has(key)) fail(`missing required argument: --${key}`);
  const output = resolve(args.get("output"));
  const trustOutput = resolve(args.get("trust-output"));
  if (output === trustOutput)
    fail("catalog and trust output paths must be different");
  const result = await buildSignedRuntimeCatalog({
    artifactPath: args.get("artifact"),
    privateKeyPath: args.get("private-key"),
    keyId: args.get("key-id"),
    runtime: args.get("runtime"),
    version: args.get("version"),
    platform: args.get("platform"),
    architecture: args.get("architecture"),
    relativePath: args.get("relative-path"),
    source: args.get("source"),
    packageFormat: args.get("package-format") ?? "raw",
    archivePrefix: args.get("archive-prefix"),
    existingCatalogPath: args.get("existing-catalog"),
    existingTrustKeysPath: args.get("existing-trust-keys"),
  });
  let catalogWritten;
  try {
    catalogWritten = await writeExclusive(output, result.catalog);
    await writeExclusive(trustOutput, result.trustKeys);
  } catch (error) {
    if (catalogWritten) await rm(catalogWritten, { force: true });
    throw error;
  }
  console.log(
    `Signed ${result.release.runtime} ${result.release.version} ${result.release.platform}/${result.release.architecture}; sha256=${result.release.sha256}; keyId=${result.release.keyId}`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  main().catch((error) => {
    console.error(`Runtime catalog signing failed: ${error.message}`);
    process.exitCode = 1;
  });
}
