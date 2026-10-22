import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parse, stringify } from "yaml";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = resolve(root, "docs/contracts/command-registry-v1.yaml");
const openApiSourcePath = resolve(root, "crates/api/openapi/openapi-v1.yaml");
const openApiContractOutputPath = resolve(root, "docs/contracts/openapi-v1.yaml");
const frontendOutputPath = resolve(root, "frontend/src/generated/command-registry.ts");
const adapterOutputPath = resolve(root, "docs/contracts/adapter-contract-v1.yaml");
const rustOutputPath = resolve(root, "crates/api/src/commands/generated.rs");
const checkOnly = process.argv.includes("--check") || process.argv.includes("--check-rust");
const rustOnly = process.argv.includes("--rust-only") || process.argv.includes("--check-rust");

const source = (await readFile(sourcePath, "utf8")).replace(/\r\n/g, "\n");
const registry = parse(source);
const openApiSource = (await readFile(openApiSourcePath, "utf8")).replace(/\r\n/g, "\n");
const openApi = parse(openApiSource);

if (
  !registry ||
  typeof registry !== "object" ||
  !Array.isArray(registry.commands)
) {
  throw new Error("Invalid command registry: expected a commands array");
}

const defaults = registry.defaults ?? {};
const commands = registry.commands.map((command, index) =>
  normalizeCommand(command, defaults, index),
);
const ids = new Set();
const mcpNames = new Set();
for (const command of commands) {
  if (ids.has(command.id))
    throw new Error(`Duplicate command id: ${command.id}`);
  ids.add(command.id);
  if (command.adapters.includes("mcp")) {
    const name = command.id.replaceAll(".", "_");
    if (mcpNames.has(name)) throw new Error(`Duplicate MCP tool name: ${name}`);
    mcpNames.add(name);
  }
}

const mcpInputs = [...new Set(
  commands.filter((command) => command.adapters.includes("mcp")).map((command) => command.input),
)];
const mcpInputSchemas = Object.fromEntries(mcpInputs.map((name) => [name, makeMcpInputSchema(name, openApi?.components?.schemas)]));

