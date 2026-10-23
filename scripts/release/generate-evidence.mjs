import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

const [directoryArg, platform, sourceRef, sourceSha, target] = process.argv.slice(2);
if (![directoryArg, platform, sourceRef, sourceSha, target].every(Boolean)) throw new Error("usage: generate-evidence.mjs <directory> <platform> <source-ref> <source-sha> <target>");
if (!/^[0-9a-f]{40}$/.test(sourceSha)) throw new Error("source SHA must be a full immutable commit SHA");
const directory = resolve(directoryArg);
const names = (await readdir(directory)).filter((name) => !["SHA256SUMS.txt", "provenance.json"].includes(name)).sort();
if (names.length < 2 || !names.includes("dist.spdx.json")) throw new Error("signed artifact and SPDX SBOM are required before evidence generation");
const subjects = [];
for (const name of names) {
  const contents = await readFile(join(directory, name));
  subjects.push({ name, sha256: createHash("sha256").update(contents).digest("hex") });
}
const materials = [];
for (const name of ["Cargo.lock", "pnpm-lock.yaml", ".github/workflows/release.yml", ".github/workflows/signing.yml"]) {
  const contents = await readFile(name);
  materials.push({ name, sha256: createHash("sha256").update(contents).digest("hex") });
}
await writeFile(join(directory, "SHA256SUMS.txt"), `${subjects.map(({ name, sha256 }) => `${sha256}  ${name}`).join("\n")}\n`, "utf8");
await writeFile(join(directory, "provenance.json"), `${JSON.stringify({
  schema: "buglogin.release-provenance.v1",
  source: { repository: process.env.GITHUB_REPOSITORY ?? "local", ref: sourceRef, sha: sourceSha },
  builder: { workflow: process.env.GITHUB_WORKFLOW_REF ?? "local", runId: process.env.GITHUB_RUN_ID ?? "local", runner: process.env.RUNNER_NAME ?? "local" },
  toolchains: { rust: "1.95.0", node: "24.18.0", pnpm: "11.10.0", target },
  platform,
  materials,
  subjects,
}, null, 2)}\n`, "utf8");
console.log(`[release-evidence] wrote ${subjects.length} hashes and provenance for ${basename(directory)}`);
