# Legacy Account Credential Migration Design

## Decision

The one-pass 1.0 data clone preserves existing password sign-in. The source uses Node `scryptSync(password, hexSaltText, 64)`; the salt is the 32-character UTF-8 hex text and the digest is 64 bytes. 3.0 accepts that verifier only for imported accounts and upgrades it to the configured Argon2id PHC hash after the first successful login.

## Secret boundary

- The read-only source exporter selects only user ID, password salt/hash, and provider needed to continue local password sign-in. OAuth subject values are never exported.
- Credential material is immediately encrypted with the migration package key using `AuthenticatedChunk`, with a fixed versioned AAD. The control-plane JSON may contain ciphertext and checksums only; plaintext hash/salt never reach a file, report, CLI output, or log.
- The package contains only the authenticated ciphertext object. The API decrypts it after signed-package verification and applies identities, indexes, and legacy verifier records in the same control import transaction as accounts and memberships.
- Google-only accounts without a local password are represented in the diff as requiring credential setup; Google subjects are not copied.

## Identity mapping

Use the existing deterministic account ID mapping and normalized legacy email. Generate a stable, collision-resistant username from the mapped account ID so email login remains supported without inventing a legacy username. Since 1.0 has no identity-verification field and already permits these account credentials, imported identities are verified. An email collision or malformed credential fails the whole import before writes.

## Authentication and rollback

Legacy verification uses Node-compatible scrypt parameters and constant-time digest comparison under the existing bounded password-verification semaphore and account lockout. On success, atomically replace the legacy verifier with Argon2id while preserving the password version, clearing legacy material, and resetting failure state. A compare-and-swap retry must re-check the latest lockout state so a concurrent failed-login lock cannot be cleared by an authentication that verified an older record. Password reset always removes the legacy verifier. Migration journal owns the identity, username/email indices, and credential records and deletes only matching CAS-owned rows on rollback.

Argon2id PHC records are bounded before verification: accept only the emitted Argon2id v19 policy (`m=19456,t=2,p=1`, 32-byte output, no key ID or associated data) and a PHC string no longer than 256 bytes. Do not let persisted PHC parameters choose verifier memory or CPU cost.

Every password-record read used for authentication, session-version checks, reset, or CAS retry must bind its embedded account ID to the storage key and reject version zero or an invalid verifier-state combination as corruption.

## Acceptance

- No raw credential material appears in control export, package reports, CLI output, audit detail, or logs.
- Ciphertext authenticity and checksum are verified before activation; missing, duplicate, malformed, or mismatched credentials block import.
- Package account identity/index/password puts are atomic with control-plane activation and journaled for rollback.
- Existing 1.0 password works once in 3.0, becomes Argon2id, and subsequent login no longer requires scrypt. Wrong-password lockout and password reset behavior remain unchanged.
- Provider `google` without local password is explicitly excluded from password sign-in; no OAuth subject is migrated.
