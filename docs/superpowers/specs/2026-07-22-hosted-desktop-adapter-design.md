# Hosted Desktop Adapter Design

## Scope

Production desktop uses a hosted HTTPS API as the only metadata/control authority. Local mode keeps the existing file-first authority. Production may use only replaceable browser payload/runtime caches.

## Contract

- `BUGLOGIN_DEPLOYMENT_MODE=production` requires `BUGLOGIN_API_BASE_URL` and `BUGLOGIN_API_TOKEN`.
- Remote requests use HTTPS, bearer authentication, correlation IDs, and idempotency keys for mutations.
- Remote profile list/create and operation polling use the existing REST contract.
- Remote failures return typed `Problem`; no fallback to local repository.
- Local proxy/profile repository is never opened as authority in production.

## Boundary

Rust owns the remote client, authentication header, request validation, response decoding, retry policy, and error mapping. React receives only typed command results. Tokens never enter React, logs, operation payloads, or errors.

## Deferred

Hosted proxy CRUD, profile payload/object sync, cookies, ACL/share graph, event stream, lease fencing, and offline cache reconciliation require their dedicated contracts; they remain release blockers until implemented and tested.

## Required Launch-Sync Contract

Current canonical contracts expose `resource.list/get/register` but no profile-resource binding read/mutation or lease transport. Production browser processes run on the desktop, so `profile.launch` must not call the server-side runtime endpoint or guess permissions.

The next versioned contract must define:

- profile-resource binding read/create/archive with workspace/resource/profile scope checks;
- lease acquire/renew/release with owner identity, opaque token, fencing token, TTL, and idempotency;
- profile payload manifest read/write guarded by the current fencing token;
- reconnect behavior for lease loss and remote revision changes;
- exact REST, Tauri, MCP, capability, audit, and error mappings.

Until this contract exists, production launch/stop, proxy persistence, and payload writes remain explicit fail-closed errors.
