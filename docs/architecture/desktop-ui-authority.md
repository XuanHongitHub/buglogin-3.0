# Desktop UI Authority

Status: accepted implementation contract, 2026-08-11.

Read `docs/design/ui-authority.md` first. This file defines the desktop entrypoint and authority boundary; it does not authorize a custom visual language.

- Native desktop production entrypoint: `apps/desktop/src/main.tsx` built into `apps/desktop/dist` and loaded by Tauri.
- Native desktop authority: Rust commands exposed by `apps/desktop/src-tauri`; React is presentation only.
- Native shell: `apps/desktop/src/ProfileWorkspace.tsx`, following the pinned Donut density, rail, tabs, search, table, drawer, theme, and interaction patterns.
- The pinned source is `.tmp/donutbrowser-upstream` at the commit in `docs/architecture/upstream-baseline-manifest.yaml`; `.tmp/` remains read-only.
- BugLogin additions may extend the shell for workspace, resources, jobs, ACL, sync, migration, and diagnostics, but must use the upstream/shared token and component language.
- A separate raw-CSS shell, unrelated palette, second typography/icon system, or static fake-data screen is a contract violation.
- Web production entrypoint: `src/app/page.tsx` and `WebRouteHost`; it never starts or emulates a local desktop authority.
- `src/app/desktop-home.tsx` is retained only as upstream compatibility/reference code. It is not a production entrypoint.
- Production mode forwards protected operations to the hosted HTTPS control plane. Local mode uses the standalone Rust authority selected at Tauri bootstrap.
- Any new desktop action must use the generated command registry, typed bridge, Rust ACL/state authority, explicit loading/error/offline states, tests, and screenshot/interaction QA.
