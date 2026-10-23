import { spawnSync } from "node:child_process";
import { verifyVersionAuthority } from "./static-policy.mjs";

await verifyVersionAuthority();

const result = spawnSync(process.execPath, ["--test", "tests/release/release-workflows.test.mjs"], { stdio: "inherit" });
process.exit(result.status ?? 1);
