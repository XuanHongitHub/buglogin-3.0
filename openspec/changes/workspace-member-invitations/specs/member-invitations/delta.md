# Member invitations

## ADDED Requirements

### Requirement: Workspace admins can issue bounded member invitations

The API SHALL authorize invitation creation against the target workspace and the actor's assignable role. It SHALL validate and canonicalize the recipient email, role, and expiry, persist only a cryptographic hash of a high-entropy single-use code, and return the raw code only in the successful creation response. It SHALL audit the actor, workspace, recipient identity hash, role, expiry, and correlation ID without logging the raw code.

#### Scenario: Authorized admin creates an invite

- GIVEN an active workspace administrator with member-create authority
- WHEN they create an invitation for a valid email and assignable role
- THEN the API persists the invitation and returns its code once with the expiry and workspace context

#### Scenario: Unauthorized or elevated role is rejected

- GIVEN an actor without member-create authority or attempting to assign a role at or above their own
- WHEN they create an invitation
- THEN the API returns a permission error and creates no invitation record

### Requirement: Invite redemption is single-use and bound to a verified existing account

The API SHALL require a valid, unexpired, unconsumed invitation code and an authenticated active account with a verified canonical email matching the invite. It SHALL persist invitation consumption, membership, and audit atomically. Invitation redemption SHALL NOT create accounts or credentials.

#### Scenario: Unknown or unverified invitee cannot redeem

- GIVEN a valid invitation for an email without an active verified account
- WHEN a client attempts to redeem it
- THEN the API rejects the request without disclosing whether an account exists or creating credentials, membership, or partial indexes

#### Scenario: Existing invitee redeems

- GIVEN a valid invitation whose email matches an existing verified account
- WHEN that authenticated account redeems the code
- THEN the API atomically creates an invited membership for that account and consumes the invite

#### Scenario: Expired, replayed, mismatched, or revoked invite

- GIVEN an expired, already consumed, revoked, workspace-mismatched, or email-mismatched invitation
- WHEN a client redeems it
- THEN the API rejects it without creating credentials, memberships, or partial identity indexes

### Requirement: Invite acceptance uses the recipient's authenticated session

The API SHALL activate an invited membership only for its target account, within the invitation workspace, while preserving role and workspace authorization. The operation SHALL be idempotent and durable.

#### Scenario: Recipient accepts

- GIVEN an authenticated account with a pending invitation membership
- WHEN the account accepts the invitation
- THEN only that membership becomes active and workspace access follows the assigned role
