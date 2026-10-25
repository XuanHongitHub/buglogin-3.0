# Profile transfer delta

## ADDED

### Requirement: A profile transfer is a canonical move between workspace scopes

The system MUST expose an explicit `profile.transfer` operation that moves one canonical profile resource from a source workspace to a destination workspace. It MUST NOT implement transfer by changing group membership, editing a workspace field in the client, exporting a portable package, or copying cookies. The source and destination organization/workspace IDs MUST be explicit and validated against authoritative records. The operation MUST be a move: after commit, the source binding is a transfer tombstone and only the destination binding is active.

The operation MUST support a destination group belonging to the destination workspace or an explicit `No Group` value. The default MUST NOT infer a group. A group change inside one workspace remains the existing `profile.groups.replace` operation and MUST NOT enter this transfer flow.

#### Scenario: Move a grouped profile to another workspace without a group

- **WHEN** an authorized actor transfers a stopped profile from Workspace A to Workspace B and selects `No Group`
- **THEN** the transfer commits one active Workspace B binding with an empty destination group assignment
- **AND** the Workspace A binding and source grants are no longer active
- **AND** no source group grant is carried into Workspace B

### Requirement: Transfer authority covers both scopes

The Rust/API authority MUST authorize source removal/transfer and destination resource creation/manage separately. Both checks MUST include effective capabilities, explicit denies, active membership, token scope, organization/workspace containment, and current revisions. Authority over only the source or only the destination MUST be insufficient. Organization-scoped authority MUST not extend to an unrelated organization; cross-organization transfer requires Platform Admin or explicit effective authority over both organization/workspace scopes. REST, Tauri, and MCP MUST use the same authorization service.

#### Scenario: Actor controls only one endpoint

- **WHEN** an actor is authorized to manage the source but not the destination, or the destination but not the source
- **THEN** transfer is denied with a typed scope-specific problem
- **AND** no source fence, destination staging, grant change, or envelope mutation is committed

### Requirement: Transfer is durable, idempotent, and single-owner

Transfer MUST use a durable state machine with a caller-supplied idempotency key and expected source/destination revisions. The state MUST record actor, source and destination scopes, plan hash, stage receipt, and committed receipt without storing plaintext secrets. Replaying the same key and normalized input MUST return the same state/receipt; changed input MUST return an idempotency conflict.

Before preparation, the system MUST fence source launch, mutation, ACL change, archive/delete, and sync writes, require the browser stopped, and require no active source lease. No destination binding may be visible before the authoritative commit. A successful commit MUST atomically publish the destination binding/manifest/envelopes/grants and source transfer tombstone so exactly one active binding exists. A pre-commit failure or cancellation MUST leave source data and authority recoverable and MUST make staged destination data invisible. Recovery after process/store restart MUST resume or cancel from the journal without guessing completion.

#### Scenario: Transfer is interrupted while staging

- **WHEN** an upload, desktop process, or API process fails before the control commit
- **THEN** the source remains fenced and authoritative until the journal deterministically resumes or cancels
- **AND** the destination remains invisible
- **AND** retrying the same idempotent operation cannot create duplicate active bindings

#### Scenario: Commit succeeds but response is lost

- **WHEN** the durable commit succeeds and the client loses the response
- **THEN** replay returns the original committed receipt
- **AND** it MUST NOT re-stage or create another destination resource

### Requirement: Encrypted content is re-bound on the authorized client

The server MUST never decrypt profile data, resource keys, proxy credentials, or private device keys. Because resource-key envelopes authenticate workspace/resource scope and proxy-credential envelopes authenticate workspace/resource/proxy/revision, the desktop MUST NOT copy those envelopes unchanged to the destination.

An authorized connected desktop MUST obtain source keys from the OS vault, validate/decrypt every source object and plaintext checksum, generate a fresh destination resource key, re-encrypt the complete logical profile generation, and create destination-bound envelopes for the exact active destination recipient-device set. It MUST confirm device fingerprints and reject missing, duplicate, stale, extra, wrong-scope, or over-limit envelopes. The new manifest MUST bind its key envelope and all dependencies to the destination scope and be fully verified before commit. Source keys and plaintext MUST not be sent to REST, MCP, logs, audit, or persistent transfer journal.

