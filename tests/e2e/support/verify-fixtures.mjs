import { readFile } from "node:fs/promises";
import { connect } from "node:net";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const manifestPath = resolve(root, "fixtures", "manifest.json");
const forbidden =
  /(?:production|\.tmp|real[_-]?credential|live[_-]?credential|prod[_-]?secret)/i;
const idPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const fail = (message) => {
  throw new Error(`[fixture-verification] ${message}`);
};
const walk = (value, path = "$") => {
  if (typeof value === "string" && forbidden.test(value))
    fail(`forbidden payload at ${path}`);
  if (Array.isArray(value))
    value.forEach((item, index) => walk(item, `${path}[${index}]`));
  else if (value && typeof value === "object")
    Object.entries(value).forEach(([key, item]) =>
      walk(item, `${path}.${key}`),
    );
};
const uniqueIds = (manifest) => {
  const records = [
    "users",
    "workspaces",
    "teams",
    "proxies",
    "profiles",
    "legacyImports",
  ].flatMap((key) => manifest[key]);
  const ids = records.map((record) => record.id);
  if (new Set(ids).size !== ids.length) fail("duplicate fixture ID");
  records.forEach((record) => {
    if (!idPattern.test(record.id)) fail(`non-deterministic ID: ${record.id}`);
  });
};
const references = (manifest) => {
  const ids = new Set(
    [
      "users",
      "workspaces",
      "teams",
      "proxies",
      "profiles",
      "legacyImports",
    ].flatMap((key) => manifest[key].map(({ id }) => id)),
  );
  const refs = [
    ...manifest.workspaces.map(({ ownerId }) => ownerId),
    ...manifest.teams.flatMap(({ workspaceId, memberIds }) => [
      workspaceId,
      ...memberIds,
    ]),
    ...manifest.profiles.flatMap(
      ({ workspaceId, teamId, ownerId, proxyId }) => [
        workspaceId,
        teamId,
        ownerId,
        proxyId,
      ],
    ),
    ...manifest.legacyImports.map(({ profileId }) => profileId),
  ];
  refs.forEach((id) => {
    if (!ids.has(id)) fail(`dangling reference: ${id}`);
  });
};
const checkHttp = async (name, url, failures) => {
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(1500) });
    if (!response.ok)
      failures.push(`${name} unavailable at ${url} (HTTP ${response.status})`);
  } catch (error) {
    failures.push(
      `${name} unavailable at ${url} (${error instanceof Error ? error.message : String(error)})`,
    );
  }
};
const checkTcp = (name, host, port, failures) =>
  new Promise((done) => {
    const socket = connect({ host, port: Number(port), timeout: 1500 });
    socket.once("connect", () => {
      socket.destroy();
      done();
    });
    socket.once("timeout", () => {
      failures.push(`${name} unavailable at ${host}:${port} (timeout)`);
      socket.destroy();
      done();
    });
    socket.once("error", (error) => {
      failures.push(
        `${name} unavailable at ${host}:${port} (${error.message})`,
      );
      done();
    });
  });
const checkServices = async () => {
  const failures = [];
  await Promise.all([
    checkHttp(
      "control",
      process.env.BUGLOGIN_E2E_CONTROL_URL ?? "http://127.0.0.1:4310/v1/health",
      failures,
    ),
    checkTcp(
      "postgres",
      process.env.BUGLOGIN_E2E_POSTGRES_HOST ?? "127.0.0.1",
      process.env.BUGLOGIN_E2E_POSTGRES_PORT ?? "54321",
      failures,
    ),
    checkTcp(
      "redis",
      process.env.BUGLOGIN_E2E_REDIS_HOST ?? "127.0.0.1",
      process.env.BUGLOGIN_E2E_REDIS_PORT ?? "56379",
      failures,
    ),
    checkHttp(
      "object-storage",
      process.env.BUGLOGIN_E2E_S3_HEALTH_URL ??
        "http://127.0.0.1:59000/minio/health/live",
      failures,
    ),
  ]);
  if (failures.length) fail(failures.join("; "));
};
const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
if (manifest.source !== "synthetic") fail("manifest source must be synthetic");
walk(manifest);
uniqueIds(manifest);
references(manifest);
if (process.argv.includes("--services")) await checkServices();
console.log(`[fixture-verification] PASS: ${manifest.fixtureSet}`);
