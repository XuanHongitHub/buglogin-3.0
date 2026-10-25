export function hasProfilesForWorkspace(
  selectedWorkspaceId: string,
  profilesWorkspaceId: string | null,
  workspaceSwitching: boolean,
): boolean {
  return (
    Boolean(selectedWorkspaceId) &&
    profilesWorkspaceId === selectedWorkspaceId &&
    !workspaceSwitching
  );
}

export function visibleProfilesForWorkspace<T>(
  profiles: T[],
  isReady: boolean,
): T[] {
  return isReady ? profiles : [];
}

export function workspaceProfileCount(
  profileCount: number,
  isReady: boolean,
): number | undefined {
  return isReady ? profileCount : undefined;
}

export function followLatestProfileLoadResult(
  result: boolean | null,
  latestRequest: Promise<boolean> | null,
  thisRequest: Promise<boolean>,
): Promise<boolean> | boolean {
  if (result !== null) return result;
  return latestRequest && latestRequest !== thisRequest ? latestRequest : false;
}
