# BugLogin production API and MCP integration

This document describes the API contract currently implemented by BugLogin. The Rust command registry and the deployed `/v1/openapi.yaml` are authoritative; the live MCP `tools/list` response is authoritative for the MCP tools enabled by that deployment.

## Endpoints and deployment

| Purpose | Endpoint |
| --- | --- |
| Liveness | `GET /v1/health` |
| Readiness | `GET /v1/readyz` |
| OpenAPI | `GET /v1/openapi.yaml` |
| MCP status | `GET /v1/mcp/status` |
| MCP JSON-RPC | `POST /v1/mcp` |

MCP is disabled unless the API process starts with `BUGLOGIN_MCP_ENABLED=true`. There is no separate `BUGLOGIN_MCP_TOKEN`: every MCP client presents its own scoped BugLogin API token. The API validates the token against its persistent token store for each request. Token revocation therefore takes effect without an API restart.

For a remote platform, expose the API through HTTPS with a trusted certificate and an ingress allowlist. Do not bind an unauthenticated local desktop API directly to a public interface. `/v1/readyz` must be healthy before registering the MCP server. MCP JSON-RPC currently supports `initialize`, `tools/list`, and `tools/call` over HTTP POST; this implementation does not advertise SSE or resumable sessions. JSON-RPC requests include an `id`.

## Issue and revoke a client token

An account administrator creates a token while authenticated to the BugLogin account API. The create and list endpoints require the BugLogin browser session and CSRF cookies; token creation is not allowed using an existing API token. Create a separate token per integrating platform, grant only the capabilities and actions that platform needs, and set an expiry.

The token belongs to the authenticated account that created it. For this source version, token issuance is exposed through the account web-session API; the desktop Account UI does not yet provide API-key creation/rotation. Once issued, the same token may be used by a platform MCP client. MCP resolves the principal from that token on every request; it does not inherit whichever account happens to be open in the desktop UI. Do not share a token between BugLogin accounts.

```http
POST /v1/auth/api-tokens
Cookie: __Host-buglogin_session=<session>; __Host-buglogin_csrf=<csrf>
X-CSRF-Token: <csrf>
X-Correlation-ID: cor_platform_token_create_01
Content-Type: application/json

{
  "name": "platform-agent-prod",
  "scopes": ["profile.read:read", "profile.create:create", "profile.update:update"],
  "expiresAt": 1823990400
}
```

The response contains `token` (`blapi_` followed by 64 lowercase hexadecimal characters) exactly once. Store it in the platform's secret manager. The API stores only a verifier. Scopes use `<capability>:<action>`; actions are `read`, `create`, `update`, `delete`, `execute`, `share`, `manage`, or `*`. The API validates scope names against its capability model when authorizing each command. Do not grant `*` to a routine integration.

```http
DELETE /v1/auth/api-tokens/<tokenRecordId>
Cookie: __Host-buglogin_session=<session>; __Host-buglogin_csrf=<csrf>
X-CSRF-Token: <csrf>
X-Correlation-ID: cor_platform_token_revoke_01
Idempotency-Key: platform-token-revoke-0001
```

Changing the account password revokes its API tokens. See the deployed OpenAPI document for cookie, CSRF, error, and response details.

## Connect an MCP client

Configure the remote platform with the MCP URL and a secret reference for the token. Do not put the token in a URL, a checked-in config file, a prompt, or logs.

```json
{
  "mcpServers": {
    "buglogin": {
      "type": "http",
      "url": "https://<buglogin-api-host>/v1/mcp",
      "headers": {
        "Authorization": "Bearer ${BUGLOGIN_API_TOKEN}"
      }
    }
  }
}
```

Client configuration formats vary. If a client cannot interpolate a secret reference in headers, use that client's secure credential facility. A platform on another machine must reach the HTTPS API host; `127.0.0.1` refers to the platform machine itself.

Check deployment status without exposing a credential:

```http
GET /v1/mcp/status
```

The endpoint reports whether MCP is enabled and the MCP path. It does not disclose tokens or tool data.

## JSON-RPC examples

Initialize:

```http
POST /v1/mcp
Authorization: Bearer ${BUGLOGIN_API_TOKEN}
Content-Type: application/json

{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"platform-agent","version":"1.0"}}}
```

