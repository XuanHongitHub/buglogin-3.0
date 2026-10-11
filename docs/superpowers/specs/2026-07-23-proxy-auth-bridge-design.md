# Authenticated Proxy Bridge Design

## Status

Implementation wave; local bridge first. Hosted credential delivery remains blocked until the device-envelope authority is approved.

## Goal

Allow Chromium Bugmini/Wayfern profiles to use authenticated HTTP, HTTPS, SOCKS4, and SOCKS5 upstream proxies without placing proxy credentials in browser argv, URLs, logs, telemetry, MCP output, or profile metadata.

## Authority

- Rust owns proxy endpoint validation, credential retrieval, bridge lifecycle, protocol handshakes, redaction, and audit events.
- Browser receives only a loopback proxy endpoint such as `http://127.0.0.1:<ephemeral-port>`.
- TypeScript/React never receives credential material.
- The bridge runs per profile launch, or uses a scoped shared instance only after a separate concurrency proof; initial implementation is per profile.

## Local flow

1. Resolve proxy metadata from the local repository or hosted redacted metadata.
2. Resolve credentials from the local OS vault only when a local credential binding is present.
3. Bind loopback on `127.0.0.1` to an OS-assigned port.
4. Start the bridge before browser spawn; inject only the loopback URL into `--proxy-server`.
5. For HTTP/HTTPS upstreams, add `Proxy-Authorization` in the bridge-to-upstream request. Never forward it to the destination server.
6. For SOCKS4/5 upstreams, perform the upstream authentication handshake inside the bridge.
7. Stop bridge before releasing the profile lease and erase in-memory credential buffers as far as Rust ownership permits.

## Protocol contract

- HTTP proxy: absolute-form requests and CONNECT.
- HTTPS proxy: TLS-to-upstream proxy, then CONNECT/forwarding.
- SOCKS4/SOCKS4a: CONNECT with username where supported.
- SOCKS5: CONNECT with no-auth or username/password authentication.
- Unsupported schemes, malformed targets, upstream auth failure, and bridge bind failure fail closed with stable problem codes.
- No DNS resolution outside the selected upstream for SOCKS domain targets.

## Security invariants

- No username/password in command line, URL, error, audit payload, screenshot, test evidence, or serialized profile.
- Loopback listener rejects non-loopback bind and does not accept browser credentials.
- Bridge accepts only the owning profile's local process context where the platform can prove it; otherwise uses a random unguessable listener token in an internal header and strips it before upstream forwarding.
- Credential values stay in memory only for the active bridge and are never persisted by the bridge.
- Every upstream connection has bounded connect/read/write timeouts and cancellation on profile stop.

## Hosted delivery blocker

Hosted proxy records expose `hasCredential` only. A production desktop cannot safely infer or reconstruct the credential reference. Add a separate device-bound encrypted secret envelope contract before hosted authenticated launch:

- server-side ACL checks workspace/profile/device scope;
- short-lived, one-use envelope bound to proxy ID, profile ID, device ID, and launch nonce;
- client decrypts locally; server never returns plaintext secret;
- replay, scope mismatch, expiry, and device revocation fail closed;
- envelope metadata is redacted and auditable.

Until this contract exists, hosted authenticated proxy launch returns `proxy_auth_bridge_required`, not a direct proxy URL.

## Evidence

Required before approval: local HTTP/HTTPS/SOCKS4/SOCKS5 auth tests, no-secret argv/log assertions, bridge stop cleanup, dead-upstream failure, concurrent profile isolation, Windows native run, macOS arm64/Intel run, hosted envelope tests.
