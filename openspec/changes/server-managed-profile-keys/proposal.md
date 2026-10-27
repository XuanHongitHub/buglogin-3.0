# Server-managed profile keys

Replace user-carried recovery codes and recipient-device fingerprint key delivery with service-managed envelope encryption. Keep profile payloads encrypted in object storage. Rust services authorize each key delivery against the current profile read/launch capability and audit the operation.

The service-managed wrapping key must come from a dedicated key-management provider or protected deployment secret. Do not silently fall back to a fixed key, plaintext key, or client-only recovery flow. Legacy encrypted profiles remain readable during migration; an already-unavailable legacy client key cannot be reconstructed from ciphertext.

## Compatibility and rollout

- Add a versioned server-managed envelope format while retaining reads of legacy client-managed envelopes during migration.
- New profile enrollment wraps the resource data key with the service KEK; S3 continues to receive encrypted profile payloads and wrapped key envelopes only.
- An authorized desktop requests the data key through Rust over authenticated TLS. React, logs, audit records, and MCP responses never receive key bytes.
- Remove manual recovery-code entry and recipient fingerprint confirmation from normal profile access.
- Do not claim release readiness until key-provider configuration, legacy migration, API ACL/audit tests, Windows/macOS vault evidence, and rollback are verified.
