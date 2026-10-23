# Desktop Control REST Parity Design

Status: local implementation in progress.

## Problem

The canonical registry advertises organization, workspace, team, member, and resource commands through Tauri and REST, while the production desktop REST client does not map several of those commands. They fall through to api_command_unsupported even though the Rust API and OpenAPI routes exist.

## Contract

- Every Tauri control command that has a canonical REST adapter and is not handled by a specialized authenticated desktop flow must map to its documented Rust API route.
- Path identifiers are validated before path construction and removed from request bodies when OpenAPI defines a narrower body DTO.
- Mutations preserve the caller's idempotency key and use the HTTP method, JSON body, and result envelope defined by canonical OpenAPI.
- No production command may report success from a local fallback. Unsupported subscription, device-local, or migration operations keep their existing explicit behavior.

## Scope

Organization and workspace CRUD, team CRUD, workspace-member operations, and resource inventory/registration. This closes the production desktop control transport gap; it does not implement group desktop UI, cookie device dispatch, profile-key distribution, or production deployment.

## Acceptance

- Loopback transport tests assert route, method, authorization, correlation, idempotency, normalized body, and decoded response for each mapped command family.
- A command-parity test ensures every selected Tauri/REST control command is mapped or explicitly owned by a specialized desktop branch.
- OpenAPI generation, compatibility, Rust formatting, and desktop library tests pass.
