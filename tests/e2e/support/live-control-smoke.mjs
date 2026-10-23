import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { resolveBuildTargetDir } from "../../../scripts/build-target.mjs";

const execFileAsync = promisify(execFile);
const root = resolve(import.meta.dirname, "../../..");
const port = 44000 + (process.pid % 1000);
const baseUrl = `http://127.0.0.1:${port}`;
const dataDir = await mkdtemp(join(tmpdir(), "buglogin-live-control-"));
const targetDir = resolveBuildTargetDir(process.env.CARGO_TARGET_DIR);
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const binary = join(
  targetDir,
  "debug",
  process.platform === "win32" ? "buglogin-control.exe" : "buglogin-control",
);
const env = {
  ...process.env,
  CARGO_TARGET_DIR: targetDir,
  BUGLOGIN_CONTROL_PORT: String(port),
  BUGLOGIN_DATA_DIR: dataDir,
};

await execFileAsync(
  cargo,
  ["build", "-q", "-p", "buglogin-control-app", "--bin", "buglogin-control"],
  {
    cwd: root,
    env,
    timeout: 180_000,
  },
);
const child = spawn(binary, [], {
  cwd: root,
  env,
  stdio: ["ignore", "pipe", "pipe"],
});
let output = "";
child.stdout.on("data", (chunk) => {
  output += chunk;
});
child.stderr.on("data", (chunk) => {
  output += chunk;
});

const waitForControl = async () => {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (child.exitCode !== null)
      throw new Error(`control exited ${child.exitCode}: ${output}`);
    try {
      const response = await fetch(`${baseUrl}/health`, {
        signal: AbortSignal.timeout(500),
      });
      if (response.ok) return;
    } catch {}
    await new Promise((done) => setTimeout(done, 250));
  }
  throw new Error(`control did not start: ${output}`);
};

test.after(async () => {
  child.kill();
  await Promise.race([
    new Promise((done) => child.once("exit", done)),
    new Promise((done) => setTimeout(done, 3000)),
  ]);
  await rm(dataDir, { recursive: true, force: true });
});

await waitForControl();

test("live control exposes health and protects API routes", async () => {
  const health = await fetch(`${baseUrl}/health`);
  assert.equal(health.status, 200);
  const healthBody = await health.json();
  assert.equal(healthBody.status, "ok");
  assert.equal(healthBody.service, "buglogin-control");
  assert.match(healthBody.version, /^\d+\.\d+\.\d+/);

  const missingCorrelation = await fetch(`${baseUrl}/v1/account`, {
    headers: { authorization: "Bearer live-control-secret-canary" },
  });
  assert.equal(missingCorrelation.status, 400);

  const unauthenticated = await fetch(`${baseUrl}/v1/account`, {
    headers: { "x-correlation-id": "live-control-smoke" },
  });
  assert.equal(unauthenticated.status, 401);
  const body = await unauthenticated.text();
  assert.match(body, /authentication_required/);
  assert.doesNotMatch(body, /live-control-secret-canary|authorization|bearer/i);
});
