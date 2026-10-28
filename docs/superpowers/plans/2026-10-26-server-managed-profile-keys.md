# Server-managed profile keys implementation plan

Status: Rust/API and desktop access paths implemented; deployment key-provider and platform rollout gates remain open.

- [x] Inspect current sync encryption, device-envelope flow, API ACL, and desktop recovery path.
- [x] Record decision and compatibility constraints in OpenSpec and superpowers artifacts.
- [ ] Implement and verify a production key wrapping provider; current deployment key is environment-injected, not a remote KMS.
- [x] Implement durable versioned wrapped-key enrollment and ACL-checked delivery in Rust API.
- [x] Connect desktop Rust sync/share to enrollment/delivery and OS-vault persistence.
- [x] Let only an empty generation-1 genesis manifest pass the legacy recipient-envelope preflight so lifecycle can reach service-managed KEK enrollment; keep explicit rotation confirmations and non-genesis checks enforced.
- [x] Remove recovery-code and recipient-device fingerprint prompts from ordinary sharing/access.
- [x] Update manifest and REST/OpenAPI contracts; MCP does not receive key material.
- [x] Run API focused tests and desktop TypeScript typecheck; native desktop Rust build remains unverified because the configured migration sidecar binary is absent.
- [x] Add desktop regression coverage for the genesis-only fallback; the existing API role test confirms initial encrypted key enrollment for every authorized workspace role. Local desktop Rust test and `cargo check` pass; Windows/macOS packaged runtime QA remains open.
- [ ] Rehearse staging migration/rollback and obtain Windows/macOS evidence.

No production data was migrated or changed.
