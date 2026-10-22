import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

const e2eRoot = resolve(import.meta.dirname, "..");
const repoRoot = resolve(e2eRoot, "..", "..");
const fixture = JSON.parse(
  await readFile(
    resolve(e2eRoot, "fixtures", "core-vertical-smoke.json"),
    "utf8",
  ),
);
const commandRegistry = await readFile(
  resolve(repoRoot, "docs", "contracts", "command-registry-v1.yaml"),
  "utf8",
);
const openApi = await readFile(
  resolve(repoRoot, "docs", "contracts", "openapi-v1.yaml"),
  "utf8",
);
const accessControl = JSON.parse(
  await readFile(
    resolve(repoRoot, "docs", "contracts", "access-control-schema-v1.json"),
    "utf8",
  ),
);
const uuidV5 =
  /^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const leakedSecret =
  /synthetic-pass|authorization\s*[:=]|cookie\s*[:=]|proxy\.password\s*[:=]/i;

test("E2E-CORE-01 synthetic vertical smoke", async (suite) => {
  await suite.test("account owns one authorized workspace", () => {
    assert.equal(fixture.source, "synthetic");
    assert.match(fixture.account.id, uuidV5);
    assert.equal(fixture.workspace.ownerId, fixture.account.id);
    assert.ok(accessControl.properties.entity.enum.includes("account"));
    assert.ok(accessControl.properties.entity.enum.includes("workspace"));
  });

  await suite.test(
    "profile remains workspace-scoped with normalized proxy secrets",
    () => {
      assert.equal(fixture.profile.workspaceId, fixture.workspace.id);
      assert.equal(fixture.profile.ownerId, fixture.account.id);
      assert.equal(fixture.profile.proxyId, fixture.proxy.id);
      assert.equal(fixture.profile.runtime, "wayfern");
      assert.deepEqual(Object.keys(fixture.proxy.normalized).sort(), [
        "host",
        "passwordSecretRef",
        "port",
        "protocol",
        "usernameSecretRef",
      ]);
      assert.doesNotMatch(
        JSON.stringify(fixture.proxy.normalized),
        /synthetic-user|synthetic-pass/,
      );
      assert.match(openApi, /operationId: proxy\.parse/);
      assert.match(openApi, /operationId: profile\.create/);
    },
  );

  await suite.test("migration dry-run is immutable and repeatable", () => {
    assert.equal(fixture.migration.classification, "direct");
    assert.equal(fixture.migration.dryRunApproved, true);
    assert.equal(
      fixture.migration.sourceHashAfter,
      fixture.migration.sourceHashBefore,
    );
    assert.deepEqual(fixture.migration.firstRun, {
      created: 1,
      updated: 0,
      failed: 0,
    });
    assert.deepEqual(fixture.migration.repeatRun, {
      created: 0,
      updated: 0,
      failed: 0,
    });
    assert.match(openApi, /operationId: migration\.inspect/);
    assert.match(
      commandRegistry,
      /preconditions: \[source-read-only, snapshot-present, dry-run-approved\]/,
    );
  });

  await suite.test(
    "MCP command preserves capability, scope, result, and audit",
    () => {
      assert.equal(
        fixture.mcp.tool,
        fixture.mcp.commandId.replaceAll(".", "_"),
      );
      assert.equal(fixture.mcp.capability, "profile.create");
      assert.equal(fixture.mcp.workspaceId, fixture.workspace.id);
      assert.equal(fixture.mcp.actorId, fixture.account.id);
      assert.equal(fixture.mcp.resourceId, fixture.profile.id);
      assert.match(
        commandRegistry,
        /id: profile\.create[\s\S]*?capability: profile\.create[\s\S]*?adapters: \[tauri, rest, mcp\]/,
      );
      assert.match(
        commandRegistry,
        /executionOrder:[\s\S]*?- authorize-capability[\s\S]*?- append-audit-event/,
      );
    },
  );

  await suite.test(
    "audit correlates the MCP mutation without secret leakage",
    () => {
      assert.equal(fixture.audit.commandId, fixture.mcp.commandId);
      assert.equal(fixture.audit.adapter, "mcp");
      assert.equal(fixture.audit.actorId, fixture.mcp.actorId);
      assert.equal(fixture.audit.workspaceId, fixture.mcp.workspaceId);
      assert.equal(fixture.audit.resourceId, fixture.mcp.resourceId);
      assert.equal(fixture.audit.result, fixture.mcp.result);
      assert.deepEqual(fixture.audit.redactedFields, [
        "authorization",
        "cookies",
        "proxy.password",
      ]);
      assert.doesNotMatch(JSON.stringify(fixture.audit), leakedSecret);
    },
  );
});
