import type { Problem, Profile, ProfileState } from "./contracts.js";

export type ProfileWithRevision = Profile & { revision: number };

export type LoadState =
  | { kind: "loading" }
  | { kind: "ready"; profiles: ProfileWithRevision[] }
  | { kind: "empty" }
  | { kind: "error"; problem: Problem }
  | { kind: "offline"; problem: Problem; profiles: ProfileWithRevision[] };
export type ProfileAction = {
  profileId: string;
  kind: "launch" | "stop";
  operationId?: string;
  problem?: Problem;
};

export function loadedProfiles(profiles: ProfileWithRevision[]): LoadState {
  return profiles.length === 0
    ? { kind: "empty" }
    : { kind: "ready", profiles };
}
export function failedProfiles(
  problem: Problem,
  cachedProfiles: ProfileWithRevision[],
): LoadState {
  return problem.code === "desktop_offline"
    ? { kind: "offline", problem, profiles: cachedProfiles }
    : { kind: "error", problem };
}
export function displayedProfiles(state: LoadState): ProfileWithRevision[] {
  return state.kind === "ready" || state.kind === "offline"
    ? state.profiles
    : [];
}
export function projectedState(
  profile: ProfileWithRevision,
  action?: ProfileAction,
): ProfileState {
  return !action || action.profileId !== profile.id || action.problem
    ? profile.state
    : action.kind === "launch"
      ? "launching"
      : "stopping";
}
export function canLaunch(state: ProfileState) {
  return state === "created" || state === "ready" || state === "failed";
}
export function canStop(state: ProfileState) {
  return state === "launching" || state === "running" || state === "recovering";
}
