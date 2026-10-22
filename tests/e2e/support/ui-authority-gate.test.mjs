import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const read = (path) => readFile(resolve(repoRoot, path), "utf8");

const [
  desktopMain,
  desktopAuth,
  desktopBridge,
  remoteApi,
  donutDesktop,
  donutButton,
  donutCard,
  donutInput,
  donutCheckbox,
  donutLabel,
  webShell,
  workspace,
  routeMatch,
  baseline,
] = await Promise.all([
  read("apps/desktop/src/main.tsx"),
  read("apps/desktop/src/AuthBootstrap.tsx"),
  read("apps/desktop/src/bridge.ts"),
  read("apps/desktop/src-tauri/src/remote_api.rs"),
  read("src/app/desktop-home.tsx"),
  read("src/components/ui/button.tsx"),
  read("src/components/ui/card.tsx"),
  read("src/components/ui/input.tsx"),
  read("src/components/ui/checkbox.tsx"),
  read("src/components/ui/label.tsx"),
  read("frontend/src/apps/web/shells/index.tsx"),
  read("frontend/src/apps/web/workspace/index.tsx"),
  read("frontend/src/apps/web/route-match.ts"),
  read("docs/architecture/upstream-baseline-manifest.yaml"),
]);

test("desktop and web UI authority is represented by production code", async (suite) => {
  await suite.test(
    "desktop mounts Donut UI and reuses its existing form components",
    () => {
      assert.match(
        desktopMain,
        /from ['"]\.\.\/\.\.\/\.\.\/src\/app\/desktop-home\.js['"]/,
      );
      assert.match(
        desktopMain,
        /<AuthBootstrap api=\{api\}><Desktop(Home|Root)/,
      );
      for (const component of [
        "button",
        "card",
        "checkbox",
        "input",
        "label",
      ]) {
        assert.match(
          desktopAuth,
          new RegExp(`src/components/ui/${component}\\.js`),
        );
      }
      for (const [source, slot] of [
        [donutButton, "button"],
        [donutCard, "card"],
        [donutInput, "input"],
        [donutCheckbox, "checkbox"],
        [donutLabel, "label"],
      ]) {
        assert.match(source, new RegExp(`data-slot="${slot}"`));
      }
      assert.doesNotMatch(
        desktopAuth,
        /auth-shell|auth-topbar|auth-panel|<style/i,
      );
      assert.match(donutDesktop, /ProfilesDataTable/);
      assert.match(donutDesktop, /RailNav/);
    },
  );

  await suite.test(
    "desktop login submits credentials directly through the Rust API client",
    () => {
      assert.match(desktopAuth, /type="text"[\s\S]*autoComplete="username"/);
      assert.match(
        desktopAuth,
        /type=\{showPassword \? 'text' : 'password'\}[\s\S]*autoComplete="current-password"/,
      );
      assert.match(desktopAuth, /api\.login\(identifier\.trim\(\), password\)/);
      assert.match(
        desktopBridge,
        /login\(identifier: string, password: string\)/,
      );
      assert.match(desktopBridge, /desktop_auth_login/);
      assert.match(remoteApi, /v1\/auth\/desktop\/login/);
      assert.match(desktopAuth, /role="alert"/);
      assert.doesNotMatch(
        desktopAuth + desktopBridge,
        /startDeviceAuthorization|pollDeviceAuthorization|verificationUri|userCode|authorizing/,
      );
    },
  );

  await suite.test(
    "web shell and workspace expose state boundaries without fake counts",
    () => {
      assert.match(webShell, /Offline[\s\S]*Live data may be stale/);
      assert.match(webShell, /title="Loading"/);
      assert.match(webShell, /title="Unable to load"/);
      assert.match(webShell, /title="Nothing here yet"/);
      assert.match(workspace, /status === ["']forbidden["']/);
      assert.match(workspace, /status === ["']offline["']/);
      assert.match(workspace, /status === ["']error["']/);
      assert.match(workspace, /Server-authorized/);
      assert.doesNotMatch(workspace, /fake|stub|simulat/i);
    },
  );

  await suite.test(
    "web route surface matches the approved IA entrypoints",
    () => {
      for (const path of [
        "/login",
        "/recover",
        "/account",
        "/operations",
        "/migration",
        "/workspaces",
      ]) {
        assert.match(
          routeMatch,
          new RegExp(`path === '${path.replace("/", "\\/")}'`),
        );
      }
      assert.match(routeMatch, /\['\/admin'/);
      assert.match(routeMatch, /resources\/proxies/);
      assert.match(routeMatch, /workspaceId/);
    },
  );

  await suite.test(
    "desktop visual baseline remains pinned to the inspected Donut source",
    async () => {
      const commit = baseline.match(/commit: ([0-9a-f]{40})/)?.[1];
      assert.equal(commit, "f70a6c9671b17ab03a6e6f87a7362f8076c8339d");
      const { execFile } = await import("node:child_process");
      const { promisify } = await import("node:util");
      const run = promisify(execFile);
      const { stdout } = await run(
        "git",
        [
          "-C",
          resolve(repoRoot, ".tmp/donutbrowser-upstream"),
          "rev-parse",
          "HEAD",
        ],
        { encoding: "utf8" },
      );
      assert.equal(stdout.trim(), commit);
    },
  );
});
