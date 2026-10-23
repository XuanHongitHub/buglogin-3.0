# Profile List Pagination Plan

- [x] Add `limit` and `cursor` to the command input and canonical OpenAPI schema.
- [x] Make local Tauri, hosted Tauri, and MCP return bounded profile pages with matching filters and cursor validation.
- [x] Keep the desktop full-list convenience method backward-compatible while adding explicit page access and cycle protection.
- [x] Use the generated command page type so every listed profile statically carries its required revision.
- [x] Align the desktop profile projection's nullable runtime-version field with the canonical command DTO.
- [x] Preserve required profile revisions through the desktop profile load state and action components.
- [x] Enforce the canonical 128-character profile-ID cursor boundary in REST, MCP, and hosted Tauri.
- [x] Avoid rereading/deserializing the full local profile inventory for each cursor page.
- [x] Document canonical invalid-input, authentication, and permission Problems on paginated profile reads.
- [x] Regenerate command/API artifacts and update the profile-list contract documentation.
- [x] Route REST and MCP profile listing through one Rust service for identical workspace/archive/ACL filtering and cursor behavior.
- [x] Reject malformed `includeArchived` values in MCP and apply the canonical 128-character cursor boundary in both adapters.
- [x] Parse REST profile-list query values through the canonical Problem boundary; reject duplicate and unsupported query keys rather than leaking extractor-specific errors.
- [ ] Run the consolidated verification pass after implementation is complete (deferred by user).

Source implementation is complete. No build, test, or runtime verification has been run for this change. The shared REST/MCP list-service wiring was completed on 2026-10-17; consolidated verification remains deferred by user request.