const sourceHash = createHash("sha256").update(source).digest("hex");
const openApiHash = createHash("sha256").update(openApiSource).digest("hex");
const schemas = openApi?.components?.schemas;
const profileTypes = [
  ["Runtime", schemas?.Runtime],
  ["ProfileState", schemas?.ProfileState],
  ["SyncMode", schemas?.SyncMode],
  ["JsonValue", schemas?.JsonValue],
  ["Profile", schemas?.Profile],
  ["ProfileRecord", schemas?.ProfileRecord],
  ["ProfileResource", schemas?.ProfileResource],
  ["ProfileCreateInput", schemas?.ProfileCreateCommandInput],
  ["ProfileProxyAssignment", schemas?.ProfileProxyAssignment],
  ["ProfileProxyAssignmentGetInput", schemas?.ProfileProxyAssignmentGetInput],
  ["ProfileProxyAssignmentSetInput", schemas?.ProfileProxyAssignmentSetInput],
  ["Group", schemas?.Group],
  ["GroupRecord", schemas?.GroupRecord],
  ["GroupRecordList", schemas?.GroupRecordList],
  ["GroupShareGrant", schemas?.GroupShareGrant],
  ["GroupShareGrantList", schemas?.GroupShareGrantList],
  ["GroupCreateInput", schemas?.GroupCreateInput],
  ["GroupShareCreateInput", schemas?.GroupShareCreateInput],
  ["GroupShareRevokeInput", schemas?.GroupShareRevokeInput],
  ["GroupUpdateInput", schemas?.GroupUpdateInput],
  ["GroupDeleteInput", schemas?.GroupDeleteInput],
  ["ProfileGroupsReplaceInput", schemas?.ProfileGroupsReplaceInput],
  ["ResourceScope", schemas?.ResourceScope],
  ["Problem", schemas?.Problem],
  ["OperationStatus", schemas?.OperationStatus],
  ["Operation", schemas?.Operation],
  ["ProfileLaunchInput", schemas?.ProfileLaunchInput],
  ["WebSessionResponse", schemas?.WebSessionResponse],
  ["WebSession", schemas?.WebSession],
  ["ApiTokenCreateInput", schemas?.ApiTokenCreateInput],
  ["ApiToken", schemas?.ApiToken],
  ["ProxyMetadataInput", schemas?.ProxyMetadataInput],
  ["ProxyView", schemas?.ProxyView],
  ["ProxyRecord", schemas?.ProxyRecord],
  ["ProxyPage", schemas?.ProxyPage],
  ["ProxyImportInput", schemas?.ProxyImportInput],
  ["ProxyImportReport", schemas?.ProxyImportReport],
  ["ProxyCreateInput", schemas?.ProxyCreateInput],
  ["ProxyUpdateInput", schemas?.ProxyUpdateInput],
  ["ProxyDeleteInput", schemas?.ProxyDeleteInput],
  ["ProxyCreateResult", schemas?.ProxyCreateResult],
  ["Organization", schemas?.Organization],
  ["OrganizationRecord", schemas?.OrganizationRecord],
  ["OrganizationCreateInput", schemas?.OrganizationCreateInput],
  ["OrganizationUpdate", schemas?.OrganizationUpdate],
  ["OrganizationPolicy", schemas?.OrganizationPolicy],
  ["WorkspacePolicy", schemas?.WorkspacePolicy],
  ["Workspace", schemas?.Workspace],
  ["WorkspaceUpdate", schemas?.WorkspaceUpdate],
  ["Team", schemas?.Team],
  ["TeamRecord", schemas?.TeamRecord],
  ["MemberRole", schemas?.MemberRole],
  ["MemberStatus", schemas?.MemberStatus],
  ["Action", schemas?.Action],
  ["Effect", schemas?.Effect],
  ["ProxyProtocol", schemas?.ProxyProtocol],
  ["Member", schemas?.Member],
  ["ResourceKind", schemas?.ResourceKind],
  ["ResourceState", schemas?.ResourceState],
  ["Resource", schemas?.Resource],
  ["Grant", schemas?.Grant],
  ["GrantCreateInput", schemas?.GrantCreateInput],
  ["AuditResource", schemas?.AuditResource],
  ["AuditEvent", schemas?.AuditEvent],
  ["AuditEventPage", schemas?.AuditEventPage],
  ["ProxyCheckInput", schemas?.ProxyCheckInput],
  ["ProxyCheckResult", schemas?.ProxyCheckResult],
  ["VersionedProfile", schemas?.VersionedProfile],
  ["VersionedProfilePage", schemas?.VersionedProfilePage],
].map(([name, schema]) => renderOpenApiType(name, schema));
const generatedSchemaNames = new Set(profileTypes.flatMap((type) => [...type.matchAll(/^export type (\w+)/gm)].map((match) => match[1])));
for (const [name, schema] of Object.entries(schemas ?? {})) {
  if (!generatedSchemaNames.has(name)) profileTypes.push(renderOpenApiType(name, schema));
}
const frontendOutput = render({
  registryId: requiredString(registry.registryId, "registryId"),
  version: requiredString(registry.version, "version"),
  sourceHash,
  openApiHash,
  profileTypes,
  commands,
});
const adapterOutput = renderAdapters(commands, sourceHash);
const rustOutput = renderRust(commands, sourceHash, mcpInputSchemas);

if (checkOnly) {
  if (!rustOnly) {
    await checkGenerated(openApiContractOutputPath, openApiSource);
    await checkGenerated(frontendOutputPath, frontendOutput);
    await checkGenerated(adapterOutputPath, adapterOutput);
  }
  await checkGenerated(rustOutputPath, rustOutput);
} else {
  if (!rustOnly) {
    await mkdir(dirname(openApiContractOutputPath), { recursive: true });
    await writeFile(openApiContractOutputPath, openApiSource, "utf8");
    await mkdir(dirname(frontendOutputPath), { recursive: true });
    await writeFile(frontendOutputPath, frontendOutput, "utf8");
    await writeFile(adapterOutputPath, adapterOutput, "utf8");
  }
  await writeFile(rustOutputPath, rustOutput, "utf8");
}

async function checkGenerated(path, expected) {
  const existing = (await readFile(path, "utf8").catch(() => null))?.replace(/\r\n/g, "\n");
  if (existing !== expected.replace(/\r\n/g, "\n")) {
    console.error(`Generated file is stale: ${path}`);
    process.exitCode = 1;
  }
}

