#!/usr/bin/env node

import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const [, , command, ...args] = process.argv;
if (!command) {
  console.error("usage: run-web-host.mjs <next-command> [...args]");
  process.exit(2);
}

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const nextBin = resolve(projectRoot, "node_modules/next/dist/bin/next");
const child = spawn(process.execPath, [nextBin, command, ...args], {
  cwd: projectRoot,
  env: {
    ...process.env,
    BUGLOGIN_APP_TARGET: "web",
    NEXT_PUBLIC_BUGLOGIN_APP_TARGET: "web",
  },
  stdio: "inherit",
});

child.on("error", (error) => {
  console.error(`Failed to start Next.js: ${error.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 1);
});
