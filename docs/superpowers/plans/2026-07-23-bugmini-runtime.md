# Bugmini Runtime Plan

1. [x] Audit production runtime inventory
2. [x] Add checksum/path/launch adapter core
3. [x] Preserve fingerprint adapter configuration
4. [x] Add desktop multi-runtime configuration
5. [~] Prove native Windows launch/stop/proxy/cookie/extensions
6. [ ] Prove native macOS arm64 and Intel/waiver
7. [ ] Approve migration classification as direct

## Gate

`bugmini` remains `partial/runtime_adapter_available_artifact_pending` until steps 3–6 pass. Adapter existence alone is not release evidence.

## Evidence update — 2026-07-23

- Migration now preserves runtime version, full versioned runtime config, parsed fingerprint, and note for local and hosted activation.
- Windows native smoke proves checksum, process launch, CDP, and process-tree stop using a disposable profile root.
- Canonical desktop native E2E now proves create, migrated cookie materialization, Bugmini launch, real HTTP cookie transmission, stop, shutdown, and cleanup.
- Bugmini-specific Windows argv normalization is covered: one `--user-data-dir=<path>` argument, no `\\?\` verbatim prefix at the process boundary.
- Credential-free proxy mapping has unit evidence; native proxy egress, authenticated proxy bridge, extension load/CDP target, full fingerprint runtime parity, and macOS remain pending.

