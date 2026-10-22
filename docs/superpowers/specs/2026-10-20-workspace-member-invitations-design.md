# Workspace member invitations

Status: proposed implementation contract, synchronized with `openspec/changes/workspace-member-invitations/`.

## Product behavior

The member dialog must issue a bounded invitation code that an admin can hand to the intended person through an approved external channel. The raw code appears only once. Invitees redeem it inside BugLogin, then accept the resulting workspace membership. The inviter never chooses or receives the invitee's password.

Only existing active accounts with a verified canonical email can redeem an invitation, while authenticated. Invitation redemption creates a pending workspace membership; the account holder must accept it. It does not create accounts or credentials. The inviter never chooses or receives the invitee's password. Account provisioning remains a separate, owner-authorized flow and the member dialog must not claim it created a login.

## Authority and persistence

Rust owns invitation codes, email-to-account resolution, membership, authorization, expiry, replay prevention, idempotency, and audit. The invitation record stores the canonical recipient email, workspace, role, expiry, issuer, token hash, revision, and consumed/revoked state. Invitation consumption, membership, and audit must commit atomically in the shared PostgreSQL control store. No identity or credential is created by this flow.

## Compatibility

Keep the existing `member.invite` command and its AccountId-based contract for administrative compatibility. Add a separate invitation capability and REST/Tauri/MCP command contract. Do not make email values valid AccountIds and do not widen `Member` into an invitation DTO.

## Boundaries

- No SMTP/email delivery in this release.
- No inviter-supplied initial password or direct active membership.
- No group grant assignment in the first invitation transaction.
- Existing account recovery remains governed by `docs/superpowers/specs/2026-07-23-web-identity-design.md`.
- UI changes must follow `docs/design/ui-authority.md` and the relevant member/invite screen state rows.

## Security states

Creation and redemption need explicit loading, validation, expired, revoked, used, wrong-email, inactive-account, conflict, offline, and success states. Raw invitation codes, passwords, and full recipient addresses must be absent from logs, audit details, MCP results, and error text.

## Acceptance evidence

Use persistent PostgreSQL tests for transaction rollback and restart, domain/API tests for role boundaries and single-use codes, generated REST/command/MCP parity, and an authenticated Windows flow from invite creation through redemption and membership acceptance.
