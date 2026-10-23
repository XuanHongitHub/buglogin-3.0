# Member, Resource, and Grant List Pagination Plan

- [x] Add bounded, ID-ordered page methods to member and resource services.
- [x] Add canonical REST page routes while retaining existing array routes for v1 compatibility.
- [x] Update command input schemas, OpenAPI, and generated API artifacts.
- [x] Update desktop and MCP adapters to preserve page semantics.
- [x] Align member/resource cursor schemas and hosted Tauri validation with canonical 128-character domain IDs.
- [x] Document canonical invalid-input, authentication, and permission Problems on paginated reads.
- [x] Declare required workspace path parameters on both canonical page routes and deprecated array routes.
- [x] Apply the canonical workspace ID boundary to the shared path parameter and list-command inputs.
- [x] Declare every OpenAPI `{pathParameter}` through a shared typed component; cover workspace/profile/member/resource/grant/proxy/job/operation/account/organization/team/group IDs plus the custom migration-package, API-token, and session ID formats.
- [x] Add contract regression coverage requiring every route placeholder to resolve to exactly one shared path-parameter component.
- [x] Reuse the Rust `CanonicalId` schema across domain-ID fields in REST request/record DTOs and their ID arrays; keep device, operation, token, session, export, and migration-package IDs on their dedicated formats.
- [x] Add contract regression coverage for canonical ID limits and DTO field reuse.
- [x] Unify migration package path, command, import, activation, and receipt IDs under the validator-compatible `MigrationPackageId` schema.
- [x] Match operation ID path and DTO schemas to the operation service's permissive-first-character but bounded ASCII identifier validator.
- [x] Normalize workspace/profile/resource/member ID query parameters through shared components referencing `CanonicalId`.
- [x] Update web workspace loading to fetch all pages with cursor-cycle protection.
- [x] Add regression source for ordered continuation, v1 legacy compatibility, invalid limits, workspace ACL, and repeated client cursors.
- [x] Add bounded `GrantId`-ordered pages with resource/member filters and reuse sharing authorization.
- [x] Add a canonical grant page REST route while retaining the current array route for existing clients.
- [x] Align command registry, OpenAPI, generated schemas, desktop REST mapping, and MCP grant listing to `GrantPage`; label the array REST route as `grant.list.legacy`.
- [x] Make desktop share UI follow member and grant cursors with cycle/size protection.
- [x] Add tests for grant page filtering, continuation, invalid input, authorization, and legacy array compatibility. Service tests cover filters and continuation; HTTP tests cover continuation, invalid limit, authorization denial, and legacy REST array shape/filtering; MCP execution tests cover page continuation and legacy all-items behavior.
- [x] Run the consolidated verification pass, including process E2E and contract parity.

Member/resource pagination is implemented. Grant pagination and desktop collection following were implemented on 2026-10-19. The API/contract/process gates cover continuation, filter/limit behavior, authorization denial, and v1 legacy array compatibility. Native Tauri interaction is outside this API pagination gate and remains unverified.
