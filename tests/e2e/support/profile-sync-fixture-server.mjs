#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const fixturePath = resolve(
  fileURLToPath(new URL("./profile-sync-fixture.html", import.meta.url)),
);
const portArgument = process.argv.find((value) => value.startsWith("--port="));
const portValue =
  portArgument?.slice("--port=".length) ??
  process.env.BUGLOGIN_PROFILE_FIXTURE_PORT ??
  "4177";
const port = Number(portValue);

if (!Number.isInteger(port) || port < 1 || port > 65_535) {
  throw new Error("Fixture port must be an integer from 1 to 65535.");
}

const html = await readFile(fixturePath);
const server = createServer((request, response) => {
  if (request.method === "GET" && request.url === "/healthz") {
    response.writeHead(200, {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
    });
    response.end(JSON.stringify({ status: "ready", fixture: "profile-sync" }));
    return;
  }

  if (request.method === "GET" && ["/", "/tab-two"].includes(request.url)) {
    response.writeHead(200, {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
      "content-security-policy":
        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
      "referrer-policy": "no-referrer",
    });
    response.end(html);
    return;
  }

  response.writeHead(404, {
    "content-type": "text/plain; charset=utf-8",
    "cache-control": "no-store",
  });
  response.end("Not found");
});

server.on("error", (error) => {
  console.error(`Profile sync fixture server failed: ${error.message}`);
  process.exitCode = 1;
});

server.listen(port, "127.0.0.1", () => {
  console.log(`BUGLOGIN_PROFILE_FIXTURE_URL=http://127.0.0.1:${port}/`);
});

function close() {
  server.close(() => process.exit(0));
}

process.once("SIGINT", close);
process.once("SIGTERM", close);
