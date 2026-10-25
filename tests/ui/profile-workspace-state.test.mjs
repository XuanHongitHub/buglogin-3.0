import assert from "node:assert/strict";
import test from "node:test";
import {
  followLatestProfileLoadResult,
  hasProfilesForWorkspace,
  visibleProfilesForWorkspace,
  workspaceProfileCount,
} from "../../src/lib/profile-workspace-state.ts";

test("a failed workspace list keeps old records hidden and the count unavailable", () => {
  const previouslyLoadedProfiles = [{ id: "profile-from-workspace-a" }];
  assert.equal(
    hasProfilesForWorkspace("workspace-a", "workspace-a", true),
    false,
  );
  assert.deepEqual(
    visibleProfilesForWorkspace(
      previouslyLoadedProfiles,
      hasProfilesForWorkspace("workspace-a", "workspace-a", true),
    ),
    [],
  );

  const selectedWorkspaceId = "workspace-b";
  const profilesWorkspaceId = null;
  const ready = hasProfilesForWorkspace(
    selectedWorkspaceId,
    profilesWorkspaceId,
    false,
  );

  assert.equal(ready, false);
  assert.deepEqual(
    visibleProfilesForWorkspace(previouslyLoadedProfiles, ready),
    [],
  );
  assert.equal(workspaceProfileCount(0, ready), undefined);
  assert.equal(previouslyLoadedProfiles.length, 1);
});

test("a successful empty workspace list keeps a real zero distinct from unavailable", () => {
  const ready = hasProfilesForWorkspace("workspace-b", "workspace-b", false);

  assert.equal(ready, true);
  assert.deepEqual(visibleProfilesForWorkspace([], ready), []);
  assert.equal(workspaceProfileCount(0, ready), 0);
});

test("a superseded list request follows the latest snapshot result", async () => {
  const latestRequest = Promise.resolve(true);
  const supersededRequest = Promise.resolve(false);

  assert.equal(
    await followLatestProfileLoadResult(null, latestRequest, supersededRequest),
    true,
  );
  assert.equal(
    await followLatestProfileLoadResult(
      null,
      Promise.resolve(false),
      supersededRequest,
    ),
    false,
  );
});