function renderOpenApiType(name, schema) {
  if (!schema || typeof schema !== "object") {
    throw new Error(`OpenAPI schema ${name} is required for generated frontend types`);
  }
  if (Array.isArray(schema.enum) && schema.enum.length > 0) {
    const values = JSON.stringify(schema.enum);
    const constant = `${name.replace(/([a-z])([A-Z])/g, "$1_$2").toUpperCase()}_VALUES`;
    return `export const ${constant} = ${values} as const;\nexport type ${name} = (typeof ${constant})[number];`;
  }
  if (name === "JsonValue") {
    return "export interface JsonObject { [key: string]: JsonValue; }\nexport type JsonValue = JsonObject | JsonValue[] | string | number | boolean | null;";
  }
  return `export type ${name} = ${schemaToTypeScript(schema, name)};`;
}

function schemaToTypeScript(schema, owner) {
  if (schema.$ref) return schema.$ref.split("/").at(-1);
  if (schema.const !== undefined) return JSON.stringify(schema.const);
  if (Array.isArray(schema.enum) && schema.enum.length > 0) {
    return schema.enum.map((value) => JSON.stringify(value)).join(" | ");
  }
  if (Array.isArray(schema.anyOf)) {
    return schema.anyOf.map((variant) => schemaToTypeScript(variant, owner)).join(" | ");
  }
  if (Array.isArray(schema.oneOf)) {
    return schema.oneOf.map((variant) => schemaToTypeScript(variant, owner)).join(" | ");
  }
  if (Array.isArray(schema.allOf)) {
    return schema.allOf.map((variant) => schemaToTypeScript(variant, owner)).join(" & ");
  }
  if (Array.isArray(schema.type)) {
    return schema.type.map((type) => schemaToTypeScript({ ...schema, type }, owner)).join(" | ");
  }
  if (schema.type === "array") {
    if (!schema.items) throw new Error(`OpenAPI array ${owner} is missing items`);
    return `Array<${schemaToTypeScript(schema.items, owner)}>`;
  }
  if (schema.type === "object") {
    if (schema.properties) {
      if (schema.required !== undefined && !Array.isArray(schema.required)) {
        throw new Error(`OpenAPI object ${owner} has invalid required fields`);
      }
      if (Object.keys(schema.properties).length === 0 && schema.additionalProperties === false) {
        return "Record<string, never>";
      }
      const fields = Object.entries(schema.properties).map(([field, property]) => {
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(field)) throw new Error(`Invalid ${owner} field name: ${field}`);
        const optional = (schema.required ?? []).includes(field) ? "" : "?";
        return `  ${field}${optional}: ${schemaToTypeScript(property, `${owner}.${field}`)};`;
      });
      if (schema.additionalProperties && typeof schema.additionalProperties === "object") {
        fields.push(`  [key: string]: ${schemaToTypeScript(schema.additionalProperties, `${owner} additional property`)};`);
      } else if (schema.additionalProperties === true) {
        fields.push("  [key: string]: unknown;");
      }
      return `{\n${fields.join("\n")}\n}`;
    }
    if (schema.additionalProperties && typeof schema.additionalProperties === "object") {
      return `Record<string, ${schemaToTypeScript(schema.additionalProperties, owner)}>`;
    }
    return "Record<string, unknown>";
  }
  if (schema.type === "string") return "string";
  if (schema.type === "integer" || schema.type === "number") return "number";
  if (schema.type === "boolean") return "boolean";
  if (schema.type === "null") return "null";
  if (!schema.type) return "unknown";
  throw new Error(`Unsupported OpenAPI type ${schema.type} for ${owner}`);
}

