import { spawn } from "node:child_process";
import { connect as tcpConnect } from "node:net";
import { connect as tlsConnect } from "node:tls";
import { basename, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const SERVICES = ["postgres", "redis", "s3"],
  TRUE = new Set(["1", "true", "yes", "on"]);
export function redact(value, secrets = []) {
  let out = String(value ?? "");
  for (const s of [
    ...new Set(secrets.filter((x) => typeof x === "string" && x.length >= 3)),
  ].sort((a, b) => b.length - a.length))
    out = out.split(s).join("[REDACTED]");
  return out
    .replace(/([a-z][a-z0-9+.-]*:\/\/[^\s:@/]+:)[^\s@/]+@/gi, "$1[REDACTED]@")
    .replace(
      /([?&](?:access_token|api_key|key|password|secret|signature|token)=)[^&\s]+/gi,
      "$1[REDACTED]",
    )
    .replace(/\b(Bearer\s+)[A-Za-z0-9._~+/=-]+/gi, "$1[REDACTED]");
}
export function validateConfig(
  env = process.env,
  args = process.argv.slice(2),
) {
  return {
    strict:
      args.includes("--strict") ||
      TRUE.has(String(env.BUGLOGIN_LIVE_INFRA_STRICT ?? "").toLowerCase()),
    timeoutMs: timeout(env.BUGLOGIN_LIVE_INFRA_TIMEOUT_MS),
    postgres: url("postgres", env.BUGLOGIN_LIVE_POSTGRES_URL, [
      "postgres:",
      "postgresql:",
    ]),
    redis: url("redis", env.BUGLOGIN_LIVE_REDIS_URL, ["redis:", "rediss:"]),
    s3: url("s3", env.BUGLOGIN_LIVE_S3_PROBE_URL, ["http:", "https:"]),
    bins: {
      psql: env.BUGLOGIN_LIVE_PSQL_BIN || "psql",
      redisCli: env.BUGLOGIN_LIVE_REDIS_CLI_BIN || "redis-cli",
    },
  };
}
export function exitCodeFor(results, strict) {
  return results.some((x) => x.status === "blocked") ||
    (strict && results.some((x) => x.status !== "pass"))
    ? 1
    : 0;
}
export async function runLiveInfra({
  env = process.env,
  args = process.argv.slice(2),
} = {}) {
  const config = validateConfig(env, args),
    secrets = secretValues(config),
    results = await Promise.all([
      postgres(config, env, secrets),
      redis(config, env, secrets),
      s3(config, secrets),
    ]);
  const evidence = {
    schema: "buglogin.live-infra-evidence/1",
    generatedAt: new Date().toISOString(),
    strict: config.strict,
    results,
    summary: Object.fromEntries(
      ["pass", "skipped", "blocked"].map((s) => [
        s,
        results.filter((x) => x.status === s).length,
      ]),
    ),
  };
  return { evidence, exitCode: exitCodeFor(results, config.strict) };
}
function timeout(value) {
  if (value === undefined || value === "") return 5000;
  const n = Number(value);
  if (!Number.isInteger(n) || n < 100 || n > 60000)
    throw Error(
      "BUGLOGIN_LIVE_INFRA_TIMEOUT_MS must be an integer between 100 and 60000",
    );
  return n;
}
function url(service, value, protocols) {
  if (!value) {
    const key =
      service === "s3" ? "S3_PROBE_URL" : `${service.toUpperCase()}_URL`;
    return { status: "skipped", reason: `BUGLOGIN_LIVE_${key} is missing` };
  }
  try {
    const parsed = new URL(value);
    if (!protocols.includes(parsed.protocol))
      return {
        status: "blocked",
        reason: `${service} URL protocol must be ${protocols.join(" or ")}`,
      };
    if (!parsed.hostname)
      return {
        status: "blocked",
        reason: `${service} URL must include a host`,
      };
    if (service === "postgres" && (!parsed.pathname || parsed.pathname === "/"))
      return {
        status: "blocked",
        reason: "postgres URL must include a database",
      };
    return { status: "ready", url: parsed };
  } catch {
    return { status: "blocked", reason: `${service} URL is invalid` };
  }
}
function secretValues(config) {
  return SERVICES.flatMap((s) => {
    const x = config[s];
    return x.status !== "ready"
      ? []
      : [
          x.url.href,
          x.url.username,
          x.url.password,
          ...x.url.searchParams.values(),
        ];
  });
}
async function postgres(config, baseEnv, secrets) {
  if (config.postgres.status !== "ready")
    return staticResult("postgres", config.postgres);
  const u = config.postgres.url,
    r = await exec(
      config.bins.psql,
      ["-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-c", "SELECT 1"],
      {
        timeoutMs: config.timeoutMs,
        secrets,
        env: {
          ...baseEnv,
          PGHOST: u.hostname,
          PGPORT: u.port || "5432",
          PGDATABASE: decodeURIComponent(u.pathname.slice(1)),
          PGUSER: decodeURIComponent(u.username),
          PGPASSWORD: decodeURIComponent(u.password),
          ...(u.searchParams.get("sslmode")
            ? { PGSSLMODE: u.searchParams.get("sslmode") }
            : {}),
        },
      },
    );
  if (r.kind === "missing")
    return skipped(
      "postgres",
      `${basename(config.bins.psql)} executable is missing`,
    );
  return !r.ok || r.stdout.trim() !== "1"
    ? blocked(
        "postgres",
        r.detail || "SELECT 1 returned unexpected evidence",
        r.durationMs,
      )
    : passed("postgres", "SELECT 1", r.durationMs);
}
async function redis(config, baseEnv, secrets) {
  if (config.redis.status !== "ready")
    return staticResult("redis", config.redis);
  const u = config.redis.url,
    db = u.pathname.slice(1) || "0";
  if (!/^\d+$/.test(db))
    return blocked("redis", "redis URL database must be numeric", 0);
  const args = [
    "-h",
    u.hostname,
    "-p",
    u.port || (u.protocol === "rediss:" ? "6380" : "6379"),
    "-n",
    db,
  ];
  if (u.username) args.push("--user", decodeURIComponent(u.username));
  if (u.protocol === "rediss:") args.push("--tls");
  args.push("PING");
  const r = await exec(config.bins.redisCli, args, {
    timeoutMs: config.timeoutMs,
    secrets,
    env: { ...baseEnv, REDISCLI_AUTH: decodeURIComponent(u.password) },
  });
  const socketResult =
    r.kind === "missing"
      ? await redisPingSocket(u, config.timeoutMs, secrets)
      : null;
  const evidence = socketResult ?? r;
  if (evidence.kind === "missing")
    return skipped("redis", "redis probe executable and socket probe unavailable");
  return !evidence.ok || evidence.stdout.trim().toUpperCase() !== "+PONG"
    ? blocked(
        "redis",
        r.detail || "PING returned unexpected evidence",
        r.durationMs,
      )
    : passed("redis", "PING/PONG", r.durationMs);
}
function redisPingSocket(url, timeoutMs, secrets) {
  return new Promise((resolve) => {
    const started = Date.now();
    const socket = url.protocol === "rediss:"
      ? tlsConnect({ host: url.hostname, port: url.port || 6380, rejectUnauthorized: true })
      : tcpConnect({ host: url.hostname, port: url.port || 6379 });
    let output = "";
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      resolve({ ...result, durationMs: Date.now() - started });
    };
    const timer = setTimeout(() => finish({
      kind: "timeout",
      ok: false,
      stdout: "",
      detail: `timed out after ${timeoutMs}ms`,
    }), timeoutMs);
    socket.setEncoding("utf8");
    socket.on("connect", () => {
      const password = url.password ? decodeURIComponent(url.password) : "";
      const auth = password
        ? `*2\r\n$4\r\nAUTH\r\n$${Buffer.byteLength(password)}\r\n${password}\r\n`
        : "";
      socket.write(`${auth}*1\r\n$4\r\nPING\r\n`);
    });
    socket.on("data", (chunk) => {
      output += chunk;
      if (output.includes("+PONG")) finish({ kind: "socket", ok: true, stdout: "+PONG", detail: "" });
      else if (output.startsWith("-")) finish({ kind: "socket", ok: false, stdout: "", detail: redact(output.trim(), secrets) });
    });
    socket.on("error", (error) => finish({ kind: "socket", ok: false, stdout: "", detail: redact(error.message, secrets) }));
    socket.on("close", () => {
      if (!settled) finish({ kind: "socket", ok: false, stdout: "", detail: "redis socket closed before PONG" });
    });
  });
}
async function s3(config, secrets) {
  if (config.s3.status !== "ready") return staticResult("s3", config.s3);
  const started = Date.now();
  try {
    const response = await fetch(config.s3.url, {
      method: "GET",
      redirect: "manual",
      signal: AbortSignal.timeout(config.timeoutMs),
    });
    if (!response.ok)
      return blocked(
        "s3",
        `HTTP probe returned ${response.status}`,
        Date.now() - started,
      );
    await response.body?.cancel();
    return passed("s3", `HTTP ${response.status}`, Date.now() - started);
  } catch (error) {
    return blocked(
      "s3",
      redact(error instanceof Error ? error.message : String(error), secrets),
      Date.now() - started,
    );
  }
}
function exec(command, args, { env, timeoutMs, secrets }) {
  return new Promise((done) => {
    const started = Date.now();
    let stdout = "",
      stderr = "",
      settled = false,
      timer;
    const child = spawn(command, args, {
        env,
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
      }),
      finish = (r) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        done({ ...r, durationMs: Date.now() - started });
      };
    child.stdout.setEncoding("utf8").on("data", (x) => (stdout += x));
    child.stderr.setEncoding("utf8").on("data", (x) => (stderr += x));
    child.once("error", (e) =>
      finish(
        e.code === "ENOENT"
          ? {
              kind: "missing",
              ok: false,
              stdout: "",
              detail: "executable missing",
            }
          : {
              kind: "error",
              ok: false,
              stdout: "",
              detail: redact(e.message, secrets),
            },
      ),
    );
    child.once("close", (code) =>
      finish({
        kind: "exit",
        ok: code === 0,
        stdout: redact(stdout, secrets),
        detail:
          code === 0
            ? ""
            : redact(stderr.trim() || `exited with code ${code}`, secrets),
      }),
    );
    timer = setTimeout(() => {
      child.kill();
      finish({
        kind: "timeout",
        ok: false,
        stdout: "",
        detail: `timed out after ${timeoutMs}ms`,
      });
    }, timeoutMs);
  });
}
function staticResult(service, c) {
  return c.status === "blocked"
    ? blocked(service, c.reason, 0)
    : skipped(service, c.reason);
}
function passed(service, evidence, durationMs) {
  return { service, status: "pass", evidence, durationMs };
}
function skipped(service, reason) {
  return { service, status: "skipped", reason };
}
function blocked(service, reason, durationMs) {
  return { service, status: "blocked", reason, durationMs };
}
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  try {
    const { evidence, exitCode } = await runLiveInfra();
    console.log(JSON.stringify(evidence, null, 2));
    process.exitCode = exitCode;
  } catch (error) {
    console.error(
      JSON.stringify(
        {
          schema: "buglogin.live-infra-evidence/1",
          strict: process.argv.includes("--strict"),
          results: SERVICES.map((s) =>
            blocked(
              s,
              redact(error instanceof Error ? error.message : String(error)),
              0,
            ),
          ),
        },
        null,
        2,
      ),
    );
    process.exitCode = 1;
  }
}
