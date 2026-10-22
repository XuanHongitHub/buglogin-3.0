import { createHash, randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "../..");
const localSpecPath = resolve(root, "crates/api/openapi/openapi-v1.yaml");
const defaultBaseUrl = "https://api3.buglogin.com";
const requiredCoreOperations = ["auth.desktop.login", "runtime.catalog.get"];

export function summarizeOpenApi(bytes) {
  const text = Buffer.isBuffer(bytes) ? bytes.toString("utf8") : String(bytes);
  const paths = [...text.matchAll(/^  \/[^:\r\n]+:/gm)].length;
  const operations = [...text.matchAll(/^\s+operationId:\s*([^\s#]+)/gm)]
    .map((match) => match[1])
    .sort();
  return {
    paths,
    operations,
    sha256: createHash("sha256").update(bytes).digest("hex").toUpperCase(),
  };
}

export function compareOpenApi(localBytes, remoteBytes) {
  const local = summarizeOpenApi(localBytes);
  const remote = summarizeOpenApi(remoteBytes);
  const localSet = new Set(local.operations);
  const remoteSet = new Set(remote.operations);
  return {
    matches: local.sha256 === remote.sha256,
    local,
    remote,
    missingOperations: local.operations.filter((operation) => !remoteSet.has(operation)),
    unexpectedOperations: remote.operations.filter((operation) => !localSet.has(operation)),
  };
}

export async function verifyStagingApi({
  baseUrl = process.env.BUGLOGIN_STAGING_API_BASE_URL ?? defaultBaseUrl,
  fetchImpl = fetch,
  localBytes,
} = {}) {
  const base = baseUrl.replace(/\/+$/, "");
  const headers = { "x-correlation-id": `release-verify-${randomUUID()}` };
  const readyResponse = await fetchImpl(`${base}/v1/readyz`, {
    headers,
    signal: AbortSignal.timeout(10_000),
  });
  if (!readyResponse.ok) {
    const problem = await readyResponse.json().catch(() => null);
    const code = typeof problem?.code === "string" ? ` (${problem.code})` : "";
    throw new Error(`Staging readiness failed: GET /v1/readyz returned ${readyResponse.status}${code}`);
  }
  const readiness = await readyResponse.json();
  const runtimeCatalog = readyResponse.headers.get("x-buglogin-runtime-catalog");
  const readinessIssues = [];
  if (readiness?.status !== "ready" || readiness?.service !== "buglogin-api") {
    readinessIssues.push("Readiness body does not identify a ready BugLogin API");
  }
  if (runtimeCatalog !== "available") {
    readinessIssues.push(runtimeCatalog === "unavailable"
      ? "Signed runtime catalog is unavailable; production readiness requires it"
      : "Readiness response is missing a valid X-BugLogin-Runtime-Catalog header");
  }

  const openApiResponse = await fetchImpl(`${base}/v1/openapi.yaml`, {
    headers,
    signal: AbortSignal.timeout(10_000),
  });
  if (!openApiResponse.ok) {
    throw new Error(`Staging OpenAPI fetch failed: GET /v1/openapi.yaml returned ${openApiResponse.status}`);
  }
  const source = localBytes ?? (await readFile(localSpecPath));
  const contract = compareOpenApi(source, Buffer.from(await openApiResponse.arrayBuffer()));
  const remoteOperations = new Set(contract.remote.operations);
  const missingCoreOperations = requiredCoreOperations.filter(
    (operation) => !remoteOperations.has(operation),
  );
  return {
    readiness: { ...readiness, runtimeCatalog },
    readinessValid: readinessIssues.length === 0,
    readinessIssues,
    ...contract,
    missingCoreOperations,
  };
}

async function main() {
  const report = await verifyStagingApi();
  console.log(
    JSON.stringify(
      {
        matches: report.matches,
        readinessValid: report.readinessValid,
        readinessIssues: report.readinessIssues,
        missingCoreOperations: report.missingCoreOperations,
        readiness: report.readiness,
        local: {
          paths: report.local.paths,
          operations: report.local.operations.length,
          sha256: report.local.sha256,
        },
        remote: {
          paths: report.remote.paths,
          operations: report.remote.operations.length,
          sha256: report.remote.sha256,
        },
        missingOperations: report.missingOperations,
        unexpectedOperations: report.unexpectedOperations,
      },
      null,
      2,
    ),
  );
  if (!report.readinessValid || !report.matches || report.missingCoreOperations.length > 0) {
    if (!report.readinessValid) {
      console.error("[staging-api-contract] FAIL: staging readiness contract is incomplete.");
    }
    if (!report.matches) {
      console.error("[staging-api-contract] FAIL: staging OpenAPI does not match this source checkout.");
    }
    if (report.missingCoreOperations.length > 0) {
      console.error(
        `[staging-api-contract] FAIL: staging API is missing core operations: ${report.missingCoreOperations.join(", ")}`,
      );
    }
    process.exitCode = 1;
    return;
  }
  console.log("[staging-api-contract] PASS");
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    console.error(`[staging-api-contract] ERROR: ${error.message}`);
    process.exitCode = 1;
  });
}
