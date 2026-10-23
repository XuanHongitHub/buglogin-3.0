# API surface parity — design addendum

## Goal

Make every supported desktop/API capability resolve through the canonical Rust command registry and its declared adapters. A visible UI action must map to a real authenticated handler, or be removed/disabled under an explicit capability disposition. Do not add compatibility stubs for removed capabilities.

## Authority

- Rust command registry and OpenAPI are canonical for request/response shape.
- Rust services enforce authentication, workspace/resource ACL, idempotency, and audit requirements.
- REST and MCP expose only commands declared for those adapters; Tauri commands bridge to the configured API or a documented local operation.
- Extension package installation remains blocked until CAP-EXT-001/002 policy, source verification, quarantine, rollback, and lifecycle contracts exist. Extension-group CRUD/assignment is separate and supported.
- VPN remains out of the current desktop scope per user direction. Its removed UI calls must not be reintroduced as no-op APIs.

## Lease status

Add a read-only lease status query for team-lock state. It is scoped by workspace and resource, requires `profile.read`, validates the authenticated actor for REST, returns `null` when no live lease exists, and never returns the owner token. The same query is available through MCP with the same resource authorization. Active lease state is backed by the configured coordination backend; backend errors remain explicit.

## Revision-aware organization/workspace reads

Keep the existing `organization.get` and `workspace.get` entity responses unchanged for compatibility. Add `organization.record.get` and `workspace.record.get`, returning their versioned service records (entity plus revision, and organization policy for organizations). The reads use the same authenticated principal and Rust ACL as existing gets. Clients use the returned revision as `expectedRevision` on updates and refresh the record after each successful mutation. Expose the commands consistently through REST, Tauri, and MCP; do not weaken optimistic concurrency or fabricate a revision.

For the active desktop compatibility adapter, partial workspace updates (`name` and/or supported `policy` fields) must fetch the workspace record under the requested organization/workspace scope, verify returned IDs, merge only allowlisted fields, and send the canonical full workspace with `expectedRevision`. A record read failure or scope mismatch fails closed. Canonical callers that already provide a full workspace and positive revision continue directly to PATCH; server revision conflicts remain authoritative.

## Standard MCP client requests

`tools/call` must work for a standard MCP client that sends only the JSON-RPC request ID, tool name, and arguments. BugLogin-specific `correlationId` and `idempotencyKey` fields remain optional extensions. When omitted, Rust derives a stable correlation ID and, for commands that require idempotency, a stable key from the JSON-RPC request ID and canonical tool name. Replaying the same request ID and tool replays the original mutation receipt; reusing that identity for different input returns `idempotency_conflict`. The transport bearer remains the sole authentication authority and must match any optional bearer extension.

## Hosted runtime dispatch boundary

The API validates the account token, current org/workspace/resource grants, idempotency, and audit record. It must not start a browser process on the API host. A connected desktop device owns the Wayfern process and profile data root, so launch/stop needs a durable device-bound command/receipt path: enqueue against the authorized account/device/profile binding, let only that device claim and execute it through the existing Rust lifecycle service, and persist the terminal operation result. Missing or disconnected devices must return a clear retryable status without changing the profile's runtime projection. The command must carry no caller-controlled executable or profile root, and lease tokens must remain out of MCP output, audit records, and logs.

Dispatch is a separate runtime capability and record type from connected cookie jobs. The account initiating MCP/API must choose one of its active device sessions (or receive `device_selection_required` when more than one is active). The device inbox derives identity from its device bearer, rechecks workspace/profile ACL at delivery and claim, and only replays in-progress work to the same device/account after restart. The desktop uses its existing lifecycle actor and locally hydrated profile path; it never accepts an executable, `profilesRoot`, or arbitrary process arguments from the control plane. A terminal receipt is the only event allowed to advance the hosted runtime projection.

Acceptance for this boundary requires authorization-denial tests across account/workspace/resource scope, duplicate/replay and conflicting-key tests, offline-device behavior, desktop restart recovery, and a live Windows launch/stop using the BugLogin-managed Wayfern binary and isolated profile root. An API-only lifecycle test does not satisfy desktop runtime acceptance.

## Verification

Contract generation, command registry parity, REST ACL/redaction, MCP scope/ACL, coordination backend tests, and source-current API checks are required. This addendum does not claim that every UI feature is complete; each remaining missing command must be classified against capability disposition before implementation.