Discover the deployment's actual tool names and input schemas before calling them:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
```

Read profiles in a workspace (the deployed schema returned by `tools/list` defines the exact arguments):

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "profile_list",
    "arguments": {"workspaceId": "<workspace-id>"},
    "correlationId": "cor_platform_profile_list_01"
  }
}
```

`profile_launch` requires `profileId`; it resolves the profile's workspace/resource binding before authorization. Hosted desktop dispatch does not accept executable paths, profile roots, arbitrary arguments, or device IDs from MCP. `profilesRoot` and `extraArgs` are optional compatibility inputs for a local-authority API only; hosted desktop devices resolve those values from their verified runtime and profile configuration.

Mutation calls must include an `idempotencyKey` unique to the logical request. Reuse the same key and identical arguments when retrying an uncertain response. `correlationId` is required for every call. The token is taken from the HTTP Authorization header; if `bearerToken` is also supplied in tool parameters it must match that header.

The MCP tool list is built from `CommandRegistry` entries with the MCP adapter. It currently includes profile lifecycle and CRUD commands, organization/workspace/group and extension-group operations, proxy operations, resource/lease operations, audit/event reads, and selected migration/runtime operations. It excludes commands not registered for MCP. Use `tools/list` and its `inputSchema` as the exact per-release catalog; do not hard-code a guessed tool count.

## Authorization and data boundaries

- **Account → organization → workspace → group → profile:** the API token identifies one account; the authenticated account's active organization/workspace membership and role supply its grants. The MCP request must name the relevant workspace/resource/profile where the tool schema requires them. Group membership or a profile ID alone does not grant access. Profile and group sharing grants are evaluated by the API for each operation.
- A token issued to a different account has that other account's roles and memberships, even if the desktop app is signed in as another user. Token scopes can narrow these grants; they cannot add an organization role, workspace membership, or resource grant.
- Every call is authenticated as the API-token owner's account. Inactive accounts, expired/revoked tokens, missing scopes, workspace membership failures, and resource grants are rejected by the Rust API.
- Tool execution uses the same command registry, request validation, authorization, idempotency, and audit path as other API adapters. MCP is not an alternate authorization layer.
- Use a workspace-scoped token and only pass IDs belonging to that workspace. A caller cannot select another tenant by changing an ID.
- The runtime catalog and launch/stop calls do not grant permission to arbitrary host process execution. Browser control remains subject to the profile/runtime lifecycle and host device authorization.
- Do not pass passwords, proxy credentials, cookie values, refresh tokens, or other secrets in MCP arguments. Secret-bearing outputs are rejected by the MCP output boundary. Cookie values are not an MCP automation channel.
- Browser page automation, reusable scenarios/goals, and arbitrary script execution are not part of the current MCP command contract. Cookie-farming and target-specific engagement automation are explicitly excluded from this product scope. No endpoint should be inferred from UI labels or old examples; only the deployed registry catalog is supported.

## Errors and operations

MCP errors use JSON-RPC `error` with a BugLogin problem object in `error.data`. Common HTTP statuses are `401` for invalid credentials, `403` for Origin or authorization denial, `413` for an oversized body, `503` when MCP is disabled or a required service is unavailable, and `409` for revision/idempotency conflicts. Each call should log its own correlation ID, never its bearer or sensitive arguments.

The API currently accepts browser-origin requests only from loopback origins. Native platform MCP clients should omit the `Origin` header; browser-based remote clients are rejected unless the server's Origin policy is deliberately changed and reviewed.

## Release checks

Before onboarding a platform against a release:

1. Verify `/v1/readyz` and confirm the expected production persistence/runtime-catalog headers.
2. Verify `/v1/mcp/status` reports `enabled: true`.
3. Create a least-privilege token, then test `initialize`, `tools/list`, one authorized read, and one denied out-of-scope read.
4. Test a mutation with an idempotency key, repeat the same request, then revoke the token and verify the next call returns `401`.
5. Compare tool schemas with the deployed OpenAPI and registry artifacts. Do not infer production readiness from local tests alone.

The general REST contract is published at `/v1/openapi.yaml`. MCP adds only the JSON-RPC wrapper around commands explicitly enabled for the MCP adapter; it does not create operations that REST or the canonical registry do not define.
