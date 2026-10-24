import { strict as assert } from "node:assert";
import { shouldPrepareAppUpdate } from "./app-update-lifecycle.ts";

const manualCheck = {
  source: "manual" as const,
  updateAvailable: true,
  consented: false,
  autoUpdatesEnabled: true,
  isBusy: false,
  updateReady: false,
};

assert.equal(shouldPrepareAppUpdate(manualCheck), false);
assert.equal(shouldPrepareAppUpdate({ ...manualCheck, consented: true }), true);
assert.equal(
  shouldPrepareAppUpdate({
    ...manualCheck,
    source: "background",
    consented: false,
  }),
  true,
);
assert.equal(
  shouldPrepareAppUpdate({
    ...manualCheck,
    source: "background",
    autoUpdatesEnabled: false,
  }),
  false,
);
assert.equal(
  shouldPrepareAppUpdate({ ...manualCheck, isBusy: true, consented: true }),
  false,
);
assert.equal(
  shouldPrepareAppUpdate({
    ...manualCheck,
    updateReady: true,
    consented: true,
  }),
  false,
);

console.log("app update lifecycle self-test passed");
