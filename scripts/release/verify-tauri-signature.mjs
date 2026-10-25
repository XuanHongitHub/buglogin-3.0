import {
  createHash,
  createPublicKey,
  verify as verifySignature,
} from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

if (
  process.argv[1] &&
  resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))
) {
  const [artifactPath, signaturePath] = process.argv.slice(2);
  if (!artifactPath || !signaturePath) {
    throw new Error(
      "usage: verify-tauri-signature.mjs <artifact> <tauri-signature-file>",
    );
  }

  const publicKeyBase64 = process.env.BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY?.trim();
  if (!publicKeyBase64) {
    throw new Error("BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY is required");
  }

  const artifact = await readFile(artifactPath);
  const signatureFile = await readFile(signaturePath, "utf8");
  verifyTauriSignature(artifact, signatureFile, publicKeyBase64);
  console.log(`[tauri-signature] verified ${artifactPath}`);
}

export function verifyTauriSignature(artifact, signatureFile, publicKeyBase64) {
  const publicArmor = decodeBase64Text(publicKeyBase64, "public updater key");
  const publicLines = publicArmor.trim().split(/\r?\n/);
  if (
    !publicLines[0].startsWith("untrusted comment: minisign public key:") ||
    publicLines.length !== 2
  ) {
    throw new Error("public updater key is not valid minisign armor");
  }
  const publicPacket = decodeBase64(publicLines[1], "public key packet");
  if (
    publicPacket.length !== 42 ||
    publicPacket.subarray(0, 2).toString("ascii") !== "Ed"
  ) {
    throw new Error(
      "public updater key uses an unsupported minisign algorithm",
    );
  }

  const signatureArmor = decodeBase64Text(
    signatureFile.trim(),
    "signature file",
  );
  const signatureLines = signatureArmor.trimEnd().split(/\r?\n/);
  if (
    signatureLines.length !== 4 ||
    !signatureLines[0].startsWith("untrusted comment: signature from ") ||
    !signatureLines[2].startsWith("trusted comment: ")
  ) {
    throw new Error("Tauri signature is not valid minisign armor");
  }
  const signaturePacket = decodeBase64(signatureLines[1], "signature packet");
  const globalSignature = decodeBase64(
    signatureLines[3],
    "global signature packet",
  );
  if (
    signaturePacket.length !== 74 ||
    !["Ed", "ED"].includes(signaturePacket.subarray(0, 2).toString("ascii")) ||
    globalSignature.length !== 64
  ) {
    throw new Error("Tauri signature uses an unsupported minisign algorithm");
  }
  if (!publicPacket.subarray(2, 10).equals(signaturePacket.subarray(2, 10))) {
    throw new Error(
      "Tauri signature key ID does not match the configured public key",
    );
  }

  const publicKey = createPublicKey({
    key: Buffer.concat([
      Buffer.from("302a300506032b6570032100", "hex"),
      publicPacket.subarray(10),
    ]),
    format: "der",
    type: "spki",
  });
  const message =
    signaturePacket.subarray(0, 2).toString("ascii") === "ED"
      ? createHash("blake2b512").update(artifact).digest()
      : artifact;
  if (
    !verifySignature(null, message, publicKey, signaturePacket.subarray(10))
  ) {
    throw new Error("Tauri updater signature verification failed");
  }
  const trustedComment = signatureLines[2].slice("trusted comment: ".length);
  const globalMessage = Buffer.concat([
    signaturePacket.subarray(10),
    Buffer.from(trustedComment),
  ]);
  if (!verifySignature(null, globalMessage, publicKey, globalSignature)) {
    throw new Error(
      "Tauri updater trusted comment signature verification failed",
    );
  }
}

function decodeBase64Text(value, label) {
  const decoded = decodeBase64(value, label);
  return decoded.toString("utf8");
}

function decodeBase64(value, label) {
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(value)) {
    throw new Error(`${label} is not valid base64`);
  }
  const decoded = Buffer.from(value, "base64");
  if (decoded.length === 0 || decoded.toString("base64") !== value) {
    throw new Error(`${label} is not canonical base64`);
  }
  return decoded;
}
