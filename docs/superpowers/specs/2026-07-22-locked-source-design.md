# Locked Source File Policy

## Contract

- Snapshot must read every source file from one immutable view.
- Windows sharing or lock violations return `migration_source_locked` with status `423`.
- A locked source blocks package creation and cutover before destination writes.
- No retry may silently omit the locked file.
- Source bytes and destination state remain unchanged after the blocked attempt.
