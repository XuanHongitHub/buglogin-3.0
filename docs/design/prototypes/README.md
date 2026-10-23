# Prototype references removed

The former static desktop prototype was removed because it introduced a second visual authority and fake production-looking records.

Do not recreate or reuse it. UI implementation must use:

1. `.tmp/donutbrowser-upstream` as the read-only desktop baseline;
2. `docs/design/ui-authority.md` for BugLogin extensions and web/desktop boundaries;
3. the approved-reference checklist, real/sanitized fixtures, and screenshot/interaction evidence.

Prototype-only HTML, fake jobs, fake connection states, fake PIDs, and simulated progress are not production references.
