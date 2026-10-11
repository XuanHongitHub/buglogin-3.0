import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parse, stringify } from "yaml";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = resolve(root, "docs/contracts/command-registry-v1.yaml");
const frontendOutputPath = resolve(root, "frontend/src/generated/command-registry.ts");
const adapterOutputPath = resolve(root, "docs/contracts/adapter-contract-v1.yaml");
const rustOutputPath = resolve(root, "crates/api/src/commands/generated.rs");
const checkOnly = process.argv.includes("--check") || process.argv.includes("--check-rust");
const rustOnly = process.argv.includes("--rust-only") || process.argv.includes("--check-rust");

const source = await readFile(sourcePath, "utf8");
const registry = parse(source);

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

const sourceHash = createHash("sha256").update(source).digest("hex");
const frontendOutput = render({
  registryId: requiredString(registry.registryId, "registryId"),
  version: requiredString(registry.version, "version"),
  sourceHash,
  commands,
});
const adapterOutput = renderAdapters(commands, sourceHash);
const rustOutput = renderRust(commands, sourceHash);

if (checkOnly) {
  if (!rustOnly) {
    await checkGenerated(frontendOutputPath, frontendOutput);
    await checkGenerated(adapterOutputPath, adapterOutput);
  }
  await checkGenerated(rustOutputPath, rustOutput);
} else {
  if (!rustOnly) {
    await mkdir(dirname(frontendOutputPath), { recursive: true });
    await writeFile(frontendOutputPath, frontendOutput, "utf8");
    await writeFile(adapterOutputPath, adapterOutput, "utf8");
  }
  await writeFile(rustOutputPath, rustOutput, "utf8");
}

async function checkGenerated(path, expected) {
  const existing = await readFile(path, "utf8").catch(() => null);
  if (existing !== expected) {
    console.error(`Generated file is stale: ${path}`);
    process.exitCode = 1;
  }
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
          command.auth === "optional-local"
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
          command.auth === "optional-local"
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
          command.auth === "optional-local"
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
  assertOneOf(action, ["read", "create", "update", "delete", "execute", "manage"], `${id}.action`);
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
  assertOneOf(auth, ["required", "optional-local"], `${id}.auth`);
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

function renderRust(commands, sourceHash) {
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
  return `// @generated by scripts/generate-api.mjs; do not edit.
// Source SHA-256: ${sourceHash}

pub const COMMAND_REGISTRY_SOURCE_SHA256: &str =
    "${sourceHash}";

pub(crate) const COMMANDS: &[CommandMetadata] = &[
${rows}
];
`;
}

function render({ registryId, version, sourceHash, commands }) {
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

export const COMMAND_REGISTRY_ID = ${quote(registryId)} as const;
export const COMMAND_REGISTRY_VERSION = ${quote(version)} as const;
export const COMMAND_REGISTRY_SOURCE_SHA256 =
  ${quote(sourceHash)} as const;

export type CommandId =
${idUnion};
export type CommandKind = "query" | "command" | "operation" | "subscription";

export type CommandMetadata = {
  readonly id: CommandId;
  readonly kind: CommandKind;
  readonly capability: string;
  readonly action: "read" | "create" | "update" | "delete" | "execute" | "manage";
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
