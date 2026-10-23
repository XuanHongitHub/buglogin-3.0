# Workspace member invitations — implementation plan

OpenSpec: `openspec/changes/workspace-member-invitations/`.

## Current evidence

- The desktop member dialog passes an email address as `Member.accountId`; canonical `AccountId` excludes `@`.
- Its direct-provision action submits `status: active` to `member.invite`, which only accepts `invited` memberships with a future expiry; the password is not sent to any account API.
- The API has durable `AccountIdentity`, Argon2id credentials, and existing-member activation, but no email-invite code flow. Account creation remains separate from workspace invitation.
- The master UI specification already defines no-SMTP invitation codes and deep links.
- Current source adds a hashed, single-use invitation record; REST create/redeem/revoke routes; OpenAPI schemas; and generated command contracts. Redemption resolves an active account with a verified canonical email and creates only a pending membership.
- The three commands now pass through the existing Tauri hosted-API adapter and typed desktop bridge. The command inputs include their route identifiers, while REST request bodies stay limited to body fields.
- Added `crates/domain/tests/member_invitation.rs`: four focused tests cover canonical email and digest-only persistence, invalid expiry/owner role, revision-checked single-use redemption, and revision-checked revocation. `cargo test --locked -p buglogin-domain --test member_invitation --target-dir E:\buglogin-target` passes (4/4).
- `cargo check --locked -p buglogin-api --lib --target-dir E:\buglogin-target`, `cargo check --locked -p buglogin-desktop-app --lib --target-dir E:\buglogin-target`, `pnpm check:api`, and targeted Rust formatting checks pass. Desktop `tsc --noEmit` still fails on the existing cross-dialog/UI contract set; no invitation bridge error appeared. No API lifecycle test or desktop smoke has been run for these new routes.
- MCP does not expose invitation creation or redemption because those operations carry bearer codes; MCP capability output must never receive the raw code. Safe, non-secret invitation operations can be assessed separately during the Groups/Extensions/MCP phase.

## Implementation order

1. Domain invitation model and security-focused validation. **Implemented and covered by focused domain tests.**
2. Persistent Rust invitation service with verified-email resolution and atomic control-store writes.
3. REST/OpenAPI routes, registry capability, and backward-compatible existing member routes. **Implemented in source; API lifecycle tests remain.**
4. Tauri adapter and typed desktop bridge. **Implemented in source.** Secret-bearing create/redeem are intentionally unavailable through MCP.
5. UI-owned dialog wiring after canonical input/output and state contracts are generated. **Pending AGY/UI ownership.**
6. PostgreSQL-backed REST lifecycle tests, source contract gates, then Windows UI smoke. **Pending; the domain tests do not prove durable API behavior.**

## Stop conditions

- Require an existing verified account email at redemption; the code grants membership, not account creation or mailbox verification.
- Do not return credentials or reusable raw invitation secrets from MCP or logs.
- Stop before production migration/deploy until persistence migration and rollback evidence are complete.
