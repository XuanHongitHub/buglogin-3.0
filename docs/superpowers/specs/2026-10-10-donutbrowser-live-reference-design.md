# DonutBrowser live reference design

## Goal

Keep a separate local checkout of current DonutBrowser `main` available for comparison with BugLogin's pinned upstream baseline.

## Boundaries

- Store the live checkout in `F:\Herd\DonutBrowser-Reference\source`, outside the BugLogin application tree.
- Keep `F:\Herd\buglogin\.tmp\donutbrowser-upstream` read-only and unchanged.
- Do not change the BugLogin upstream baseline manifest or product source when the live reference updates.
- Treat the live `main` checkout as reference material, not release evidence or a production dependency.

## Update behavior

- `Update-Reference.ps1` clones upstream `main` on first run, then fetches the latest `main` commit on later runs.
- Refuse to update if the existing checkout has local modifications or its origin differs from the canonical upstream URL.
- Verify the fetched commit ID and checked-out commit before recording state.
- Record repository, branch, commit, fetch time, and reference-only status in `reference-state.json`; append success or failure details to `update.log`.
- An optional installation switch registers a hidden daily Windows task at 04:00 local time under the current user's interactive session.

## Failure handling

Network, Git, validation, or scheduler errors are logged and surfaced with a nonzero script exit. A fetch failure leaves the previously checked-out reference intact. A modified reference is preserved and blocks automatic replacement.
