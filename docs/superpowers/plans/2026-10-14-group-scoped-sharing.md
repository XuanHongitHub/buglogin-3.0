# Group Scoped Sharing Plan

Spec: `docs/superpowers/specs/2026-10-14-group-scoped-sharing-design.md`.
OpenSpec: `openspec/changes/group-scoped-sharing/`.
Bead: `buglogin-6z3`.

## Execution

1. Freeze the typed grant and dynamic-membership contract across WP-33 and WP-39.
2. Implement the group-grant domain/service and persistence without changing existing profile grants.
3. Integrate one hosted evaluator across every protected profile operation; add denial and reassignment coverage.
4. Add canonical REST/MCP/Tauri/web adapters and generated contract parity.
5. [x] Ensure profile clone clears group assignments; only the separately authorized `profile.groups.replace` command may grant dynamic group access.
6. [x] Extend migration mapping/package/control import to preserve workspace-owned groups, profile group assignments, and active/revoked group grant provenance; reject ambiguous mappings. Synthetic package, dry-run, activation, replay, and rollback coverage passes.
7. [ ] Rehearse on the immutable source clone; compare all source and destination group/member/share counts and authorization outcomes. The read-only PostgreSQL adapter and deterministic organization-per-workspace/inert-invite rules now exist, but have not yet run through the migration CLI. The final per-workspace storage policy and complete S3/profile snapshot are still required.
8. Complete UI, E2E, security, and restart gates before activating migrated grants.

## Source-only follow-up — 2026-10-17

- [x] Check share authority before returning profile-binding, recipient-membership, or rotation-state outcomes; make REST and service paths return the same denial for unauthorized callers. Group share creation likewise checks group-share authority before active-member lookup. Regression source added; verification remains deferred.

## Evidence

- Current API candidate binary is deployed separately and its 200 API tests pass; this does not verify group sharing.
- The 1.0 rehearsal clone has 8 group grants; read-only checks found exact group/workspace and recipient-membership matches. This does not provide a migration export or activate grants.
- Mapping, package generation, and hosted control import now preserve typed group grants, revocation state, and group assignments in synthetic tests. No production clone export/import or cutover has run.
- API control import now accepts any positive source revision for typed group grants, preserving revoked grants at revision 2 while keeping the revision-1 rule for other imported record types. The dry-run/activate/replay fixture covers active and revoked group grants; API and sync crate suites pass on Windows with build output directed to `C:\buglogin-target`.
- The 2026-10-14 adapter reads only an explicitly named loopback `*_rehearsal*` database in a repeatable-read, read-only transaction, requires exact workspace storage-policy coverage, excludes invite tokens and password/auth secrets, and routes active shares for one unconsumed invite to hashed inert provenance. SQL query shapes were checked with `EXPLAIN` against `buglogin3_rehearsal_20261013`; the Rust CLI itself has not yet exported the clone.

## Out of scope

- Changing individual profile-share semantics.
- Guessing legacy runtime behavior or dropping legacy grants.
- Activating any migrated grant before dry-run/review evidence is complete.
