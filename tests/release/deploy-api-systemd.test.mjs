import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("remote deploy accepts matching active marker hashes and rejects drift", async () => {
  const helper = await readFile("scripts/release/deploy-api-systemd-remote.sh", "utf8");
  const guard = helper
    .split(/\r?\n/)
    .find((line) => line.includes("if ! is_deployment_id") && line.includes('old_marker_api_sha" !='));
  assert.ok(guard, "active marker guard must be present in the deployment helper");
  assert.match(
    guard,
    /^\s*if ! is_deployment_id "\$old_marker_deployment_id" \|\| \[\[ "\$old_marker_api_sha" != "\$old_api_sha" \|\| "\$old_marker_admin_sha" != "\$current_admin_sha" \]\]; then$/,
    "marker hash comparisons must be grouped inside a Bash conditional",
  );

  const markerIsValid = (markerApiSha, installedApiSha, markerAdminSha, installedAdminSha) =>
    markerApiSha === installedApiSha && markerAdminSha === installedAdminSha;
  const current = "52937c13417e063d6bc753a6276aa2e5dcddd88ca23ef5c811b8914c5bea3e66";
  const admin = "ab2773afd10aa90664c9472385f136a1fd7ece4bfcae814b94b6e67a3973a79a";
  assert.equal(markerIsValid(current, current, admin, admin), true);
  assert.equal(markerIsValid(current, "063dbd8096eee7c864e3a56979eb0fd58b0d2246fc74128571d3b277d82a69fa", admin, admin), false);
  assert.equal(markerIsValid(current, current, admin, "different-admin-hash"), false);
});
