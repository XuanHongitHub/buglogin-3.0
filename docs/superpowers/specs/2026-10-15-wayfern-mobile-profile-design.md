# Wayfern Mobile Profile Design

Status: implementation required; mobile fingerprint is not currently supported.

## User outcome

Create and sync a Wayfern profile that presents a real generated Android or iOS fingerprint, including selectable supported iOS generations (15–26) when the installed Wayfern provider supports them. The desktop must not report success for a profile that only has a mobile user-agent string.

## Authority and data flow

- Rust API owns the canonical profile target/configuration, workspace ACL, revision, and idempotency.
- The installed Wayfern runtime is the fingerprint generation/application authority. Use its CDP `Wayfern.refreshFingerprint`, `Wayfern.setFingerprint`, and `Wayfern.getFingerprint` behavior, as found in the pinned Donut reference; do not copy the upstream premium/account gate into BugLogin.
- Desktop UI selects only values returned by the local provider's supported catalogue. Rust validates the selected OS/version against that catalogue before profile creation or launch.
- Persist the selected target in the canonical profile. Persist the generated fingerprint only in the encrypted profile data generation so profile sync carries it without adding fingerprint internals to API logs or ordinary control responses.
- Never synthesize success from a user-agent override. If the runtime lacks the requested provider/version, fail with a typed unsupported-capability error and do not create or launch the profile.

## Acceptance contract

- Create, read, update, sync, and restart preserve the typed target and profile revision.
- A live Wayfern CDP acknowledgement plus a read-back matching the requested OS/version is required before launch is reported successful.
- Credentials and Wayfern tokens stay out of profile payloads, logs, audit, and frontend state.
- The supported OS/version catalogue comes from the installed provider, not a hard-coded claim that every iOS version is available.
- UI follows the pinned Donut create-profile and fingerprint controls; no new visual system.

## Current evidence

- `ProfileCreateInput` now has a typed fingerprint target. `ProfileCreationService::build` stores it under versioned Wayfern runtime configuration. Dynamic provider support checks and runtime application are still missing.
- `WayfernAdapter` launches the executable with `--user-data-dir` and configured extra args. It does not issue Wayfern CDP fingerprint commands.
- The pinned Donut reference contains CDP generation/application code. The reference tree is read-only and is not itself proof that the installed BugLogin runtime exposes every requested iOS version.

## Deferred acceptance evidence

Per user instruction, do not run tests/build/E2E until implementation is complete. Final acceptance must include API contract coverage, desktop adapter coverage, and Windows runtime evidence for each advertised OS/version; macOS evidence is also required for supported macOS execution.