function renderAdapters(commands, sourceHash) {
  const metadata = (command) => ({
    commandId: command.id,
    kind: command.kind,
    capability: command.capability,
    action: command.action,
    inputSchema: command.input,
    outputSchema: command.output,
    auth: command.auth,
    audit: command.audit,
    idempotency: command.idempotency,
    timeoutMs: command.timeoutMs,
    ...(command.events.length > 0 ? { events: command.events } : {}),
    ...(command.preconditions.length > 0
      ? { preconditions: command.preconditions }
      : {}),
  });
  const requiresIdempotency = (command) =>
    command.idempotency === "required"
      ? { requires: ["Idempotency-Key"] }
      : {};
  const projections = commands.map((command) => ({
    ...metadata(command),
    adapters: command.adapters,
    names: Object.fromEntries(
      command.adapters.map((adapter) => [
        adapter,
        adapter === "rest" ? command.id : command.id.replaceAll(".", "_"),
      ]),
    ),
  }));
  const mcpTools = commands
    .filter((command) => command.adapters.includes("mcp"))
    .map((command) => ({
      name: command.id.replaceAll(".", "_"),
      ...metadata(command),
      security: {
        authentication:
          command.auth === "public"
            ? "public"
            : command.auth === "optional-local"
            ? "optional-local-bearer-token"
            : "bearer-token",
        authorization: "canonical-capability-acl",
        scope: "canonical-resource-scope",
        secrets: "never-returned",
      },
      ...requiresIdempotency(command),
    }));
  const restCommands = commands
    .filter((command) => command.adapters.includes("rest"))
    .map((command) => ({
      operationId: command.id,
      ...metadata(command),
      security: {
        authentication:
          command.auth === "public"
            ? "public"
            : command.auth === "optional-local"
            ? "optional-local-auth"
            : "bearer-token-or-session",
        authorization: "canonical-capability-acl",
        scope: "canonical-resource-scope",
        errors: "Problem",
      },
      ...requiresIdempotency(command),
    }));
  const tauriCommands = commands
    .filter((command) => command.adapters.includes("tauri"))
    .map((command) => ({
      invoke: command.id.replaceAll(".", "_"),
      ...metadata(command),
      security: {
        authentication:
          command.auth === "public"
            ? "public"
            : command.auth === "optional-local"
            ? "optional-local-session"
            : "canonical-session",
        authorization: "canonical-capability-acl",
        scope: "canonical-resource-scope",
        filesystem: "rust-only",
      },
      ...(command.idempotency === "required"
        ? { retry: "use-idempotency-key" }
        : {}),
    }));
  const document = {
    version: "1.0",
    name: "buglogin-adapters-v1",
    sourceOfTruth: "docs/contracts/command-registry-v1.yaml",
    sourceSha256: sourceHash,
    projectionCount: commands.length,
    projections,
    mcp: {
      transport: "stdio-or-local-http",
      enabledByDefault: false,
      authentication: "bearer-token",
      toolNaming: "replace-dots-with-underscores",
      resultShape: {
        success: {
          content: [{ type: "text", text: "string" }],
          structuredContent: "object",
        },
        error: {
          isError: true,
          content: [{ type: "text", text: "string" }],
          structuredContent: "Problem",
        },
      },
      tools: mcpTools,
      policy: {
        acl: "same-as-rest-and-tauri",
        audit: "required",
        resourceScope: "required",
        secrets: "never-returned",
      },
    },
    rest: {
      operationIdNaming: "command-id",
      routes: "defined-by-runtime-HTTP-authority; not specified here",
      commands: restCommands,
      policy: {
        acl: "canonical-capability-acl",
        audit: "required",
        resourceScope: "required",
        errors: "Problem",
        secrets: "never-returned",
      },
    },
    tauri: {
      invokeNamespace: "buglogin",
      commandNaming: "snake_case",
      commands: tauriCommands,
      eventChannels: [
        { name: "operation.updated", payload: "OperationEvent" },
        { name: "profile.runtime.updated", payload: "RuntimeProjectionEvent" },
        { name: "migration.item.updated", payload: "MigrationItemEvent" },
      ],
      rules: [
        "validate-before-domain-call",
        "no-filesystem-access-from-react",
        "no-authorization-in-react",
        "return-Problem-not-string-errors",
      ],
    },
    shared: {
      correlationId: "required-on-every-request",
      idempotency: "required-for-mutations-and-operations",
      serialization: "JSON",
      timestamps: "RFC3339-UTC",
      compatibility: "additive-fields-only-within-v1",
      executionOrder: [
        "validate-schema",
        "authenticate",
        "resolve-scope",
        "authorize-capability",
        "claim-idempotency-key",
        "execute-domain-operation",
        "append-audit-event",
        "publish-domain-events",
      ],
    },
  };
  return `# @generated by scripts/generate-api.mjs; do not edit.\n${stringify(document)}`;
}

