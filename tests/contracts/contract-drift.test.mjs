import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import YAML from "yaml";

const root = resolve(import.meta.dirname, "../..");
const read = (path) => readFile(resolve(root, path), "utf8");
const parseYaml = async (path) => YAML.parse(await read(path));
const execFileAsync = promisify(execFile);

const registrySource = await read("docs/contracts/command-registry-v1.yaml");
const registry = YAML.parse(registrySource);
const adapterContract = await parseYaml("docs/contracts/adapter-contract-v1.yaml");
const canonicalOpenApi = await parseYaml("docs/contracts/openapi-v1.yaml");
const runtimeOpenApi = await parseYaml("crates/api/openapi/openapi-v1.yaml");
const productionSyncContract = await parseYaml("docs/contracts/production-launch-sync-v1.yaml");

const commands = new Map(
  registry.commands.map((command) => [command.id, command]),
);
assert.equal(
  commands.size,
  registry.commands.length,
  "command registry contains duplicate IDs",
);

function expectedCommandIds(adapter) {
  return registry.commands
    .filter((command) => command.adapters.includes(adapter))
    .map((command) => command.id)
    .sort();
}

function openApiOperationIds(document) {
  const methods = new Set([
    "delete",
    "get",
    "head",
    "options",
    "patch",
    "post",
    "put",
    "trace",
  ]);
  return Object.values(document.paths ?? {})
    .flatMap((path) =>
      Object.entries(path ?? {})
        .filter(
          ([method, operation]) =>
            methods.has(method) && operation?.operationId,
        )
        .map(([, operation]) => operation.operationId),
    )
    .sort();
}

function surfaceDrift(expected, actual) {
  const expectedSet = new Set(expected);
  const actualSet = new Set(actual);
  return {
    missing: expected.filter((id) => !actualSet.has(id)),
    unexpected: actual.filter((id) => !expectedSet.has(id)),
    duplicates: actual.filter((id, index) => actual.indexOf(id) !== index),
  };
}

function assertExactSurface(surface, expected, actual) {
  assert.deepEqual(
    surfaceDrift(expected, actual),
    { missing: [], unexpected: [], duplicates: [] },
    `${surface} drift`,
  );
}

function assertAdapterRows(surface, adapter, rows, nameKey) {
  const expected = expectedCommandIds(adapter);
  const actual = rows.map((row) => row.commandId).sort();
  assertExactSurface(surface, expected, actual);

  for (const row of rows) {
    const command = commands.get(row.commandId);
    assert.ok(command, `${surface} references unknown command ${row.commandId}`);
    assert.equal(
      row[nameKey],
      row.commandId.replaceAll(".", "_"),
      `${surface} name drift for ${row.commandId}`,
    );
    assert.equal(
      row.inputSchema ?? row.input,
      command.input,
      `${surface} input drift for ${row.commandId}`,
    );
    assert.equal(
      row.outputSchema ?? row.output,
      command.output,
      `${surface} output drift for ${row.commandId}`,
    );
  }
}

test("REST OpenAPI matches the canonical command registry", () => {
  const expected = expectedCommandIds("rest");
  assert.deepEqual(
    {
      canonical: surfaceDrift(
        expected,
        openApiOperationIds(canonicalOpenApi),
      ),
      runtime: surfaceDrift(expected, openApiOperationIds(runtimeOpenApi)),
    },
    {
      canonical: { missing: [], unexpected: [], duplicates: [] },
      runtime: { missing: [], unexpected: [], duplicates: [] },
    },
    "REST OpenAPI drift",
  );
});

test("Tauri commands match the canonical command registry", () => {
  assertAdapterRows(
    "Tauri",
    "tauri",
    adapterContract.tauri.commands,
    "invoke",
  );
});

test("MCP tools match the canonical command registry", () => {
  assertAdapterRows("MCP", "mcp", adapterContract.mcp.tools, "name");
});

test("legacy Tauri MCP advertises no non-canonical tools", async () => {
  const source = await read("src-tauri/src/mcp_server.rs");
  assert.match(
    source,
    /pub fn get_tools\(&self\) -> Vec<McpTool> \{\s*Vec::new\(\)\s*\}/,
  );
  assert.match(
    source,
    /Legacy MCP tools are disabled until they delegate to the canonical BugLogin command authority/,
  );
});

test("Generated frontend registry matches its canonical source", async () => {
  const generated = await read("frontend/src/generated/command-registry.ts");
  const sourceHash = createHash("sha256").update(registrySource).digest("hex");
  assert.match(
    generated,
    new RegExp(`COMMAND_REGISTRY_SOURCE_SHA256 =\\s*"${sourceHash}"`),
  );
  const generatedIds = [
    ...generated.matchAll(/^  \| "([^"]+)";?\r?$/gm),
  ].map((match) => match[1]);
  assert.deepEqual(
    generatedIds,
    registry.commands.map((command) => command.id),
  );
});

test("Generated Rust registry exactly matches its canonical source", async () => {
  const generated = await read("crates/api/src/commands/generated.rs");
  const sourceHash = createHash("sha256").update(registrySource).digest("hex");
  assert.match(
    generated,
    new RegExp(`COMMAND_REGISTRY_SOURCE_SHA256: &str =\\s*"${sourceHash}"`),
  );
  const generatedIds = [
    ...generated.matchAll(/^    meta\(\r?\n        "([^"]+)",\r?\n        CommandKind::/gm),
  ].map((match) => match[1]);
  assert.deepEqual(
    generatedIds,
    registry.commands.map((command) => command.id),
  );
  await execFileAsync(process.execPath, [
    resolve(root, "scripts/generate-api.mjs"),
    "--check-rust",
  ]);
});

test("Hosted sync contract exposes unknown-commit reconciliation", () => {
  assert.ok(
    productionSyncContract.errors?.includes("migration_sync_commit_outcome_unknown"),
    "hosted sync contract must expose the retryable unknown-commit reconciliation error",
  );
});

