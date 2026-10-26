import assert from "node:assert/strict";
import {
  parseAccountTable,
  toAccountCsv,
  validateAccountRows,
} from "./account-input.ts";

const imported = parseAccountTable(
  'username,email,initialPassword,displayName\r\nshop-us,,LongPassword123!,"Shop, US"\r\nshop-eu,eu@example.com,"password,\"\"quoted\"\"123",EU',
);
assert.equal(imported.length, 2);
assert.equal(imported[0].displayName, "Shop, US");
assert.equal(imported[1].initialPassword, 'password,"quoted"123');
assert.deepEqual(validateAccountRows(imported), [[], []]);
assert.equal(
  parseAccountTable(
    'username\tdisplayName\temail\tinitialPassword\nuser-1\t"Name\nTwo"\t\tlong-password123',
  )[0].displayName,
  "Name\nTwo",
);
assert.throws(
  () => parseAccountTable('username,email\n"broken'),
  /unclosedQuote/,
);
const duplicate = validateAccountRows([
  { ...imported[0], username: "SHOP-US" },
  imported[0],
]);
assert(duplicate.every((issues) => issues.includes("duplicateUsername")));
assert(!toAccountCsv(imported, false).includes("LongPassword123"));
assert.deepEqual(parseAccountTable(toAccountCsv(imported, true)), imported);
console.log(
  "account-input: quoted CSV, TSV, optional email, duplicate validation and secret-free export passed",
);
