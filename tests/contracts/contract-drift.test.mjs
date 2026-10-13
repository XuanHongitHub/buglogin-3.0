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
const ciWorkflow = await parseYaml(".github/workflows/ci.yml");

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

test("every MCP command input resolves to a closed OpenAPI schema", () => {
  const schemas = canonicalOpenApi.components.schemas;
  const checkInput = (name, schema, seen = new Set()) => {
    assert.ok(schema, `missing OpenAPI input schema ${name}`);
    if (schema.$ref) {
      const target = schema.$ref.split("/").at(-1);
      assert.ok(schemas[target], `unresolved input schema reference ${schema.$ref}`);
      if (seen.has(target)) return;
      seen.add(target);
      checkInput(target, schemas[target], seen);
      return;
    }
    if (schema.type === "object") {
      assert.equal(schema.additionalProperties, false, `${name} accepts unknown fields`);
    }
    for (const variant of schema.oneOf ?? schema.anyOf ?? []) {
      checkInput(name, variant, new Set(seen));
    }
  };
  for (const command of registry.commands.filter((item) => item.adapters.includes("mcp"))) {
    checkInput(command.input, schemas[command.input]);
  }
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
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
  const openApiSource = await read("crates/api/openapi/openapi-v1.yaml");
  const openApiHash = createHash("sha256").update(openApiSource).digest("hex");
  assert.match(
    generated,
    new RegExp(`COMMAND_REGISTRY_SOURCE_SHA256 =\\s*"${sourceHash}"`),
  );
  assert.match(generated, new RegExp(`OpenAPI source SHA-256: ${openApiHash}`));
  const generatedIds = [
    ...generated.matchAll(/^  \| "([^"]+)";?\r?$/gm),
  ].map((match) => match[1]);
  assert.deepEqual(
    generatedIds,
    registry.commands.map((command) => command.id),
  );
  for (const schemaName of Object.keys(runtimeOpenApi.components.schemas)) {
    assert.match(
      generated,
      new RegExp(`^export type ${schemaName}\\b`, "m"),
      `OpenAPI schema ${schemaName} is not generated for the frontend`,
    );
  }
  await execFileAsync(process.execPath, [
    resolve(root, "scripts/generate-api.mjs"),
    "--check",
  ]);
});

test("Profile REST schemas describe the real paginated API and Rust entity", () => {
  const schemas = runtimeOpenApi.components.schemas;
  const expectedProfileFields = [
    "id", "name", "runtime", "runtimeVersion", "runtimeConfig", "state",
    "archived", "fingerprint", "note", "proxyId", "vpnId", "groupIds",
    "tags", "extensionGroupId", "syncMode", "createdAt", "updatedAt",
  ].sort();
  assert.deepEqual(Object.keys(schemas.Profile.properties).sort(), expectedProfileFields);
  assert.equal(schemas.Profile.additionalProperties, false);
  assert.deepEqual(schemas.Profile.required.slice().sort(), expectedProfileFields);
  assert.equal(schemas.VersionedProfile.properties.value.$ref, "#/components/schemas/Profile");
  assert.equal(schemas.VersionedProfilePage.properties.items.items.$ref, "#/components/schemas/VersionedProfile");
  assert.equal(
    runtimeOpenApi.paths["/v1/profiles"].get.responses["200"].content["application/json"].schema.$ref,
    "#/components/schemas/VersionedProfilePage",
  );
});

test("control migration records reject unknown fields outside extensions", () => {
  const schemas = canonicalOpenApi.components.schemas;
  const records = [
    "MigrationNeutralSourceRef",
    "MigrationNeutralAccountRecord",
    "MigrationNeutralOrganizationRecord",
    "MigrationNeutralWorkspaceRecord",
    "MigrationNeutralTeamRecord",
    "MigrationNeutralMemberRecord",
    "MigrationNeutralResourceRecord",
    "MigrationNeutralProfileResourceRecord",
    "MigrationNeutralGrantRecord",
    "MigrationNeutralProfileRecord",
  ];
  for (const name of records) {
    assert.equal(schemas[name].additionalProperties, false, `${name} accepts ignored fields`);
    assert.deepEqual(schemas[name], runtimeOpenApi.components.schemas[name]);
  }
  for (const name of records.slice(1)) {
    const extensions = schemas[name].properties.extensions;
    assert.equal(extensions.type, "object", `${name}.extensions must remain explicit`);
    assert.ok(extensions.additionalProperties, `${name}.extensions must retain arbitrary extension values`);
  }
});

