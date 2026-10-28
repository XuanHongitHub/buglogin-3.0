import assert from "node:assert/strict";
import {
  analyzeOrganizationAccountRows,
  validateOrganizationAccountFields,
} from "./organization-account-input.ts";

const valid = analyzeOrganizationAccountRows(
  "username,email,initial-password\n# import batch\n alice ,alice@example.com,1234\nbob,bob@example.com,abcd",
);
assert.equal(valid.rows.length, 2);
assert.deepEqual(valid.invalidLines, []);
assert.deepEqual(valid.duplicateLines, []);
assert.equal(valid.exceedsLimit, false);
assert.equal(valid.entryCount, 2);

const duplicates = analyzeOrganizationAccountRows(
  "alice,a@example.com,123456789012\nALICE,other@example.com,123456789012\nbob,A@example.com,123456789012",
);
assert.equal(duplicates.rows.length, 1);
assert.deepEqual(duplicates.duplicateLines, [2, 3]);

const invalid = analyzeOrganizationAccountRows(
  "ab,a@example.com,123\nvalid,not-an-email,1234\nvalid2,b@example.com,1234,comma",
);
assert.deepEqual(invalid.invalidLines, [1, 2]);
assert.equal(invalid.rows[0]?.password, "1234,comma");

const tooMany = Array.from(
  { length: 101 },
  (_, index) => `user${index},user${index}@example.com,1234`,
).join("\n");
assert.equal(analyzeOrganizationAccountRows(tooMany).exceedsLimit, true);

assert.equal(
  validateOrganizationAccountFields({
    username: "alice",
    email: "alice@example.com",
    password: "1234",
    confirmPassword: "1234",
  }),
  null,
);
assert.equal(
  validateOrganizationAccountFields({
    username: "alice",
    email: "alice@example.com",
    password: "1234",
    confirmPassword: "different-password",
  }),
  "password_mismatch",
);

console.log("organization account input self-test passed");
