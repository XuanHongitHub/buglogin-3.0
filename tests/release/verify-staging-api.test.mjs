import assert from "node:assert/strict";
import test from "node:test";
import { compareOpenApi, verifyStagingApi } from "../../scripts/release/verify-staging-api.mjs";

const spec = (operationIds, paths = ["/v1/health", "/v1/profiles"]) =>
  Buffer.from(
    [
      "openapi: 3.1.0",
      "paths:",
      ...paths.flatMap((path, index) => [
        `  ${path}:`,
        `    get:`,
        `      operationId: ${operationIds[index] ?? `operation.${index}`}`,
      ]),
    ].join("\n"),
  );

test("staging OpenAPI comparison reports missing and unexpected operations", () => {
  const local = spec(["profile.list", "profile.stop"]);
  const remote = spec(["profile.list", "legacy.profile.get"]);
  const report = compareOpenApi(local, remote);

  assert.equal(report.matches, false);
  assert.deepEqual(report.missingOperations, ["profile.stop"]);
  assert.deepEqual(report.unexpectedOperations, ["legacy.profile.get"]);
});

test("staging verification checks readiness before fetching the contract", async () => {
  const calls = [];
  const fetchImpl = async (url, options) => {
    calls.push({ url, headers: options.headers });
    return Response.json({ code: "runtime_catalog_unavailable" }, { status: 503 });
  };

  await assert.rejects(
    verifyStagingApi({ baseUrl: "https://staging.example/", fetchImpl, localBytes: spec([]) }),
    /GET \/v1\/readyz returned 503 \(runtime_catalog_unavailable\)/,
  );
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, "https://staging.example/v1/readyz");
  assert.match(calls[0].headers["x-correlation-id"], /^release-verify-/);
});

test("staging verification reports browser catalog readiness separately from API readiness", async () => {
  const calls = [];
  const openApi = spec([]);
  const fetchImpl = async (url, options) => {
    calls.push({ url, headers: options.headers });
    if (url.endsWith("/v1/readyz")) {
      return Response.json({
        status: "ready",
        service: "buglogin-api",
      }, {
        headers: { "x-buglogin-runtime-catalog": "unavailable" },
      });
    }
    return new Response(openApi, { status: 200 });
  };

  const report = await verifyStagingApi({
    baseUrl: "https://staging.example",
    fetchImpl,
    localBytes: openApi,
  });

  assert.equal(report.readinessValid, false);
  assert.deepEqual(report.readinessIssues, [
    "Signed runtime catalog is unavailable; production readiness requires it",
  ]);
  assert.equal(report.readiness.runtimeCatalog, "unavailable");
  assert.equal(report.matches, true);
  assert.deepEqual(calls.map((call) => call.url), [
    "https://staging.example/v1/readyz",
    "https://staging.example/v1/openapi.yaml",
  ]);
  assert.equal(
    calls[0].headers["x-correlation-id"],
    calls[1].headers["x-correlation-id"],
  );
});

test("staging verification reports OpenAPI drift when readiness headers are incomplete", async () => {
  const calls = [];
  const localOpenApi = spec(["auth.desktop.login", "runtime.catalog.get"]);
  const remoteOpenApi = spec(["auth.desktop.login", "profile.list"]);
  const fetchImpl = async (url, options) => {
    calls.push(url);
    if (url.endsWith("/v1/readyz")) {
      return Response.json({ status: "ready", service: "buglogin-api" });
    }
    return new Response(remoteOpenApi, { status: 200 });
  };

  const report = await verifyStagingApi({
    baseUrl: "https://staging.example",
    fetchImpl,
    localBytes: localOpenApi,
  });

  assert.equal(report.readinessValid, false);
  assert.deepEqual(report.readinessIssues, [
    "Readiness response is missing a valid X-BugLogin-Runtime-Catalog header",
  ]);
  assert.equal(report.matches, false);
  assert.deepEqual(report.missingOperations, ["runtime.catalog.get"]);
  assert.deepEqual(report.unexpectedOperations, ["profile.list"]);
  assert.deepEqual(report.missingCoreOperations, ["runtime.catalog.get"]);
  assert.deepEqual(calls, [
    "https://staging.example/v1/readyz",
    "https://staging.example/v1/openapi.yaml",
  ]);
});

test("staging verification identifies missing desktop login and browser catalog routes", async () => {
  const localOpenApi = spec(["auth.desktop.login", "runtime.catalog.get"]);
  const remoteOpenApi = spec(["legacy.auth.login", "profile.list"]);
  const fetchImpl = async (url) => url.endsWith("/v1/readyz")
    ? Response.json({ status: "ready", service: "buglogin-api" }, {
      headers: { "x-buglogin-runtime-catalog": "available" },
    })
    : new Response(remoteOpenApi, { status: 200 });

  const report = await verifyStagingApi({
    baseUrl: "https://staging.example",
    fetchImpl,
    localBytes: localOpenApi,
  });

  assert.deepEqual(report.missingCoreOperations, [
    "auth.desktop.login",
    "runtime.catalog.get",
  ]);
  assert.equal(report.matches, false);
});