test("operation and problem responses stay aligned with their serialized Rust fields", () => {
  const schemas = canonicalOpenApi.components.schemas;
  assert.deepEqual(
    schemas.Operation,
    runtimeOpenApi.components.schemas.Operation,
    "runtime Operation schema drift",
  );
  assert.deepEqual(
    schemas.Problem,
    runtimeOpenApi.components.schemas.Problem,
    "runtime Problem schema drift",
  );
  assert.deepEqual(
    schemas.Operation.required,
    ["operationId", "commandId", "scope", "status", "progress"],
  );
  assert.equal(
    schemas.Operation.properties.scope.$ref,
    "#/components/schemas/ResourceScope",
  );
  assert.deepEqual(schemas.Operation.properties.status, {
    $ref: "#/components/schemas/OperationStatus",
  });
  assert.deepEqual(schemas.Operation.properties.progress.type, ["number", "null"]);
  assert.deepEqual(
    schemas.Problem.required,
    ["type", "title", "status", "code", "correlationId", "retryable", "fields"],
  );
});

test("profile launch OpenAPI input matches the Rust REST request", () => {
  const schema = runtimeOpenApi.components.schemas.ProfileLaunchInput;
  assert.deepEqual(schema.required, ["profilesRoot", "extraArgs"]);
  assert.deepEqual(Object.keys(schema.properties).sort(), ["extraArgs", "profilesRoot"]);
  assert.deepEqual(schema.properties.extraArgs, { type: "array", items: { type: "string" } });
  const operation = runtimeOpenApi.paths["/v1/profiles/{profileId}/launch"].post;
  assert.equal(operation.requestBody.required, true);
  assert.equal(
    operation.requestBody.content["application/json"].schema.$ref,
    "#/components/schemas/ProfileLaunchInput",
  );
});

test("profile lifecycle and local migration routes document their strict Rust bodies", () => {
  const paths = runtimeOpenApi.paths;
  const schemas = runtimeOpenApi.components.schemas;
  for (const path of [
    "/v1/profiles/{profileId}/archive",
    "/v1/profiles/{profileId}/restore",
  ]) {
    const operation = paths[path].put;
    assert.equal(operation.requestBody.required, true, `${path} requires a revision body`);
    assert.equal(
      operation.requestBody.content["application/json"].schema.$ref,
      "#/components/schemas/ProfileRevisionBody",
    );
  }
  const revision = schemas.ProfileRevisionBody;
  assert.deepEqual(revision.required, ["revision"]);
  assert.deepEqual(Object.keys(revision.properties), ["revision"]);
  assert.equal(revision.additionalProperties, false);

  const deletion = paths["/v1/profiles/{profileId}"].delete;
  assert.equal(deletion.requestBody.required, true);
  assert.equal(
    deletion.requestBody.content["application/json"].schema.$ref,
    "#/components/schemas/ProfileDeleteBody",
  );
  assert.equal(schemas.ProfileDeleteBody.properties.confirmation.const, "QUARANTINE_PROFILE_DATA");
  assert.equal(schemas.ProfileDeleteBody.additionalProperties, false);

  for (const name of ["MigrationInspectInput", "MigrationImportInput"]) {
    assert.equal(schemas[name].additionalProperties, false, `${name} accepts unknown fields`);
  }
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
});

