# Production Device Authorization Plan

1. [x] Extend device-bound session schema.
2. [x] Add durable authorization records.
3. [x] Implement start/approve/token endpoints.
4. [x] Add desktop remote authorization client.
5. [x] Run restart/race/redaction/security gates.
6. [x] Consume the approved device grant before issuing the durable access/refresh pair, so a crash cannot leave a reusable approval that creates additional session rows. If session persistence fails after consumption, the desktop starts a new authorization flow as specified.

Evidence:

- `cargo test -p buglogin-api --test http_auth`
- `cargo test -p buglogin-api device_authorization`
- `cargo test -p buglogin-desktop-app device_authorization`
- `node tests/contracts/contract-drift.test.mjs`
- `node scripts/ci/check-contracts.mjs`

Follow-up source change on 2026-10-16: exchange ordering now matches the design's single-use crash boundary and rejects approvals whose password credential version changed after approval. Regression execution is deferred to the consolidated verification pass.

Follow-up source change on 2026-10-17: the API's 60-second hosted cleanup worker purges up to 128 expired authorization records and their user-code indexes in one durable control transaction, so cleanup does not depend on new login traffic or add a full-table scan to device-start latency. Cleanup retries once after a revision conflict; source regression covers expiry, paired index deletion, and continued operation of a new flow. Tests/build remain deferred to the consolidated verification pass.

Follow-up source change: `slow_down` Problems now carry `fields.retryAfterSeconds`, and the desktop poller adopts that interval instead of retrying forever at the original cadence. API and desktop regression sources and OpenAPI descriptions are updated; execution remains deferred to the consolidated verification pass.

Follow-up hardening — 2026-10-17: because `api.buglogin.com` still serves 1.0, production desktop now accepts only exact `https://api3.buglogin.com`; it rejects the legacy host rather than risk routing profile/auth/sync traffic into 1.0. Device verification remains pinned to `https://app3.buglogin.com/device`. Re-enable the legacy hostname only after verified 3.0 DNS/API cutover. Regression source updated; build/test remains deferred.

Follow-up hardening — 2026-10-17: desktop device-authorization response `Debug` output now redacts device/user codes, and desktop/API transient authorization/refresh DTOs zeroize raw codes and tokens on drop. Token-bearing desktop response types no longer implement unnecessary `Clone`; raw codes transfer into pending state without creating extra string copies. `DesktopAuthStatus` also redacts the human user code from `Debug`. Source-only; consolidated verification deferred.

Follow-up hardening — 2026-10-17: domain-issued browser/session token containers and API device-session token pairs now zeroize their owned secrets on drop, including when persistence fails before a token is returned. Issuance transfers a token only after persistence succeeds; HTTP response construction explicitly transfers the device pair into the already-zeroizing response DTO. Temporary entropy buffers are zeroized after hex encoding. Source-only; consolidated verification deferred.

Follow-up normalization — 2026-10-17: one `DeviceId` validator now defines the 8–128 byte ASCII syntax for REST authorization/refresh, session persistence, encryption-key registration, desktop session/key vault keys, cookie-job identity, sync key envelopes, and MCP/OpenAPI envelopes. Runtime-projection IDs remain a separate local identifier. Generated command schemas were refreshed; regression sources were added but not run pending the single consolidated verification pass.

- [ ] Re-run HTTP device exchange replay, concurrent exchange, and persistence-failure coverage against the updated ordering.

Remaining production boundary:

- Tauri auth commands and UI gate.
- OS-vault session bootstrap and dynamic coordination rebind.
- Web credential/session/recovery flows.
