# Canonical cross-workspace profile transfer

## Why

BugLogin supports assigning a profile to one or more groups in its current workspace, including no group. It does not support moving the canonical profile resource to another workspace or organization. Group assignment cannot provide that behavior: profiles, grants, key envelopes, proxy dependencies, and sync manifests are workspace-scoped, and encrypted key envelopes authenticate their workspace and resource identifiers.

The existing profile archive/export and migration import flows do not constitute a live transfer. They are separate archive or package workflows and do not atomically move the authoritative profile binding, revoke source access, establish destination access, and preserve sync continuity.

## Scope

- Add a first-class, audited **move** of one stopped canonical profile from a source workspace to a destination workspace, including a destination in another organization when the actor has authority over both scopes.
- Keep the profile/resource identity stable where existing uniqueness and persistence rules allow; increment its binding/control revision and create a new encrypted data-key generation bound to the destination scope.
- Support an explicit destination group or `No Group`.
- Require a connected authorized desktop device to decrypt and re-encrypt encrypted data and key envelopes. The API stores ciphertext and coordinates the transaction; it never receives plaintext profile keys, proxy credentials, cookies, or profile data.
- Move through a durable, idempotent transfer state machine. Stage destination objects before one authoritative control commit; publish exactly one active binding; retain source blobs for bounded cleanup/rollback evidence.
- Remove source grants and group-derived access, create only explicitly selected destination grants, and prepare exact destination device envelopes before commit.
- Require explicit proxy dependency mapping or explicit removal. Re-encrypt credential envelopes on the client for the destination scope when a supported credential key is available.
- Expose the same authorization and state transitions to REST, Tauri, and MCP. MCP must dispatch cryptographic work to an authorized connected device and may not bypass the desktop key boundary.
- Add a full-page resource transfer flow with preflight, impact summary, progress, retry/cancel states, and a durable receipt.

## Non-goals

- Copy/duplicate profiles, group reassignment, ordinary profile export/import, browser-runtime download, organization ownership transfer, member migration, or bulk transfer.
- Server-side decryption, server-side credential translation, automatic source-grant copying, implicit destination group choice, or silently dropping a proxy.
- Replacing the existing group assignment contract or treating a group change as a workspace transfer.
- Immediate deletion of source ciphertext or claiming that previously copied plaintext/keys can be revoked.

## Main design decisions

1. Transfer is a **move**, not a copy. The source profile becomes unavailable as soon as the transfer enters the fenced state; there is never a period where both workspaces can launch or write the same profile.
2. The actor must be authorized to transfer/delete from the source and create/manage in the destination. Cross-organization transfer requires Platform Admin or explicit effective authority over both organization/workspace scopes. Org/Workspace roles do not inherit authority across unrelated organizations.
3. A fresh resource key is required. Resource-key envelopes bind workspace/resource; proxy credential envelopes bind workspace/resource/proxy/revision. Old ciphertext may be retained for rollback evidence, but it is not the active destination generation. The desktop decrypts source chunks and re-encrypts all logical files with a fresh key, then prepares destination-bound envelopes for the exact destination recipient-device set.
4. Destination access defaults to the minimum exact set required to administer/use the moved resource. No source direct grants or group grants are copied. Any additional destination grants are explicitly selected and validated before commit. The initiating actor must retain an authorized destination path.
5. Database/control state is the visibility commit point. Blob staging is not a commit. Until the control transaction commits, the destination is not visible and the source remains fenced. After commit, exactly one destination binding is active and the source binding is a transfer tombstone.
6. Pre-commit cancellation/rollback leaves the source binding, grants, envelopes, and manifest authoritative; staged destination blobs are queued for verified cleanup. Post-commit rollback is a new reverse transfer with fresh authorization and re-encryption, never a pointer flip that could restore stale keys or grants.

## Risks

- Transfer requires a connected device with source key access and sufficient local storage/network for a complete decrypt/re-encrypt pass. The UI must report this before starting.
- Large profiles need bounded, resumable staging and a durable journal; partial retries must not publish a partial manifest.
- Destination sharing can exceed envelope rotation limits. Preflight must report the exact recipient/device count and reject before source mutation if limits are exceeded.
- Proxy mapping may fail if the destination lacks a compatible proxy or if the client cannot open the credential envelope. The user must explicitly map or remove the dependency.
- S3/object-store writes and the control database cannot share a transaction. Staging, idempotent object addressing, a durable journal, commit fencing, and orphan cleanup are required.
- Existing offline devices may retain old plaintext or keys. Source revocation prevents future authorized fetches and future-generation access; it cannot erase data already copied by a device.

## Rollout

- Add the service/state machine and adversarial tests before enabling UI actions.
- Keep REST/Tauri/MCP feature capability disabled unless the required durable transfer journal, connected-device cryptographic dispatch, and cleanup worker are available.
- Enable first in isolated staging with two workspaces and two organizations, encrypted and unencrypted fixtures, proxy credentials, and recipient-device changes.
- Do not claim release readiness until Windows and macOS transfer journeys, interruption recovery, authorization matrix, and source/destination access revocation are verified.
