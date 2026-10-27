# Authoritative Profile Status Implementation Plan

1. Extend the existing runtime projection REST surface with an authorized `syncing` transition and desktop transport method.
2. Publish `syncing` only after the local browser process tree has stopped and before cloud sync work; retain `stopped` as the successful completion state.
3. Update lock resolution to consume explicit projection state. Preserve confirmed locks across transient status-read failures and clear only after a successful empty lease response.
4. Update command/API contracts and localized status text only where required by existing UI patterns.
5. Run focused formatting/typecheck/build and inspect the final diff against the already-dirty `main` worktree; do not overwrite unrelated WIP.

6. Replace per-profile binding, lease, and runtime reads with one bounded workspace status batch; authorize each profile, limit the batch to 250 profiles, and cap concurrent lease reads at 32.

Status: implementation complete; frontend typecheck and generated-contract checks pass. Rust compilation remains pending because another active Cargo build/test is using the shared target directory. The repository-wide format check also reports pre-existing formatting drift in the concurrent WIP, so files were not mass-formatted.
