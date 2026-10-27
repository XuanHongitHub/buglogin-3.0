#!/usr/bin/env node
import { performance } from "node:perf_hooks";

function positiveInteger(name, fallback, maximum) {
  const raw = process.env[name];
  const value = raw === undefined ? fallback : Number(raw);
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum)
    throw new Error(`${name} must be an integer from 1 to ${maximum}`);
  return value;
}

const baseUrl = process.env.BUGLOGIN_API_BASE_URL?.replace(/\/+$/, "");
const token = process.env.BUGLOGIN_BEARER_TOKEN;
const workspaceId = process.env.BUGLOGIN_WORKSPACE_ID;
const profileIds = (process.env.BUGLOGIN_PROFILE_IDS ?? "")
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);
const requestCount = positiveInteger("BUGLOGIN_STATUS_REQUESTS", 100, 100_000);
const concurrency = positiveInteger("BUGLOGIN_STATUS_CONCURRENCY", 10, 250);
const p95LimitMs = positiveInteger("BUGLOGIN_STATUS_P95_LIMIT_MS", 250, 60_000);
const p99LimitMs = positiveInteger("BUGLOGIN_STATUS_P99_LIMIT_MS", 750, 60_000);

if (!baseUrl || !token || !workspaceId || !profileIds.length)
  throw new Error(
    "Set BUGLOGIN_API_BASE_URL, BUGLOGIN_BEARER_TOKEN, BUGLOGIN_WORKSPACE_ID, and BUGLOGIN_PROFILE_IDS (comma-separated, 1–250 profile IDs).",
  );
if (profileIds.length > 250)
  throw new Error("BUGLOGIN_PROFILE_IDS cannot contain more than 250 IDs");
const endpoint = new URL(
  `/v1/workspaces/${encodeURIComponent(workspaceId)}/profile-runtime-status`,
  `${baseUrl}/`,
);
for (const profileId of profileIds)
  endpoint.searchParams.append("profileId", profileId);

const durations = [];
const failures = [];
let nextRequest = 0;
const runStarted = performance.now();
async function worker() {
  while (true) {
    const requestIndex = nextRequest++;
    if (requestIndex >= requestCount) return;
    const started = performance.now();
    try {
      const response = await fetch(endpoint, {
        headers: {
          authorization: `Bearer ${token}`,
          "x-correlation-id": `profile-status-perf-${requestIndex}`,
        },
        signal: AbortSignal.timeout(15_000),
      });
      if (!response.ok) {
        durations.push(performance.now() - started);
        failures.push({ index: requestIndex, status: response.status });
        await response.body?.cancel();
        continue;
      }
      const body = await response.json();
      durations.push(performance.now() - started);
      if (!Array.isArray(body))
        failures.push({ index: requestIndex, status: "invalid_response" });
    } catch (error) {
      durations.push(performance.now() - started);
      failures.push({
        index: requestIndex,
        status: error?.name === "TimeoutError" ? "timeout" : "request_failed",
      });
    }
  }
}

await Promise.all(
  Array.from({ length: Math.min(concurrency, requestCount) }, () => worker()),
);
durations.sort((left, right) => left - right);
const percentile = (fraction) =>
  durations.length
    ? Math.round(durations[Math.min(durations.length - 1, Math.ceil(durations.length * fraction) - 1)])
    : null;
const summary = {
  route: "GET /v1/workspaces/{workspaceId}/profile-runtime-status",
  requests: requestCount,
  concurrency: Math.min(concurrency, requestCount),
  profilesPerRequest: profileIds.length,
  elapsedMs: Math.round(performance.now() - runStarted),
  latencyMs: {
    p50: percentile(0.5),
    p95: percentile(0.95),
    p99: percentile(0.99),
    max: durations.length ? Math.round(durations.at(-1)) : null,
  },
  latencyLimitsMs: { p95: p95LimitMs, p99: p99LimitMs },
  failures: failures.length,
  failureStatuses: failures.reduce((counts, item) => {
    const key = String(item.status);
    counts[key] = (counts[key] ?? 0) + 1;
    return counts;
  }, {}),
};
console.log(JSON.stringify(summary, null, 2));
if (
  failures.length ||
  (summary.latencyMs.p95 !== null && summary.latencyMs.p95 > p95LimitMs) ||
  (summary.latencyMs.p99 !== null && summary.latencyMs.p99 > p99LimitMs)
)
  process.exitCode = 1;
