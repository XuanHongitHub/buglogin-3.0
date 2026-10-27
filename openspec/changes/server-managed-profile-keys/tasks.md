# Tasks

- [ ] Add versioned key-management contract and threat-model decision; document loss of zero-knowledge and recovery behavior.
- [ ] Implement a production-configured key wrapping provider with authenticated scope (workspace, profile/resource, key version), rotation support, and secret redaction.
- [ ] Add durable API enrollment and authorized key delivery endpoints; enforce profile ACL on every request and emit redacted audit events.
- [ ] Update desktop Rust sync to enroll/fetch the key through API and persist only in the OS vault; remove user-entered recovery operation.
- [ ] Update manifest validation for managed envelopes while retaining legacy reads.
- [ ] Remove device fingerprint confirmation from ordinary profile use; keep explicit share ACLs and revoke semantics.
- [ ] Update REST/OpenAPI, MCP capability registry, command registry, design, and migration documentation.
- [ ] Add regression tests for unauthorized delivery, valid delivery, key scope mismatch, invalid wrapping key, legacy compatibility, and revocation.
- [ ] Rehearse legacy migration and rollback without mutating production data; capture Windows/macOS evidence before release.
