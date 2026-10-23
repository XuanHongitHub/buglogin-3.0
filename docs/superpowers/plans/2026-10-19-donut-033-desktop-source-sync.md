# Donut 0.33 desktop source sync

- [x] Inventory the BugLogin and Donut 0.33 source trees; preserve the Donut checkout as read-only.
- [x] Port the Donut 0.33 create-profile modal and BugLogin idempotent create contract.
- [x] Add resizable, sortable, accessible profile table headers using the shared Donut table control.
- [x] Keep iOS/Android visible as disabled “Coming soon” choices; provide EN/VI labels.
- [ ] Review and resolve the remaining 49 differing shared files against BugLogin API/auth/sync contracts.
- [ ] Record intentional Donut premium/device-code and upstream-test exclusions; update the pinned baseline manifest and patch queue from the selected source snapshot.
- [ ] After source merge is complete, run one consolidated local desktop/API verification pass; separately track runtime-catalog publication and release readiness.

## Current source inventory

Compared with the current `F:\Herd\buglogin-3.0\src` working tree: 182 files are byte-identical, 49 shared paths differ, 21 paths are BugLogin-only, and 19 paths are Donut-only. The Donut-only set includes 3 excluded product components and 16 upstream tests. The source worktree is dirty, so HEAD alone does not identify every referenced byte. The production runtime catalog remains unavailable; this is a separate launch blocker from UI source sync.
