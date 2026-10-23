# Cookie Authority Durability Gate

Status: design gate; implementation blocked until the authority contract is approved.

## Scope

Make `cookie.list`, `cookie.import`, and `cookie.copy` safe across restart and concurrent local processes without persisting cookie values, staged secret inputs, lease tokens, or runtime state.

## Existing authority

- `CookieService` owns the local metadata ledger and OS-vault-backed values.
- Wayfern's native stopped-profile cookie database remains the browser runtime source of truth.
- Hosted production cannot decrypt or directly mutate client-owned native cookie files.
- REST/MCP cookie mutation therefore requires a connected-device job contract, not a server-side cookie ledger.

## Required durable records

Only these records may be durable:

- cookie metadata ledger revision and vault version references;
- per-scope CAS generation;
- completed idempotency receipt with redacted `CookieMutationResult`;
- cleanup-pending metadata that contains no cookie value.

Never durable:

- raw cookie values;
- staged import payloads after process exit;
- lease token, owner secret, or fencing credential;
- `running`/`process_id` runtime state.

## Restart semantics

- Staged imports are invalid after restart and return `staged_cookie_import_not_found`.
- Leases are invalid after restart; caller must reacquire from the canonical coordination authority.
- Runtime state starts unknown and must be re-established by the runtime reconciler.
- Generations and completed idempotency receipts reload from the same cookie metadata authority.
- An ambiguous mutation never reports success until the durable receipt is proven.

## Concurrency semantics

- The same authority lock covers read-modify-write of metadata, generation, and idempotency receipt.
- Locking must be advisory and platform-supported on Windows and macOS.
- A stale in-memory service cannot overwrite a newer ledger; reopen/reconcile is required before mutation.
- No second JSON/SQLite/Redis authority may be introduced.

## Acceptance

- restart: generation and receipt replay survive;
- restart: staged import and lease fail closed;
- two processes: one CAS winner, one conflict;
- crash after vault write: cleanup or explicit pending state, never false success;
- no secret values in files, logs, audit, MCP, errors, or diagnostics;
- Windows Credential Manager and macOS Keychain evidence;
- native Wayfern launch verifies the resulting cookie database.

