import { writeFile } from "node:fs/promises";
import { resolve } from "node:path";

const output = process.argv[2];
if (!output) throw new Error("usage: create-tauri-updater-config.mjs <output-json>");

const publicKey = process.env.BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY?.trim();
const signingKey = process.env.TAURI_SIGNING_PRIVATE_KEY?.trim();
const endpoint = process.env.BUGLOGIN_TAURI_UPDATER_ENDPOINT?.trim()
  || "https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/v3/stable/latest.json";

let parsedPublicKey;
try {
  parsedPublicKey = Buffer.from(publicKey ?? "", "base64").toString("utf8");
} catch {
  parsedPublicKey = "";
}
if (
  !publicKey ||
  !/^[A-Za-z0-9+/=\r\n]+$/.test(publicKey) ||
  !parsedPublicKey.startsWith("untrusted comment: minisign public key:") ||
  !/^RW[A-Za-z0-9+/=]+$/m.test(parsedPublicKey)
) {
  throw new Error("BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY is missing or invalid");
}
if (!signingKey) throw new Error("TAURI_SIGNING_PRIVATE_KEY is required for release artifacts");
const parsedEndpoint = new URL(endpoint);
if (parsedEndpoint.protocol !== "https:" || parsedEndpoint.username || parsedEndpoint.password) {
  throw new Error("updater endpoint must be an HTTPS URL without credentials");
}

const config = {
  bundle: { createUpdaterArtifacts: true },
  plugins: {
    updater: {
      pubkey: publicKey,
      endpoints: [parsedEndpoint.toString()],
      requireSignedVersion: true,
      allowDowngrades: false,
    },
  },
};
await writeFile(resolve(output), `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600 });
console.log("Tauri updater release configuration created");
