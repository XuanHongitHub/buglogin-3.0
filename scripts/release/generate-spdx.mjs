import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import YAML from "yaml";

const output = resolve(process.argv[2] ?? "dist-release/dist.spdx.json");
const rootPackage = JSON.parse(await readFile("package.json", "utf8"));
const cargoLock = await readFile("Cargo.lock", "utf8");
const pnpmLock = YAML.parse(await readFile("pnpm-lock.yaml", "utf8"));
const packages = new Map();

for (const block of cargoLock.matchAll(/\[\[package\]\]\s+name = "([^"]+)"\s+version = "([^"]+)"/g)) addPackage(`cargo:${block[1]}@${block[2]}`, block[1], block[2], "cargo");
for (const key of Object.keys(pnpmLock.snapshots ?? pnpmLock.packages ?? {})) {
  const clean = key.replace(/^\//, "").split("(")[0];
  const separator = clean.lastIndexOf("@");
  if (separator > 0) addPackage(`npm:${clean}`, clean.slice(0, separator), clean.slice(separator + 1), "npm");
}
addPackage(`npm:${rootPackage.name}@${rootPackage.version}`, rootPackage.name, rootPackage.version, "npm");

const digest = createHash("sha256").update([...packages.keys()].sort().join("\n")).digest("hex");
const document = {
  spdxVersion: "SPDX-2.3",
  dataLicense: "CC0-1.0",
  SPDXID: "SPDXRef-DOCUMENT",
  name: `BugLogin-${rootPackage.version}`,
  documentNamespace: `https://buglogin.invalid/spdx/${rootPackage.version}/${digest}`,
  creationInfo: { created: new Date().toISOString(), creators: ["Tool: scripts/release/generate-spdx.mjs"] },
  packages: [...packages.values()].sort((left, right) => left.name.localeCompare(right.name)),
};
await writeFile(output, `${JSON.stringify(document, null, 2)}\n`, "utf8");
console.log(`[release-sbom] wrote ${document.packages.length} packages to ${output}`);

function addPackage(key, name, version, type) {
  if (packages.has(key)) return;
  const id = createHash("sha256").update(key).digest("hex").slice(0, 20);
  packages.set(key, {
    SPDXID: `SPDXRef-Package-${id}`,
    name,
    versionInfo: version,
    downloadLocation: "NOASSERTION",
    filesAnalyzed: false,
    licenseConcluded: "NOASSERTION",
    licenseDeclared: "NOASSERTION",
    copyrightText: "NOASSERTION",
    externalRefs: [{ referenceCategory: "PACKAGE-MANAGER", referenceType: "purl", referenceLocator: `pkg:${type}/${encodeURIComponent(name)}@${version}` }],
  });
}
