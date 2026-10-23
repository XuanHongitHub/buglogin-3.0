import { spawn } from "node:child_process";
import { resolve } from "node:path";

const verifier = resolve(import.meta.dirname, "verify-fixtures.mjs");
const result = await new Promise((done) => {
  const child = spawn(process.execPath, [verifier], { stdio: "inherit" });
  child.on("close", (code) => done(code ?? 1));
});
if (result !== 0) process.exit(result);
