# Tasks

## Contract and authority

- [ ] Define `profile.transfer` actions, typed request/receipt/state DTOs, error codes, scope, idempotency, expected revisions, and transfer journal transitions.
- [ ] Define dual-scope authorization: source transfer permission and destination create/manage permission; cross-organization authority; deny precedence; token scope; active membership; no role escalation.
- [ ] Define resource identity behavior, destination group/null semantics, destination grant selection, actor access retention, and archived/running/active-lease rejection.
- [ ] Define proxy mapping/removal input and compatibility validation; state whether proxy credentials can be re-encrypted with the existing device-side key authority.
- [ ] Define exact source and destination device-recipient snapshots, fingerprint confirmation, recipient limits, stale snapshot handling, and key-rotation fences.

## Durable transfer and encrypted data

- [ ] Add a durable transfer journal with reserved, source-fenced, preparing, staged, committed, canceled, and cleanup-pending states; bind transitions to idempotency key, actor, source/destination, and revisions.
- [ ] Fence launch, profile mutation, ACL changes, archive/delete, and sync writes for the source while transfer is reserved; require stopped runtime and no active lease before cryptographic preparation.
- [ ] Stage a complete destination generation using fresh resource/credential keys as applicable; decrypt and verify every source chunk, re-encrypt every logical file, verify plaintext/ciphertext checksums, and create destination-bound resource-key and proxy-credential envelopes.
- [ ] Preserve manifest dependencies only after explicit destination mapping; reject missing/ambiguous proxy mappings or explicitly remove the dependency and its credential envelope.
- [ ] Validate the staged destination manifest, exact recipient-device envelope set, destination group, proxy revisions, payload hashes, resource ownership, and size/quota before commit.
- [ ] Commit destination binding, source transfer tombstone, profile/resource workspace scope, group membership, grants, envelopes, manifest pointer, revisions, audit event, and idempotency receipt in one durable control transaction.
- [ ] Ensure a failed commit leaves source authoritative and fenced until safe retry/cancel; clean abandoned staged blobs idempotently after verifying they are not referenced by any committed manifest.
- [ ] Keep old source ciphertext under retention/GC policy after commit; never claim remote deletion of copies or automatically flip back to old keys.
- [ ] Add crash/reopen recovery at every transition and prove exactly one active binding after retries.

## REST, Tauri, MCP, and UI

- [ ] Add REST preflight/begin/status/prepare-or-device-dispatch/commit/cancel endpoints through one Rust transfer service; update OpenAPI and generated clients.
- [ ] Add canonical Tauri commands that stop/sync the source, obtain OS-vault keys on the source device, perform bounded re-encryption, stage output, and resume safely after restart.
- [ ] Add MCP commands backed by the same service and permissions. Cryptographic preparation must dispatch to an authorized connected device; MCP cannot accept or return keys, credentials, or plaintext data.
- [ ] Build a full-page transfer workflow (not a modal): source/destination breadcrumbs, destination group or No Group, grant and proxy mapping review, encryption/recipient impact, stop requirement, staged progress, retry/cancel, conflict, and committed receipt.
- [ ] Keep the action unavailable with a backend-derived reason when source is running/leased, no eligible device is connected, authority is missing, or destination prerequisites are not met.

## Verification and release gates

- [ ] Domain/API tests: source-only, destination-only, dual-scope, same-org, cross-org, Platform Admin, Lead ceiling, denies, inactive member, token scope, stale revisions, replay, and changed-input idempotency conflict.
- [ ] Crypto tests: reject source workspace/resource AAD at destination; fresh key generation; all chunks decrypt and re-encrypt; checksum mismatch; wrong/missing/stale device envelopes; proxy credential scope/revision mismatch.
- [ ] Persistence/failure tests: crash after reserve, each staged object, staged manifest, pre-commit, commit, cleanup; store reopen; no partial visibility; no two active bindings; source remains fenced until deterministic recovery.
- [ ] Resource tests: group-to-group, group-to-No-Group, No-Group-to-group, source grants removed, explicit destination grants only, group-derived recipient change, proxy mapped/removed, quota and recipient-limit rejection.
- [ ] REST/MCP/Tauri parity tests use the same state machine and produce equivalent authorization, errors, audit and replay receipts.
- [ ] E2E: two workspaces in one org and two orgs; source/destination users/devices; encrypted and unencrypted profiles; cookies/storage; proxy credentials; sync before/after; old source access denied; destination access succeeds.
- [ ] Interrupt network/app during staging, then reopen/retry/cancel; prove no lost profile data and no stale writer can commit.
- [ ] Windows and macOS screenshot, keyboard/focus, local-vault, stop/start, and end-to-end evidence; release remains blocked until both OS paths pass.
