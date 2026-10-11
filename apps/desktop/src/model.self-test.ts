import assert from 'node:assert/strict';
import { canLaunch, canStop, failedProfiles, loadedProfiles, projectedState } from './model.ts';
import type { Problem, Profile } from './contracts.ts';

const profile: Profile = { id: 'profile_1', name: 'Primary', runtime: 'wayfern', state: 'ready' };
const offline: Problem = { type: 'about:blank', title: 'Offline', status: 503, code: 'desktop_offline', correlationId: 'cor_1' };
assert.deepEqual(loadedProfiles([]), { kind: 'empty' });
assert.equal(failedProfiles(offline, [profile]).kind, 'offline');
assert.equal(projectedState(profile, { profileId: profile.id, kind: 'launch' }), 'launching');
assert.equal(canLaunch('ready'), true);
assert.equal(canLaunch('running'), false);
assert.equal(canStop('running'), true);
assert.equal(canStop('ready'), false);
