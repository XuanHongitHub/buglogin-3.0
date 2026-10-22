import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const files = [
  ".github/workflows/ci.yml",
  ".github/workflows/contracts.yml",
  ".github/actions/setup-buglogin/action.yml",
];
const forbidden =
  /(?:TAURI_SIGNING|APPLE_ID|APPLE_PASSWORD|APPLE_CERTIFICATE|CSC_LINK|CSC_KEY_PASSWORD|AZURE_.*SECRET|AWS_SECRET_ACCESS_KEY|GITHUB_TOKEN|secrets\.)/i;
for (const file of files) {
  const text = await readFile(resolve(root, file), "utf8");
  if (forbidden.test(text))
    throw new Error(`[ci-secrets] forbidden secret reference in ${file}`);
}
console.log(
  `[ci-secrets] PASS: ${files.length} PR-safe CI files contain no signing/runtime secret references`,
);
