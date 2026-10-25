import assert from "node:assert/strict";
import {
  createHash,
  generateKeyPairSync,
  sign as signBytes,
} from "node:crypto";
import test from "node:test";
import { verifyTauriSignature } from "../../scripts/release/verify-tauri-signature.mjs";

test("Tauri minisign verifier accepts a valid updater artifact", () => {
  const fixture = signedFixture();
  assert.doesNotThrow(() =>
    verifyTauriSignature(
      fixture.artifact,
      fixture.signatureFile,
      fixture.publicKeyBase64,
    ),
  );
});

test("Tauri minisign verifier rejects changed artifact bytes", () => {
  const fixture = signedFixture();
  assert.throws(
    () =>
      verifyTauriSignature(
        Buffer.from("tampered updater payload"),
        fixture.signatureFile,
        fixture.publicKeyBase64,
      ),
    /signature verification failed/,
  );
});

test("Tauri minisign verifier rejects a different trusted public key", () => {
  const fixture = signedFixture();
  const { publicKey } = generateKeyPairSync("ed25519");
  const wrongKeyText = minisignPublicKey(publicKey, fixture.keyId);
  assert.throws(
    () =>
      verifyTauriSignature(
        fixture.artifact,
        fixture.signatureFile,
        Buffer.from(wrongKeyText).toString("base64"),
      ),
    /signature verification failed/,
  );
});

test("Tauri minisign verifier rejects a mismatched key ID", () => {
  const fixture = signedFixture();
  const { publicKey } = generateKeyPairSync("ed25519");
  const wrongKeyText = minisignPublicKey(publicKey, Buffer.from("WRONGKEY"));
  assert.throws(
    () =>
      verifyTauriSignature(
        fixture.artifact,
        fixture.signatureFile,
        Buffer.from(wrongKeyText).toString("base64"),
      ),
    /key ID does not match/,
  );
});

test("Tauri minisign verifier rejects malformed signature data", () => {
  const fixture = signedFixture();
  assert.throws(
    () =>
      verifyTauriSignature(
        fixture.artifact,
        "not-base64",
        fixture.publicKeyBase64,
      ),
    /not valid base64/,
  );
});

test("Tauri minisign verifier rejects a changed trusted comment", () => {
  const fixture = signedFixture();
  const armor = Buffer.from(fixture.signatureFile, "base64")
    .toString("utf8")
    .split("\n");
  armor[2] = `${armor[2]} modified`;
  assert.throws(
    () =>
      verifyTauriSignature(
        fixture.artifact,
        Buffer.from(armor.join("\n")).toString("base64"),
        fixture.publicKeyBase64,
      ),
    /trusted comment signature verification failed/,
  );
});

function signedFixture() {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const keyId = Buffer.from("BL308001");
  const artifact = Buffer.from("synthetic signed updater payload");
  const publicKeyText = minisignPublicKey(publicKey, keyId);
  const publicKeyBase64 = Buffer.from(publicKeyText).toString("base64");
  const digest = createHash("blake2b512").update(artifact).digest();
  const signature = signBytes(null, digest, privateKey);
  const signaturePacket = Buffer.concat([Buffer.from("ED"), keyId, signature]);
  const trustedComment =
    "timestamp:1792869636\tfile:synthetic.app.tar.gz\tversion:3.0.8";
  const globalSignature = signBytes(
    null,
    Buffer.concat([signature, Buffer.from(trustedComment)]),
    privateKey,
  );
  const armor = [
    "untrusted comment: signature from minisign secret key",
    signaturePacket.toString("base64"),
    `trusted comment: ${trustedComment}`,
    globalSignature.toString("base64"),
  ].join("\n");
  return {
    artifact,
    keyId,
    publicKeyBase64,
    signatureFile: Buffer.from(armor).toString("base64"),
  };
}

function minisignPublicKey(publicKey, keyId) {
  const der = publicKey.export({ format: "der", type: "spki" });
  const raw = der.subarray(der.length - 32);
  return [
    "untrusted comment: minisign public key:",
    Buffer.concat([Buffer.from("Ed"), keyId, raw]).toString("base64"),
  ].join("\n");
}