test("organization and workspace REST contracts describe real JSON bodies and receipts", () => {
  const paths = runtimeOpenApi.paths;
  const organizations = paths["/v1/organizations"];
  const organization = paths["/v1/organizations/{organizationId}"];
  const workspaces = paths["/v1/organizations/{organizationId}/workspaces"];
  const workspace = paths["/v1/organizations/{organizationId}/workspaces/{workspaceId}"];
  const bodySchema = operation => operation.requestBody.content["application/json"].schema.$ref;
  const responseSchema = (operation, status) => operation.responses[status].content["application/json"].schema.$ref;

  assert.equal(bodySchema(organizations.post), "#/components/schemas/OrganizationCreateInput");
  assert.equal(responseSchema(organizations.post, "200"), "#/components/schemas/OrganizationRecord");
  assert.equal(bodySchema(organization.patch), "#/components/schemas/OrganizationUpdate");
  assert.equal(responseSchema(organization.patch, "200"), "#/components/schemas/Organization");
  assert.equal(bodySchema(workspaces.post), "#/components/schemas/Workspace");
  assert.equal(responseSchema(workspaces.post, "200"), "#/components/schemas/Workspace");
  assert.equal(bodySchema(workspace.patch), "#/components/schemas/WorkspaceUpdate");
  assert.equal(responseSchema(workspace.patch, "200"), "#/components/schemas/Workspace");
  for (const operation of [organizations.post, organization.patch, workspaces.post, workspace.patch]) {
    assert.ok(operation.parameters.some(parameter => parameter.$ref === "#/components/parameters/IdempotencyKey"));
  }
});

test("Team REST routes and command schemas match the Rust versioned entity", () => {
  const teamList = runtimeOpenApi.paths["/v1/workspaces/{workspaceId}/teams"];
  const teamItem = runtimeOpenApi.paths["/v1/workspaces/{workspaceId}/teams/{teamId}"];
  assert.equal(teamList.get.operationId, "team.list");
  assert.equal(teamList.get.responses["200"].content["application/json"].schema.$ref, "#/components/schemas/TeamRecordList");
  assert.equal(teamItem.delete.operationId, "team.delete");
  assert.equal(commands.get("team.delete").input, "TeamDeleteInput");
  assert.equal(commands.get("team.delete").output, "TeamRecord");
  assert.equal(teamItem.delete.requestBody.content["application/json"].schema.$ref, "#/components/schemas/TeamDeleteBody");
  assert.equal(teamItem.delete.responses["200"].content["application/json"].schema.$ref, "#/components/schemas/TeamRecord");
  assert.equal(runtimeOpenApi.components.schemas.TeamRecord.properties.team.$ref, "#/components/schemas/Team");
  assert.equal(runtimeOpenApi.components.schemas.TeamRecord.additionalProperties, false);
  for (const id of ["team.list", "team.get", "team.create", "team.update", "team.delete"]) {
    const command = commands.get(id);
    assert.ok(runtimeOpenApi.components.schemas[command.input], `${id} input schema is missing from OpenAPI`);
  }
});

test("member role REST contract matches the Rust PUT handler and generated body DTO", () => {
  const operation = runtimeOpenApi.paths["/v1/workspaces/{workspaceId}/members/{memberId}/role"].put;
  assert.equal(operation.operationId, "member.role.update");
  assert.deepEqual(
    operation.requestBody.content["application/json"].schema,
    { $ref: "#/components/schemas/MemberRoleUpdateBody" },
  );
  assert.equal(operation.requestBody.required, true);
  assert.equal(operation.responses["200"].content["application/json"].schema.$ref, "#/components/schemas/Member");
  assert.ok(operation.parameters.some(parameter => parameter.$ref === "#/components/parameters/IdempotencyKey"));
  assert.deepEqual(runtimeOpenApi.components.schemas.MemberRoleUpdateBody.required, ["role"]);
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
});

