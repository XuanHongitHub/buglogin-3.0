import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const run = (args) => {
  const result =
    process.platform === "win32"
      ? spawnSync(
          process.env.ComSpec ?? "cmd.exe",
          ["/d", "/s", "/c", "pnpm", ...args],
          { cwd: root, stdio: "inherit" },
        )
      : spawnSync("pnpm", args, { cwd: root, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
};

run(["check:api"]);
run(["--filter", "@buglogin/web", "test"]);
run(["--filter", "@buglogin/desktop", "test"]);
run(["--filter", "@buglogin/e2e", "check"]);
run([
  "exec",
  "biome",
  "check",
  "scripts/ci",
  "frontend",
  "apps/desktop",
  "tests/e2e",
  "--error-on-warnings",
]);
