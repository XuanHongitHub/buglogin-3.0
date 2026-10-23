# Manifest Commit Idempotency Identity

## Problem

Desktop manifest commits derive `Idempotency-Key` from resource, generation, and root checksum, while the API fingerprints the complete commit input. A retry after lease replacement can therefore reuse a key with a changed fencing token or expected profile revision and receive `idempotency_conflict`.

## Decision

The desktop derives the key from a canonical serialized identity containing the complete manifest, optional expected profile revision, and optional key-share envelope updates. Exact retries keep the same key; any request-input change gets a different key. The API continues validating the current fencing token and profile revision independently. No bearer or refresh credentials enter this identity.

The desktop-login `500 Problem` response is also added to OpenAPI because the handler can return `account_record_invalid`.

## Acceptance

- Unit coverage proves an identical commit identity is stable and a changed fencing token or profile revision changes the key.
- The existing lost-response reconciliation test continues to verify an already-active generation succeeds.
- OpenAPI contract and generated-client checks pass.