test("member activation and suspension contracts match Rust PUT mutations", () => {
  for (const action of ["activate", "suspend"]) {
    const operation = runtimeOpenApi.paths[`/v1/workspaces/{workspaceId}/members/{memberId}/${action}`].put;
    assert.equal(operation.operationId, `member.${action}`);
    assert.ok(operation.parameters.some(parameter => parameter.$ref === "#/components/parameters/IdempotencyKey"));
    assert.equal(operation.responses["200"].content["application/json"].schema.$ref, "#/components/schemas/Member");
  }
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
});

test("account, workspace policy, binding, and grant endpoints expose wire schemas", () => {
  const paths = runtimeOpenApi.paths;
  const responseSchema = operation => operation.responses["200"].content["application/json"].schema;
  assert.deepEqual(responseSchema(paths["/v1/account"].get), { $ref: "#/components/schemas/AccountView" });
  assert.equal(responseSchema(paths["/v1/accounts"].get).items.$ref, "#/components/schemas/Account");
  const status = paths["/v1/accounts/{accountId}/status"].put;
  assert.equal(status.requestBody.content["application/json"].schema.$ref, "#/components/schemas/AccountStatusUpdateInput");
  assert.equal(responseSchema(status).$ref, "#/components/schemas/Account");
  assert.ok(status.parameters.some(parameter => parameter.$ref === "#/components/parameters/IdempotencyKey"));
  assert.equal(responseSchema(paths["/v1/profiles/{profileId}/binding"].get).$ref, "#/components/schemas/ProfileResource");
  assert.equal(paths["/v1/profiles/{profileId}"].delete.operationId, "profile.delete");
  assert.equal(paths["/v1/profiles/{profileId}/binding"].delete, undefined);
  assert.equal(responseSchema(paths["/v1/organizations/{organizationId}/workspaces/{workspaceId}/snapshot"].get).$ref, "#/components/schemas/OrganizationSnapshot");
  assert.equal(paths["/v1/organizations/{organizationId}/workspaces/{workspaceId}/usage/check"].post.requestBody.content["application/json"].schema.$ref, "#/components/schemas/WorkspaceUsage");
  assert.equal(responseSchema(paths["/v1/organizations/{organizationId}/workspaces/{workspaceId}/offline-entitlement"].get).$ref, "#/components/schemas/OfflineEntitlement");
  assert.equal(responseSchema(paths["/v1/workspaces/{workspaceId}/grants"].get).items.$ref, "#/components/schemas/Grant");
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
});

