import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import YAML from "yaml";

const root = resolve(import.meta.dirname, "../..");
const read = (path) => readFile(resolve(root, path), "utf8");
const fail = (message) => {
  throw new Error(`[ci-contracts] ${message}`);
};

const json = JSON.parse(await read("docs/contracts/domain-schema-v1.json"));
const jsonV2 = JSON.parse(await read("docs/contracts/domain-schema-v2.json"));
const commandSource = (
  await read("docs/contracts/command-registry-v1.yaml")
).replace(/\r\n/g, "\n");
const commands = YAML.parse(commandSource);
const generatedRust = await read("crates/api/src/commands/generated.rs");
const openapiSource = await read("crates/api/openapi/openapi-v1.yaml");
const openapiMirrorSource = await read("docs/contracts/openapi-v1.yaml");
if (
  openapiMirrorSource.replace(/\r\n/g, "\n") !==
  openapiSource.replace(/\r\n/g, "\n")
) {
  fail(
    "docs/contracts/openapi-v1.yaml must exactly mirror the Rust API OpenAPI contract",
  );
}
const openapi = YAML.parse(openapiSource);
const baseline = YAML.parse(
  await read("docs/architecture/upstream-baseline-manifest.yaml"),
);
const ownership = await read("docs/agents/work-packet-ownership.md");
const registry = await read("docs/capabilities/registry.md");
for (const path of [
  ".github/workflows/ci.yml",
  ".github/workflows/contracts.yml",
  ".github/actions/setup-buglogin/action.yml",
]) {
  YAML.parse(await read(path));
}

if (json.properties?.contractVersion?.const !== "1.0")
  fail("domain schema version drift");
if (jsonV2.properties?.contractVersion?.const !== "2.0")
  fail("domain v2 schema version drift");
for (const entity of [
  "vpn",
  "extension",
  "extensionGroup",
  "workspace",
  "team",
  "member",
  "resource",
  "grant",
]) {
  if (!jsonV2.$defs?.[entity]) fail(`domain v2 lacks ${entity}`);
}
const vpnProperties = jsonV2.$defs.vpn.properties ?? {};
if (
  !vpnProperties.configRef ||
  vpnProperties.configData ||
  vpnProperties.privateKey
) {
  fail("domain v2 VPN must use configRef without inline secrets");
}
const extensionProperties = jsonV2.$defs.extension.properties ?? {};
if (
  !extensionProperties.artifactRef ||
  !extensionProperties.artifactSha256 ||
  extensionProperties.bytes
) {
  fail("domain v2 extension must use verified artifact references");
}
if (commands.registryId !== "buglogin.commands.v1")
  fail("command registry ID drift");
if (!Array.isArray(commands.commands) || commands.commands.length === 0)
  fail("command registry must not be empty");
const commandIds = commands.commands.map((command) => command.id);
if (new Set(commandIds).size !== commandIds.length)
  fail("command IDs must be unique");
const allowedKinds = new Set(["query", "command", "operation", "subscription"]);
const allowedActions = new Set([
  "read",
  "create",
  "update",
  "delete",
  "execute",
  "share",
  "manage",
]);
const allowedAdapters = new Set(["tauri", "rest", "mcp"]);
const commandsById = new Map(
  commands.commands.map((command) => [command.id, command]),
);
for (const command of commands.commands) {
  if (!allowedKinds.has(command.kind)) fail(`invalid kind for ${command.id}`);
  if (!allowedActions.has(command.action))
    fail(`invalid action for ${command.id}`);
  if (
    !Array.isArray(command.adapters) ||
    command.adapters.some((adapter) => !allowedAdapters.has(adapter))
  )
    fail(`invalid adapters for ${command.id}`);
  for (const field of ["id", "capability", "input", "output"]) {
    if (typeof command[field] !== "string" || command[field].length === 0)
      fail(`missing ${field} for ${command.id ?? "unknown command"}`);
  }
  if (
    command.auth === "public" &&
    (command.kind !== "query" || command.action !== "read")
  ) {
    fail(`public command must be a read-only query: ${command.id}`);
  }
  if (
    command.auth === "single-use-capability" &&
    (command.id !== "profile.viewer.ticket.redeem" ||
      command.kind !== "command" ||
      command.action !== "read" ||
      command.adapters.length !== 1 ||
      command.adapters[0] !== "rest")
  ) {
    fail(`single-use capability auth is restricted to profile viewer redemption: ${command.id}`);
  }
}
const sourceHash = createHash("sha256").update(commandSource).digest("hex");
if (
  !new RegExp(`COMMAND_REGISTRY_SOURCE_SHA256: &str =\\s*"${sourceHash}"`).test(
    generatedRust,
  )
)
  fail("generated Rust registry hash drift");
