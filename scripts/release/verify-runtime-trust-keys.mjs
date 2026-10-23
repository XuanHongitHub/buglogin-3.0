import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const path = resolve("apps/desktop/src-tauri/runtime-trust-keys.json");

try {
  const raw = await readFile(path, "utf8");
  const keys = JSON.parse(raw);
  if (!keys || Array.isArray(keys) || typeof keys !== "object" || Object.keys(keys).length === 0) {
    throw new Error("expected a non-empty key ID to public-key map");
  }
  for (const [id, key] of Object.entries(keys)) {
    if (!id.trim() || typeof key !== "string" || !/^[0-9a-fA-F]{64}$/.test(key)) {
      throw new Error("each key ID must be non-empty and each Ed25519 public key must be 32-byte hex");
    }
  }
  console.log(`[runtime-trust] PASS (${Object.keys(keys).length} embedded public keys)`);
} catch (error) {
  console.error(`[runtime-trust] FAIL: ${error.message}`);
  process.exitCode = 1;
}