test("Rust HTTP routes have OpenAPI methods or an explicit deferred inventory", async () => {
  const routeSources = await Promise.all([
    read("crates/api/src/http/mod.rs"),
    read("crates/api/src/http/control.rs"),
  ]);
  const routes = routeSources.flatMap(source => [...source.matchAll(/\.route\(\s*"([^"]+)"([\s\S]*?)(?=\.route\(|\n\s*\})/g)]
    .flatMap(match => [...match[2].matchAll(/(?:^|[.\s])(get|post|put|patch|delete)\(/g)]
      .map(method => ({ path: match[1], method: method[1] }))));
  const normalizePath = path => path.replace(/\{[^}]+\}/g, "{}");
  const drift = routes.flatMap(route => {
    const path = Object.keys(runtimeOpenApi.paths).find(candidate => normalizePath(candidate) === normalizePath(route.path));
    return path && runtimeOpenApi.paths[path][route.method]
      ? []
      : [`${route.method.toUpperCase()} ${route.path}`];
  }).sort();
  // Every Rust HTTP operation must have a canonical public wire contract.
  assert.deepEqual(drift, []);
});

test("connected cookie job HTTP schemas exclude server authority and cover device receipts", () => {
  const paths = runtimeOpenApi.paths;
  const schemas = runtimeOpenApi.components.schemas;
  const enqueue = paths["/v1/cookie-jobs"].post;
  assert.equal(enqueue.responses["202"].content["application/json"].schema.$ref, "#/components/schemas/ConnectedCookieJob");
  assert.equal(enqueue.requestBody.content["application/json"].schema.$ref, "#/components/schemas/EnqueueCookieJobInput");
  assert.ok(enqueue.parameters.some(parameter => parameter.$ref === "#/components/parameters/IdempotencyKey"));
  const enqueueProperties = Object.keys(schemas.EnqueueCookieJobInput.properties);
  for (const serverOwned of ["requestedBy", "idempotencyKey", "createdAt", "expiresAt", "now"]) {
    assert.ok(!enqueueProperties.includes(serverOwned), `${serverOwned} must be server-owned`);
  }
  for (const suffix of ["", "/claim", "/ack", "/complete"]) {
    const operation = suffix ? paths[`/v1/cookie-jobs/{jobId}${suffix}`].post : enqueue;
    assert.ok(operation, `missing cookie job route ${suffix}`);
  }
  assert.equal(paths["/v1/cookie-jobs/{jobId}"].get.parameters.find(parameter => parameter.name === "workspaceId").required, true);
  const inbox = paths["/v1/workspaces/{workspaceId}/cookie-jobs/inbox"].get;
  assert.equal(inbox.operationId, "cookie.jobs.inbox");
  assert.match(inbox.description, /device bearer/i);
  assert.match(inbox.description, /claim checks those permissions again/i);
  assert.equal(inbox.responses["200"].content["application/json"].schema.$ref, "#/components/schemas/ConnectedCookieJobInbox");
  assert.ok(inbox.parameters.some(parameter => parameter.$ref === "#/components/parameters/WorkspaceId"));
  assert.equal(runtimeOpenApi.components.parameters.WorkspaceId.in, "path");
  assert.equal(paths["/v1/cookie-jobs/{jobId}/complete"].post.requestBody.content["application/json"].schema.$ref, "#/components/schemas/ConnectedCookieReceipt");
  assert.equal(schemas.ConnectedCookieJobState.enum.includes("expired"), true);
  assert.equal(schemas.ConnectedCookieReceipt.properties.metadataChecksum.pattern, "^[0-9a-fA-F]{64}$");
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
});

test("control migration REST contracts match the protected Rust import and rollback DTOs", () => {
  const paths = runtimeOpenApi.paths;
  const schemas = runtimeOpenApi.components.schemas;
  const importOperation = paths["/v1/migrations/control/import"].post;
  const rollbackOperation = paths["/v1/migrations/control/rollback"].post;
  assert.equal(importOperation.operationId, "migration.control.import");
  assert.equal(importOperation.requestBody.content["application/json"].schema.$ref, "#/components/schemas/ControlImportInput");
  assert.equal(importOperation.responses["200"].content["application/json"].schema.$ref, "#/components/schemas/ControlImportResult");
  assert.equal(schemas.ControlImportResult.oneOf.length, 2);
  assert.ok(importOperation.parameters.some(parameter => parameter.$ref === "#/components/parameters/IdempotencyKey"));
  assert.equal(schemas.ControlImportInput.additionalProperties, false);
  assert.deepEqual(
    Object.keys(schemas.ControlImportInput.properties).sort(),
    ["accounts", "dryRun", "grants", "members", "organizations", "packageId", "profiles", "profileResources", "resources", "teams", "workspaces"].sort(),
  );
  assert.ok(schemas.ControlImportInput.properties.profiles.items.$ref.endsWith("MigrationNeutralProfileRecord"));
  assert.ok(schemas.ControlImportInput.properties.profileResources.items.$ref.endsWith("MigrationNeutralProfileResourceRecord"));
  assert.ok(schemas.MigrationNeutralProfileRecord.properties.data.required.includes("tags"));
  assert.equal(schemas.MigrationNeutralProfileRecord.properties.data.additionalProperties, false, "neutral v1 profile data rejects unknown fields");
  assert.equal(rollbackOperation.operationId, "migration.control.rollback");
  assert.equal(rollbackOperation.requestBody.content["application/json"].schema.$ref, "#/components/schemas/ControlRollbackInput");
  assert.equal(schemas.ControlRollbackInput.properties.confirmation.const, "ROLLBACK_CONTROL_MIGRATION");
  assert.ok(rollbackOperation.parameters.some(parameter => parameter.$ref === "#/components/parameters/IdempotencyKey"));
  assert.equal(rollbackOperation.responses["200"].content["application/json"].schema.$ref, "#/components/schemas/ControlImportReceipt");
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
});

test("sync migration REST contract matches the strict Rust import payload and receipt", () => {
  const operation = runtimeOpenApi.paths["/v1/migrations/sync/import"].post;
  const schemas = runtimeOpenApi.components.schemas;
  assert.equal(operation.operationId, "migration.sync.import");
  assert.equal(operation.requestBody.required, true);
  assert.match(operation.requestBody.description, /limited to 320 MiB/i);
  assert.match(operation.requestBody.description, /one import admitted per API process/i);
  assert.equal(operation.requestBody.content["application/json"].schema.$ref, "#/components/schemas/SyncImportInput");
  assert.ok(operation.parameters.some(parameter => parameter.$ref === "#/components/parameters/IdempotencyKey"));
  assert.equal(operation.responses["200"].content["application/json"].schema.$ref, "#/components/schemas/SyncImportReceipt");
  assert.ok(operation.responses["413"] && operation.responses["415"] && operation.responses["422"]);
  assert.equal(schemas.SyncImportInput.additionalProperties, false);
  assert.deepEqual(schemas.SyncImportInput.required, ["export"]);
  assert.equal(schemas.SyncExport.additionalProperties, false);
  assert.equal(schemas.SyncExport.properties.manifest.$ref, "#/components/schemas/SyncManifest");
  assert.equal(schemas.SyncExportBlob.properties.data.items.maximum, 255);
  assert.equal(schemas.SyncExportBlob.properties.size.maximum, 67108864);
  for (const schema of ["SyncRevision", "SyncExportBlob", "SyncManifest", "ManifestObject", "ManifestTombstone", "ManifestDependency", "ManifestPayloadEncryption", "ManifestObjectReference", "SyncImportReceipt"]) {
    const variants = schemas[schema].oneOf ?? [schemas[schema]];
    assert.ok(variants.every(variant => variant.additionalProperties === false), `${schema} accepts unknown fields`);
  }
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
});

test("browser login documents bounded password verification overload", () => {
  const login = runtimeOpenApi.paths["/v1/auth/login"].post;
  assert.equal(login.operationId, "auth.login");
  assert.equal(login.security.length, 0, "login must remain unauthenticated");
  assert.equal(login.requestBody.content["application/json"].schema.$ref, "#/components/schemas/WebLoginInput");
  assert.ok(login.responses["503"], "password verification overload must be an explicit retryable service error");
  assert.equal(runtimeOpenApi.components.schemas.WebLoginInput.additionalProperties, false);
  assert.deepEqual(canonicalOpenApi, runtimeOpenApi, "runtime OpenAPI must mirror its contract copy");
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

test("required CI builds and smoke-tests the Rust API container", () => {
  const containerJob = ciWorkflow.jobs["api-container"];
  assert.ok(containerJob, "required CI must build the supported Rust API image");
  const build = containerJob.steps.find(step => step.run?.includes("docker build"));
  assert.match(build?.run ?? "", /docker build --pull -f Dockerfile\.api/);
  const smoke = containerJob.steps.find(step => step.name?.includes("Smoke-check"));
  assert.ok(smoke?.run, "container job must exercise the image after build");
  for (const required of [
    "/v1/health",
    "/v1/openapi.yaml",
    "cmp crates/api/openapi/openapi-v1.yaml",
    "BUGLOGIN_ENV=production",
    "BUGLOGIN_POSTGRES_URL is required in production",
  ]) {
    assert.ok(smoke.run.includes(required), `container smoke gate must include ${required}`);
  }
  assert.ok(
    ciWorkflow.jobs.status.needs.includes("api-container"),
    "aggregate CI status must require the API container job",
  );
});