const generatedRustIds = [
  ...generatedRust.matchAll(
    /^ {4}meta\(\r?\n {8}"([^"]+)",\r?\n {8}CommandKind::/gm,
  ),
].map((match) => match[1]);
if (
  generatedRustIds.length !== commandIds.length ||
  generatedRustIds.some((id, index) => id !== commandIds[index])
)
  fail("generated Rust registry command drift");
if (openapi.openapi !== "3.1.0") fail("OpenAPI version drift");
const openapiOperationIds = Object.values(openapi.paths ?? {})
  .flatMap((path) => Object.values(path ?? {}))
  .filter((operation) => operation && typeof operation === "object")
  .map((operation) => operation.operationId)
  .filter((operationId) => typeof operationId === "string");
if (new Set(openapiOperationIds).size !== openapiOperationIds.length)
  fail("OpenAPI operation IDs must be unique");
const restCommandsMissingFromOpenapi = commands.commands
  .filter(
    (command) =>
      command.adapters.includes("rest") &&
      !openapiOperationIds.includes(command.id),
  )
  .map((command) => command.id);
if (restCommandsMissingFromOpenapi.length > 0) {
  fail(
    `REST registry commands missing from OpenAPI: ${restCommandsMissingFromOpenapi.join(", ")}`,
  );
}
const restCommandIds = new Set(
  commands.commands
    .filter((command) => command.adapters.includes("rest"))
    .map((command) => command.id),
);
const openapiOperationsMissingFromRegistry = openapiOperationIds.filter(
  (operationId) => !restCommandIds.has(operationId),
);
if (openapiOperationsMissingFromRegistry.length > 0) {
  fail(
    `OpenAPI operations missing from REST command registry: ${openapiOperationsMissingFromRegistry.join(", ")}`,
  );
}
const runtimeCatalog = commandsById.get("runtime.catalog.get");
if (
  runtimeCatalog?.auth !== "public" ||
  runtimeCatalog.action !== "read" ||
  runtimeCatalog.kind !== "query"
) {
  fail("runtime catalog must be an explicitly public read-only query");
}
if (
  !runtimeCatalog.adapters.includes("rest") ||
  !runtimeCatalog.adapters.includes("mcp")
) {
  fail("runtime catalog must expose the canonical REST and MCP read adapters");
}
const runtimeCatalogOperation = openapi.paths?.["/v1/runtime/catalog"]?.get;
if (
  runtimeCatalogOperation?.operationId !== "runtime.catalog.get" ||
  JSON.stringify(runtimeCatalogOperation.security) !== "[]"
) {
  fail(
    "runtime catalog OpenAPI operation must be canonical and explicitly public",
  );
}
if (
  runtimeCatalog.scope ||
  runtimeCatalog.workspaceId ||
  runtimeCatalog.input !== "Empty"
) {
  fail(
    "runtime catalog must use empty input and global scope, without a fabricated workspace",
  );
}
if (!baseline.source?.commit || !baseline.source?.sourceTreeHash)
  fail("upstream baseline lacks immutable commit/hash");

const capabilityIds = [...registry.matchAll(/^\| `([A-Z0-9-]+)` \|/gm)].map(
  (match) => match[1],
);
if (
  capabilityIds.length === 0 ||
  new Set(capabilityIds).size !== capabilityIds.length
) {
  fail(
    `capability IDs must be present and unique; found ${capabilityIds.length}`,
  );
}

const workPackets = [...ownership.matchAll(/^\| `WP-(\d+)` \|/gm)].map(
  (match) => Number(match[1]),
);
if (
  workPackets.length !== 50 ||
  new Set(workPackets).size !== workPackets.length
) {
  fail(`expected 50 unique work packets, found ${workPackets.length}`);
}

console.log(
  `[ci-contracts] PASS: domain v1+v2, ${capabilityIds.length} capabilities, ${workPackets.length} work packets, ${commands.commands.length} commands`,
);
