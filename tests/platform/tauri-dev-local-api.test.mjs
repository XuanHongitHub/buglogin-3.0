import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { parse } from "yaml";
import { resolveBuildTargetDir } from "../../scripts/build-target.mjs";
import {
  findMissingTauriDevOperations,
  hasDurableTauriDevPersistence,
  isRootTauriDevInvocation,
  isForbiddenTauriDevCargoTargetDir,
  parseTauriDevApiBaseUrl,
  validateMcpStatusResponse,
} from "../../scripts/tauri-dev-local-api-contract.mjs";

test("root Tauri dev command is routed through the local API launcher", () => {
  assert.equal(
    isRootTauriDevInvocation("pnpm", ["--dir", "apps/desktop", "exec", "tauri", "dev"]),
    true,
  );
  assert.equal(
    isRootTauriDevInvocation("pnpm", ["--dir", "apps/desktop", "exec", "tauri", "build"]),
    false,
  );
  assert.equal(isRootTauriDevInvocation("pnpm", ["exec", "tauri", "dev"]), false);
});

test("Tauri local API launcher accepts an alternate explicit loopback port", () => {
  assert.equal(
    parseTauriDevApiBaseUrl("http://127.0.0.1:12345"),
    "http://127.0.0.1:12345",
  );
  assert.equal(
    parseTauriDevApiBaseUrl("http://localhost:12346/"),
    "http://localhost:12346",
  );
});

test("Tauri local API launcher rejects remote, credentialed, and path URLs", () => {
  for (const value of [
    "https://api3.buglogin.com",
    "http://192.168.1.10:12345",
    "http://user:password@127.0.0.1:12345",
    "http://127.0.0.1:12345/v1",
    "http://127.0.0.1:12345/?host=other",
  ]) {
    assert.throws(() => parseTauriDevApiBaseUrl(value));
  }
});

test("Tauri local API launcher refuses C:, E:, and F: build caches on Windows", () => {
  assert.equal(isForbiddenTauriDevCargoTargetDir("F:\\buglogin-target", "win32"), true);
  assert.equal(isForbiddenTauriDevCargoTargetDir("C:\\buglogin-target", "win32"), true);
  assert.equal(isForbiddenTauriDevCargoTargetDir("E:\\buglogin-target", "win32"), true);
  assert.equal(isForbiddenTauriDevCargoTargetDir("G:\\buglogin-target", "win32"), false);
  assert.equal(isForbiddenTauriDevCargoTargetDir("\\\\?\\C:\\buglogin-target", "win32"), true);
  assert.equal(isForbiddenTauriDevCargoTargetDir("F:\\buglogin-target", "linux"), false);
});

test("Windows builds share only the canonical G: Cargo target", () => {
  assert.equal(resolveBuildTargetDir(undefined, "win32"), "G:\\buglogin-target");
  assert.equal(
    resolveBuildTargetDir("g:/BUGLOGIN-TARGET/", "win32"),
    "G:\\buglogin-target",
  );
  for (const path of [
    "C:\\buglogin-target",
    "E:\\buglogin-target",
    "F:\\buglogin-target",
    "D:\\buglogin-target",
    "E:\\another-target",
  ]) {
    assert.throws(() => resolveBuildTargetDir(path, "win32"));
  }
  assert.match(
    resolveBuildTargetDir("/tmp/buglogin-target", "linux"),
    /buglogin-target$/,
  );
});

test("Tauri local API launcher accepts PostgreSQL persistence", () => {
  assert.equal(hasDurableTauriDevPersistence("postgres"), true);
});

test("Tauri local API launcher rejects ephemeral or unknown persistence", () => {
  assert.equal(hasDurableTauriDevPersistence("memory"), false);
  assert.equal(hasDurableTauriDevPersistence("custom"), false);
  assert.equal(hasDurableTauriDevPersistence(null), false);
});

test("Tauri local API launcher requires the published MCP status contract", () => {
  assert.equal(
    validateMcpStatusResponse({
      enabled: false,
      endpoint: "/v1/mcp",
      authentication: "scoped-api-token",
    }),
    true,
  );
  assert.equal(validateMcpStatusResponse({ enabled: false }), false);
  assert.equal(
    validateMcpStatusResponse({
      enabled: false,
      endpoint: "/v1/mcp",
      authentication: "none",
    }),
    false,
  );
});

test("Tauri local API launcher accepts the required direct-login and CRUD contract", () => {
  const paths = {
    "/v1/auth/desktop/login": { post: { operationId: "auth.desktop.login" } },
    "/v1/account": { get: { operationId: "account.get" } },
    "/v1/organizations": { get: { operationId: "organization.list" } },
    "/v1/organizations/{organizationId}/workspaces": {
      get: { operationId: "workspace.list" },
      post: { operationId: "workspace.create" },
    },
    "/v1/profiles": {
      get: { operationId: "profile.list" },
      post: { operationId: "profile.create" },
    },
    "/v1/workspaces/{workspaceId}/proxies": {
      get: { operationId: "proxy.list" },
      post: { operationId: "proxy.create" },
    },
    "/v1/workspaces/{workspaceId}/grants/page": {
      get: { operationId: "grant.list.page" },
    },
  };
  assert.deepEqual(findMissingTauriDevOperations({ paths }), []);
});

test("Tauri local API launcher accepts the canonical source OpenAPI contract", async () => {
  const source = await readFile(
    new URL("../../crates/api/openapi/openapi-v1.yaml", import.meta.url),
    "utf8",
  );
  assert.deepEqual(findMissingTauriDevOperations(parse(source)), []);
});

test("Tauri local API launcher reports stale or malformed desktop contracts", () => {
  const stale = {
    paths: {
      "/v1/auth/desktop/login": { post: { operationId: "auth.device.start" } },
      "/v1/workspaces/{workspaceId}/grants/page": {
        get: { operationId: "grant.list" },
      },
    },
  };
  assert.deepEqual(findMissingTauriDevOperations(stale), [
    "POST /v1/auth/desktop/login (auth.desktop.login)",
    "GET /v1/account (account.get)",
    "GET /v1/organizations (organization.list)",
    "GET /v1/organizations/{organizationId}/workspaces (workspace.list)",
    "POST /v1/organizations/{organizationId}/workspaces (workspace.create)",
    "GET /v1/profiles (profile.list)",
    "POST /v1/profiles (profile.create)",
    "GET /v1/workspaces/{workspaceId}/proxies (proxy.list)",
    "POST /v1/workspaces/{workspaceId}/proxies (proxy.create)",
    "GET /v1/workspaces/{workspaceId}/grants/page (grant.list.page)",
  ]);
  assert.deepEqual(findMissingTauriDevOperations(null), [
    "valid OpenAPI paths object",
  ]);
});
