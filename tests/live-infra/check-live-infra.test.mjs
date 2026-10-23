import assert from "node:assert/strict";
import test from "node:test";
import { exitCodeFor, redact, validateConfig } from "./check-live-infra.mjs";

test("missing service configuration is explicitly skipped", () => {
  const c = validateConfig({}, []);
  assert.equal(c.strict, false);
  assert.deepEqual(
    [c.postgres.status, c.redis.status, c.s3.status],
    ["skipped", "skipped", "skipped"],
  );
});
test("strict mode fails closed for skipped or blocked services", () => {
  assert.equal(
    exitCodeFor([{ status: "pass" }, { status: "skipped" }], true),
    1,
  );
  assert.equal(
    exitCodeFor([{ status: "pass" }, { status: "blocked" }], false),
    1,
  );
  assert.equal(
    exitCodeFor([{ status: "pass" }, { status: "skipped" }], false),
    0,
  );
  assert.equal(exitCodeFor([{ status: "pass" }, { status: "pass" }], true), 0);
});
test("validation blocks malformed and unsafe service URLs", () => {
  const c = validateConfig({
    BUGLOGIN_LIVE_POSTGRES_URL: "https://db.example.test/main",
    BUGLOGIN_LIVE_REDIS_URL: "redis://cache.example.test/not-a-number",
    BUGLOGIN_LIVE_S3_PROBE_URL: "file:///tmp/minio",
  });
  assert.equal(c.postgres.status, "blocked");
  assert.equal(c.redis.status, "ready");
  assert.equal(c.s3.status, "blocked");
});
test("redaction removes credentials, query secrets, and bearer tokens", () => {
  const input =
    "postgresql://alice:password-canary@db.example.test/main?token=query-canary exact-secret-canary Bearer bearer-canary";
  const output = redact(input, [
    "exact-secret-canary",
    "password-canary",
    "query-canary",
    "bearer-canary",
  ]);
  assert.doesNotMatch(
    output,
    /password-canary|query-canary|exact-secret-canary|bearer-canary/,
  );
  assert.match(output, /\[REDACTED\]/);
});
test("timeout validation rejects ambiguous values", () => {
  assert.throws(
    () => validateConfig({ BUGLOGIN_LIVE_INFRA_TIMEOUT_MS: "0" }),
    /between 100 and 60000/,
  );
  assert.equal(
    validateConfig({ BUGLOGIN_LIVE_INFRA_TIMEOUT_MS: "1500" }).timeoutMs,
    1500,
  );
});
