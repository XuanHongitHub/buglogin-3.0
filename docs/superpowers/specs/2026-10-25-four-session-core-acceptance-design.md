# Four-session BugLogin core acceptance

User authorization: implement and test the requested core functions; use one
coordinator plus three Luna sessions. This replaces report-only execution.

## Required result

1. Account/resource management works through UI and API for Platform Admin,
   organization owner and workspace Owner/Admin/Lead/Operator/Member.
2. Profiles can download a browser version, launch, stop, sync browser data/tabs,
   transfer use between authorized devices, and enforce a single team writer.
3. Successive immutable desktop releases install and update correctly. Operational
   errors reach the production feedback service with secrets/browser data removed.

Two clients means two instances of BugLogin, not BugLogin plus Donut. Separate
device identity, vault session, WebView data, profile cache and preferences.
Use disposable QA accounts/orgs/profiles. Preserve existing user data/worktree.

## Acceptance rules

- UI and API are both required for role-sensitive actions.
- Allowed actions succeed; denied actions are rejected server-side.
- A successful Stop includes final sync/lease behavior, not just a changed icon.
- A sync succeeds after durable manifest commit and a usable restore on client B.
- Test browser data with controlled persistent cookie, localStorage, IndexedDB,
  bookmark and tabs. Never log their contents or reuse a user's real site session.
- Preserve Donut-based UI authority. Rust owns permissions, processes and storage.
- Build candidates after each coherent fix. Stable promotion follows real OS
  install/update smoke; do not overwrite old version artifacts.

## Budget and scope

Four hours is the execution target, not an automatic completion deadline. First
deliver a working create/download/launch/stop/sync/lock loop, then role breadth
and recovery. Keep every requested requirement on the acceptance matrix.
Cross-workspace move follows its existing design if exposed; group reassignment
must never be represented as a cross-workspace move.
