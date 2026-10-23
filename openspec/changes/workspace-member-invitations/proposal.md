# Workspace member invitations

## Why

The desktop member dialog offers email invitations and direct account provisioning, but the canonical `member.invite` API accepts only an existing `AccountId`. The current UI passes email as that ID and also advertises password provisioning that never reaches an account-creation API. This makes the visible onboarding flow fail or claim success for work it did not perform.

## Change

Add a canonical workspace invitation flow. An authorized workspace administrator creates an invitation for a normalized email, role, and bounded expiry. Rust stores the invitation and a hash of its random single-use code, returns the raw code once, and records an audit event. The recipient must already have an active BugLogin account with a verified matching email. They redeem the code while authenticated; the API creates an invited membership, then the recipient accepts it. Unknown or unverified email identities are rejected without revealing whether an account exists. Account registration/provisioning remains a separate owner-authorized flow; invitation codes never create credentials.

The old `member.invite` command remains available for existing AccountId-based administrative workflows. The new invitation contract is shared across REST, Tauri, and MCP; invite codes are never stored or returned after creation and are excluded from audit/log/MCP outputs except for the one-time create response.

## Boundaries

- Workspace authorization and assignable-role policy remain server enforced.
- Invitation redemption is single-use, expiry checked, idempotent, and transactionally creates membership and audit state.
- Invitation codes cannot provision identities or passwords. The inviter never sets or sees a member's password.
- Membership is `invited` until the authenticated recipient accepts; no direct active-member creation from the inviter UI.
- Group grants, email delivery, account recovery delivery, bulk invites, and organization-owner provisioning are outside this change.

## Risks

- The bearer invite code grants workspace membership to the matching authenticated account. It must have high entropy, bounded expiry, single-use persistence, rate limiting, redacted diagnostics, and a clear one-time display.
- The existing identity model requires verified identities before login. Redemption must require an already-verified matching account email and must not claim mailbox ownership.
- Account identity and membership must not become partially durable if any part of redemption fails.

## Acceptance

- A workspace admin can create a valid invite and receive its raw code exactly once.
- Existing verified users can redeem only an invite addressed to their canonical email; invitation redemption never creates credentials.
- Expired, revoked, replayed, mismatched-workspace, wrong-email, unknown-account, and unauthorized invitations fail without creating identity, password, membership, or partial indexes.
- Invitation codes grant membership only to an existing active account with a verified matching email; account registration and credential creation remain separate flows.
- Redeemed invite creation and membership acceptance survive API restart and preserve workspace role boundaries.
- The desktop no longer exposes fake direct provisioning or fake password-reset success.
