import { spawn } from "node:child_process";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const child = spawn(
  process.execPath,
  ["tests/e2e/support/verify-fixtures.mjs"],
  { cwd: root, stdio: "inherit" },
);
child.on("error", (error) => {
  throw error;
});
child.on("close", (code) => process.exit(code ?? 1));
