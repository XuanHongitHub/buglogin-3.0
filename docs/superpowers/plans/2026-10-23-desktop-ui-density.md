# Desktop UI density cleanup plan

1. [x] Update About and Create Profile components using existing tokens/components.
2. [x] Add a small display-only runtime-version formatter and concise localization.
3. [x] Remove the duplicate inline download percentage while retaining modal/toast
   progress.
4. [x] Run formatting/typecheck and targeted tests; inspect the scoped diff without
   touching unrelated dirty worktree changes.

## Verification

- Locale JSON parsing passes for English and Vietnamese.
- Scoped diff has no whitespace errors.
- Full TypeScript check remains blocked by pre-existing errors in unrelated
  entitlement, bookmark, sync, and trash modules.
- Biome reports existing whole-file line-ending/format differences on the two
  touched TypeScript files; no formatter rewrite was applied.