function normalizeCommand(command, defaults, index) {
  if (!command || typeof command !== "object")
    throw new Error(`Invalid command at index ${index}`);
  const id = requiredString(command.id, `commands[${index}].id`);
  const kind = requiredString(command.kind, `${id}.kind`);
  const capability = requiredString(command.capability, `${id}.capability`);
  const action = requiredString(command.action, `${id}.action`);
  const input = requiredString(command.input, `${id}.input`);
  const output = requiredString(command.output, `${id}.output`);
  const adapters = requiredStringArray(command.adapters, `${id}.adapters`);
  if (!/^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+$/.test(id)) {
    throw new Error(`Invalid command id: ${id}`);
  }
  assertOneOf(kind, ["query", "command", "operation", "subscription"], `${id}.kind`);
  assertOneOf(action, ["read", "create", "update", "delete", "execute", "share", "manage"], `${id}.action`);
  for (const adapter of adapters) {
    assertOneOf(adapter, ["tauri", "rest", "mcp"], `${id}.adapters`);
  }
  const audit = requiredString(command.audit ?? defaults.audit, `${id}.audit`);
  const auth = requiredString(command.auth ?? defaults.auth, `${id}.auth`);
  const idempotency = requiredString(
    command.idempotency ?? defaults.idempotency,
    `${id}.idempotency`,
  );
  assertOneOf(audit, ["required", "metadata-only"], `${id}.audit`);
  assertOneOf(auth, ["required", "optional-local", "public"], `${id}.auth`);
  assertOneOf(
    idempotency,
    ["none", "required", "required-for-command-and-long-running-operation"],
    `${id}.idempotency`,
  );
  return {
    id,
    kind,
    capability,
    action,
    adapters,
    input,
    output,
    timeoutMs: numberValue(
      command.timeoutMs ?? defaults.timeoutMs,
      `${id}.timeoutMs`,
    ),
    audit,
    auth,
    idempotency,
    events: stringArray(command.events),
    preconditions: stringArray(command.preconditions),
  };
}

function assertOneOf(value, allowed, path) {
  if (!allowed.includes(value)) {
    throw new Error(`Invalid value at ${path}: ${value}`);
  }
}

function requiredString(value, path) {
  if (typeof value !== "string" || value.length === 0)
    throw new Error(`Missing string: ${path}`);
  return value;
}

function requiredStringArray(value, path) {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    throw new Error(`Expected string array: ${path}`);
  }
  return [...value];
}

function stringArray(value) {
  return value === undefined ? [] : requiredStringArray(value, "array");
}

function numberValue(value, path) {
  if (!Number.isInteger(value) || value < 0)
    throw new Error(`Expected non-negative integer: ${path}`);
  return value;
}

