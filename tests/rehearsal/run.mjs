import { mkdir, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const result = spawnSync(
  "cargo",
  ["run", "--quiet", "--manifest-path", join(root, "Cargo.toml")],
  {
    cwd: join(root, "../.."),
    encoding: "utf8",
    env: {
      ...process.env,
      BUGLOGIN_EVIDENCE_GENERATED_AT: new Date().toISOString(),
      CARGO_TARGET_DIR:
        process.env.CARGO_TARGET_DIR ??
        (process.platform === "win32"
          ? "D:\\buglogin-build\\rehearsal"
          : join(root, "../../.build/rehearsal")),
    },
  },
);

if (result.status !== 0) {
  process.stderr.write(result.stderr || result.stdout);
  process.exit(result.status ?? 1);
}

const evidence = JSON.parse(result.stdout.trim());
const output = join(root, "artifacts", "migration-rehearsal.json");
await mkdir(dirname(output), { recursive: true });
await writeFile(output, `${JSON.stringify(evidence, null, 2)}\n`, "utf8");
process.stdout.write(`${output}\n`);
