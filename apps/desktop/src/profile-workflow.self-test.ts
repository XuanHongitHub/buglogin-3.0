import assert from "node:assert/strict";
import { normalizedRequiredText, usableRevision } from "./profile-workflow.ts";

assert.equal(normalizedRequiredText("  member_1  "), "member_1");
assert.equal(normalizedRequiredText("   "), undefined);
assert.equal(usableRevision(1), true);
assert.equal(usableRevision(0), false);
assert.equal(usableRevision(undefined), false);