#### Scenario: Source envelope is replayed in destination scope

- **WHEN** a client submits a source-bound resource-key or proxy-credential envelope as a destination envelope
- **THEN** validation rejects the transfer before commit
- **AND** no source or destination access state is partially changed

#### Scenario: Client cannot decrypt one source object

- **WHEN** any source object fails authentication or checksum verification
- **THEN** transfer fails closed, leaves the source recoverable, and exposes a typed error without publishing a destination manifest

### Requirement: Groups, grants, recipients, and member access are explicitly rebuilt

Transfer MUST remove source direct grants, source group assignments, source group-derived access, and source recipient envelopes at commit. It MUST NOT copy source grants implicitly. The transfer plan MUST name the destination group or `No Group`, and MUST explicitly describe the destination access set. The initiating actor MUST retain an authorized destination management path. Encrypted destination access MUST be backed by exactly one current envelope for every effective active destination device determined by the shared Rust recipient resolver. ACL, group assignment, envelope set, source tombstone, and rotation-fence state MUST commit atomically. Destination membership or recipient-set changes before commit MUST invalidate the prepared snapshot and require re-preparation.

Previously copied plaintext or keys cannot be revoked; the system MUST state that source revocation prevents future authorized fetches and future-generation access but cannot erase data already copied by an offline device.

#### Scenario: Destination recipient changes after preparation

- **WHEN** a destination member/device or grant changes between recipient confirmation and commit
- **THEN** the prepared envelope set is rejected as stale
- **AND** no destination access is activated until the owner prepares the exact current set

### Requirement: Proxy dependencies require an explicit safe mapping

Every source proxy dependency MUST be mapped to a compatible destination-workspace proxy or explicitly removed in the transfer plan. The system MUST NOT carry a source-workspace proxy ID into the destination or silently drop the proxy. If credentials are retained, the connected client MUST decrypt them using the authorized existing credential key path and produce a new envelope bound to the destination workspace, resource, proxy, and revision. A missing destination proxy, unsupported proxy type, stale revision, missing credential key, or invalid credential envelope MUST block credential retention; the actor may instead explicitly choose `remove dependency` where policy allows.

#### Scenario: Proxy is removed explicitly

- **WHEN** the actor selects `remove dependency` during preflight
- **THEN** the destination manifest contains no dependency or credential envelope for that proxy
- **AND** the receipt and audit record identify that the dependency was removed without recording credential contents

### Requirement: Transfer audit and cleanup preserve evidence without leaking secrets

The commit MUST append one correlated audit event containing actor, source/destination organization/workspace/resource identifiers, group outcome, grant transition summary, proxy mapping outcome, manifest/key-generation identifiers, operation/idempotency correlation, and result. Audit and journal records MUST exclude profile bytes, cookies, plaintext keys, credentials, bearer tokens, and envelope plaintext.

Source ciphertext MUST remain under configured retention/garbage-collection policy after commit; staged unreferenced destination objects MUST be cleaned idempotently only after checking no committed manifest references them. Post-commit rollback MUST be a new authorized reverse transfer with fresh key generation and envelope preparation, not a direct restoration of old grants, envelopes, manifest pointer, or key.

### Requirement: REST, Tauri, MCP, and UI share one transfer contract

REST, Tauri, and MCP MUST call the same Rust transfer service and expose equivalent authorization, idempotency, state, errors, audit, and receipts. Tauri MUST orchestrate stop/sync, OS-vault key access, bounded local re-encryption, upload staging, and recovery. MCP MUST dispatch cryptographic preparation to an authorized connected device and MUST NOT accept or return profile plaintext, keys, or proxy credentials. The UI MUST provide a full-page transfer workflow with source/destination context, preflight, explicit group/grant/proxy choices, device/recipient summary, progress, conflict, retry, cancel, and committed receipt; a group assignment control or modal MUST NOT represent workspace transfer.

#### Scenario: MCP requests a transfer without a connected authorized device

- **WHEN** MCP has API-level authority but no eligible connected device can access the source OS-vault keys
- **THEN** the service returns an actionable device-required state
- **AND** no plaintext/key operation is attempted by the MCP server

## MODIFIED

None. Existing group assignment, profile archive/restore, portable export/import, and sync-import contracts remain distinct and unchanged.
