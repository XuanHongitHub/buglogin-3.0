import assert from "node:assert/strict";
import {
  analyzeBulkInvitationEmails,
  analyzeBulkAccountRows,
  MAX_BULK_INVITATIONS,
} from "./member-invitation-input.ts";

const parsed = analyzeBulkInvitationEmails(
  "one@example.com\nTWO@example.com,one@EXAMPLE.com;bad-email",
);
assert.deepEqual(parsed.emails, ["one@example.com", "TWO@example.com"]);
assert.deepEqual(parsed.duplicates, ["one@EXAMPLE.com"]);
assert.deepEqual(parsed.invalid, ["bad-email"]);
assert.equal(parsed.entryCount, 4);
assert.equal(parsed.exceedsLimit, false);

const tooMany = Array.from(
  { length: MAX_BULK_INVITATIONS + 1 },
  (_, index) => `user${index}@example.com`,
).join("\n");
assert.equal(analyzeBulkInvitationEmails(tooMany).exceedsLimit, true);

const accountRows = analyzeBulkAccountRows("username,email\nlead1,one@example.com\nlead2,TWO@example.com\nLEAD1,three@example.com\ninvalid");
assert.deepEqual(accountRows.rows.map(({ username }) => username), ["lead1", "lead2"]);
assert.deepEqual(accountRows.duplicateLines, [4]);
assert.deepEqual(accountRows.invalidLines, [5]);

console.log("Bulk member invitation input self-test passed");
