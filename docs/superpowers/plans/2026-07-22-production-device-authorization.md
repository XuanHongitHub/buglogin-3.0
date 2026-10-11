# Production Device Authorization Plan

1. [x] Extend device-bound session schema.
2. [x] Add durable authorization records.
3. [x] Implement start/approve/token endpoints.
4. [x] Add desktop remote authorization client.
5. [x] Run restart/race/redaction/security gates.

Evidence:

- `cargo test -p buglogin-api --test http_auth`
- `cargo test -p buglogin-api device_authorization`
- `cargo test -p buglogin-desktop-app device_authorization`
- `node tests/contracts/contract-drift.test.mjs`
- `node scripts/ci/check-contracts.mjs`

Remaining production boundary:

- Tauri auth commands and UI gate.
- OS-vault session bootstrap and dynamic coordination rebind.
- Web credential/session/recovery flows.
