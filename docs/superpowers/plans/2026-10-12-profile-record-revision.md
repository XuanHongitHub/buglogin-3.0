# Profile Record Revision Plan

Status: implemented for profile list/get/create/clone/archive/restore across MCP and desktop command adapters, with versioned REST pagination; targeted gates pass.

1. [x] Add a shared Rust profile record DTO and register it as the command output.
2. [x] Return the revision on MCP and desktop list/get/create/clone/archive/restore outputs.
3. [x] Normalize REST API client conversion to the shared command record DTO and fetch all cursor pages.
4. [x] Update OpenAPI and generated command artifacts.
5. [x] Run API, MCP, desktop, contract, and format gates.
