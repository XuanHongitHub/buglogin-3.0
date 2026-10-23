# DonutBrowser live reference plan

- [x] Define an isolated live reference separate from the pinned BugLogin baseline.
- [x] Add a guarded updater, state metadata, and daily hidden scheduled-task option.
- [x] Fetch current upstream `main` and install the scheduled task on this Windows machine.
- [x] Verify the stored commit matches the upstream checkout and inspect task configuration.

## Verification evidence

- Upstream remote: `https://github.com/zhom/donutbrowser.git`
- Branch: `main`
- Current checkout and recorded commit match: `89f44466a7d4e56150dabc54528dc6300c292eda`
- Scheduled task: `BugLogin DonutBrowser Reference Daily Update`, hidden, daily at 04:00 local time, current user's interactive session.
