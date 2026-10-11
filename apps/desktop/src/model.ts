import type { Problem, Profile, ProfileState } from './contracts.js';

export type LoadState =
  | { kind: 'loading' }
  | { kind: 'ready'; profiles: Profile[] }
  | { kind: 'empty' }
  | { kind: 'error'; problem: Problem }
  | { kind: 'offline'; problem: Problem; profiles: Profile[] };
export type ProfileAction = { profileId: string; kind: 'launch' | 'stop'; operationId?: string; problem?: Problem };

export function loadedProfiles(profiles: Profile[]): LoadState { return profiles.length === 0 ? { kind: 'empty' } : { kind: 'ready', profiles }; }
export function failedProfiles(problem: Problem, cachedProfiles: Profile[]): LoadState { return problem.code === 'desktop_offline' ? { kind: 'offline', problem, profiles: cachedProfiles } : { kind: 'error', problem }; }
export function displayedProfiles(state: LoadState): Profile[] { return state.kind === 'ready' || state.kind === 'offline' ? state.profiles : []; }
export function projectedState(profile: Profile, action?: ProfileAction): ProfileState { return !action || action.profileId !== profile.id || action.problem ? profile.state : action.kind === 'launch' ? 'launching' : 'stopping'; }
export function canLaunch(state: ProfileState) { return state === 'created' || state === 'ready' || state === 'failed'; }
export function canStop(state: ProfileState) { return state === 'launching' || state === 'running' || state === 'recovering'; }
