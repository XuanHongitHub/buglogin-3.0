import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import test from "node:test";

const script = fileURLToPath(new URL("../../scripts/release/assemble-tauri-updater-feed.mjs", import.meta.url));
const targets = [
  ["windows-x64", "windows-x86_64", "BugLogin_1.2.3_x64-setup.nsis.zip"],
  ["macos-arm64", "darwin-aarch64", "BugLogin_1.2.3_aarch64.app.tar.gz"],
  ["macos-x64", "darwin-x86_64", "BugLogin_1.2.3_x64.app.tar.gz"],
];

async function fixture(root, version = "1.2.3") {
  for (const [platform, , artifact] of targets) {
    const directory = join(root, `buglogin-v${version}-${platform}-signed`);
    await mkdir(directory, { recursive: true });
    await writeFile(join(directory, artifact), `signed bundle ${platform}`);
    await writeFile(join(directory, `${artifact}.sig`), `untrusted-fixture-signature-${platform}`);
  }
}

test("updater feed binds each supported target to a signed immutable release asset", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-updater-feed-"));
  try {
    const incoming = join(root, "incoming");
    const output = join(root, "output");
    await fixture(incoming);
    const result = spawnSync(process.execPath, [script, incoming, output, "1.2.3", "keyduc91/Malvanut-Login"], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(await readFile(join(output, "latest.json"), "utf8"));
    assert.equal(manifest.version, "1.2.3");
    assert.deepEqual(Object.keys(manifest.platforms).sort(), targets.map(([, key]) => key).sort());
    for (const [, key, artifact] of targets) {
      assert.match(manifest.platforms[key].url, new RegExp(`/releases/download/v1\\.2\\.3/${artifact.replaceAll(".", "\\.")}$`));
      assert.match(manifest.platforms[key].signature, /^untrusted-fixture-signature-/);
      await readFile(join(output, artifact));
      await readFile(join(output, `${artifact}.sig`));
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("updater feed refuses missing signatures and ambiguous platform artifacts", async () => {
  const root = await mkdtemp(join(tmpdir(), "buglogin-updater-feed-"));
  try {
    const incoming = join(root, "incoming");
    await fixture(incoming);
    await rm(join(incoming, "buglogin-v1.2.3-windows-x64-signed", "BugLogin_1.2.3_x64-setup.nsis.zip.sig"));
    const result = spawnSync(process.execPath, [script, incoming, join(root, "output"), "1.2.3", "keyduc91/Malvanut-Login"], { encoding: "utf8" });
    assert.notEqual(result.status, 0);
    await writeFile(join(incoming, "buglogin-v1.2.3-windows-x64-signed", "extra.nsis.zip"), "second artifact");
    const duplicate = spawnSync(process.execPath, [script, incoming, join(root, "output-duplicate"), "1.2.3", "keyduc91/Malvanut-Login"], { encoding: "utf8" });
    assert.notEqual(duplicate.status, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
