# Runtime Adapter Approval

## Scope

Prevent migration from activating runtimes whose desktop adapter is not verified.

## Contract

- `wayfern` is the only launchable legacy runtime in this wave.
- `bugox`, `bugium`, `bugmini`, `bugthor`, and `bughelium` remain unsupported until a runtime-specific capability matrix and native artifact evidence exist.
- `cloak`, `thorium`, and `helium` remain the original runtime string; aliases are not normalized without a verified adapter mapping.
- Unsupported and unknown profiles use preserve-only authority; they never enter active `Profile` storage.
- Inspect classification and transformed `migration_class` must agree.