function renderRust(commands, sourceHash, mcpInputSchemas) {
  const enumVariant = (value) =>
    value
      .split("-")
      .map((part) => part[0].toUpperCase() + part.slice(1))
      .join("");
  const rows = commands
    .map(
      (command) => `    meta(
        ${JSON.stringify(command.id)},
        CommandKind::${enumVariant(command.kind)},
        ${JSON.stringify(command.capability)},
        Action::${enumVariant(command.action)},
        CommandSchema::${command.input},
        CommandSchema::${command.output},
        &[${command.adapters.map((adapter) => `Adapter::${enumVariant(adapter)}`).join(", ")}],
        ${command.audit === "required"},
        ${command.auth === "required"},
        ${command.idempotency === "required"},
        ${command.timeoutMs.toLocaleString("en-US").replaceAll(",", "_")},
    ),`,
    )
    .join("\n");
  const inputSchemaArms = Object.entries(mcpInputSchemas)
    .map(([name, schema]) => {
      const serialized = JSON.stringify(schema);
      const hashes = [...serialized.matchAll(/"#+/g)].map((match) => match[0].length);
      const delimiter = "#".repeat(Math.max(1, ...hashes));
      return `        CommandSchema::${name} => r${delimiter}\"${serialized}\"${delimiter},`;
    })
    .join("\n");
  return `// @generated by scripts/generate-api.mjs; do not edit.
// Source SHA-256: ${sourceHash}

pub const COMMAND_REGISTRY_SOURCE_SHA256: &str =
    "${sourceHash}";

pub(crate) const COMMANDS: &[CommandMetadata] = &[
${rows}
];

pub(crate) fn mcp_input_schema(schema: CommandSchema) -> Option<serde_json::Value> {
    let raw = match schema {
${inputSchemaArms}
        _ => return None,
    };
    serde_json::from_str(raw).ok()
}
`;
}

function makeMcpInputSchema(name, components) {
  if (!components || !Object.hasOwn(components, name)) {
    throw new Error(`MCP command input schema is missing from OpenAPI: ${name}`);
  }
  const definitions = {};
  const visit = (schema) => {
    if (Array.isArray(schema)) return schema.map(visit);
    if (!schema || typeof schema !== "object") return schema;
    if (typeof schema.$ref === "string") {
      const match = schema.$ref.match(/^#\/components\/schemas\/([^/]+)$/);
      if (!match) throw new Error(`Unsupported MCP input schema reference: ${schema.$ref}`);
      const target = match[1];
      if (!Object.hasOwn(components, target)) throw new Error(`Unknown OpenAPI schema reference: ${schema.$ref}`);
      if (!Object.hasOwn(definitions, target)) {
        definitions[target] = {};
        definitions[target] = visit(components[target]);
      }
      return { ...schema, $ref: `#/$defs/${target}` };
    }
    return Object.fromEntries(Object.entries(schema).map(([key, value]) => [key, visit(value)]));
  };
  const root = visit({ $ref: `#/components/schemas/${name}` });
  return { ...root, $defs: definitions };
}

function render({ registryId, version, sourceHash, openApiHash, profileTypes, commands }) {
  const quote = (value) => JSON.stringify(value);
  const array = (values) => `[${values.map(quote).join(", ")}]`;
  const metadata = commands
    .map(
      (command) => `  ${quote(command.id)}: {
    id: ${quote(command.id)},
    kind: ${quote(command.kind)},
    capability: ${quote(command.capability)},
    action: ${quote(command.action)},
    adapters: ${array(command.adapters)},
    input: ${quote(command.input)},
    output: ${quote(command.output)},
    timeoutMs: ${command.timeoutMs},
    audit: ${quote(command.audit)},
    auth: ${quote(command.auth)},
    idempotency: ${quote(command.idempotency)},
    events: ${array(command.events)},
    preconditions: ${array(command.preconditions)},
  },`,
    )
    .join("\n");
  const idUnion = commands
    .map((command) => `  | ${quote(command.id)}`)
    .join("\n");
  return `// @generated by scripts/generate-api.mjs; do not edit.
// Source: ${registryId} v${version}; SHA-256: ${sourceHash}
// OpenAPI source SHA-256: ${openApiHash}

export const COMMAND_REGISTRY_ID = ${quote(registryId)} as const;
export const COMMAND_REGISTRY_VERSION = ${quote(version)} as const;
export const COMMAND_REGISTRY_SOURCE_SHA256 =
  ${quote(sourceHash)} as const;

export type CommandId =
${idUnion};
export type CommandKind = "query" | "command" | "operation" | "subscription";

${profileTypes.join("\n\n")}

export type CommandMetadata = {
  readonly id: CommandId;
  readonly kind: CommandKind;
  readonly capability: string;
  readonly action: "read" | "create" | "update" | "delete" | "execute" | "share" | "manage";
  readonly adapters: readonly string[];
  readonly input: string;
  readonly output: string;
  readonly timeoutMs: number;
  readonly audit: string;
  readonly auth: string;
  readonly idempotency: string;
  readonly events: readonly string[];
  readonly preconditions: readonly string[];
};

export const COMMANDS = {
${metadata}
} as const satisfies Readonly<Record<CommandId, CommandMetadata>>;

export const COMMAND_IDS = Object.keys(COMMANDS) as CommandId[];

export function getCommandMetadata(id: CommandId): CommandMetadata {
  return COMMANDS[id];
}
`;
}
