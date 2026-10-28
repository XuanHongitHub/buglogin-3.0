# Temporary BugLogin API recovery watchdog

Operational containment authorized by the user on 2026-10-28. Source: `scripts/ops/api-watchdog.py`.

Run the reviewed helper as a transient systemd service on `bugideas-prod`, with the current `/opt/buglogin3/buglogin-api` SHA-256 and an explicit expiry no later than seven days. The unit is `buglogin3-api-watchdog.service`; it runs as root solely to read fixed service state and request `try-restart buglogin3-api.service`. Deliver the committed source in the transient command; do not overwrite installed API binaries or persistent production configuration files.

Recommended transient properties: `Restart=on-failure`, `RestartSec=10s`, `CPUQuota=5%`, `MemoryMax=64M`, `TasksMax=8`, `RuntimeDirectory=buglogin3-api-watchdog`, `RuntimeDirectoryMode=0700`, `RuntimeDirectoryPreserve=restart`, `NoNewPrivileges=yes`, `PrivateTmp=yes`, `ProtectSystem=strict`, `ProtectHome=yes`. Persist restart history only in the private runtime directory.

- Probe origin `/v1/health` every 10 seconds, with a four-second request deadline. Check the exact expected 200 JSON response; never probe readiness as the recovery trigger.
- Wait for three consecutive failures on the same active/running PID, then capture socket backlog and request a restart. Allow a 60-second startup grace.
- Require at least 120 seconds between requests and at most three restart requests per 15 minutes; persist the budget before restarting. Continue observing when rate-limited.
- Skip inactive/starting/stopping service states so intentional operator actions win. Recheck service/PID and binary before requesting recovery.
- Retire successfully on binary replacement or expiry. A service restart alone does not retire it. Runtime units disappear on host reboot; deployment must check retirement explicitly.

Verification: run `python -B scripts/ops/test_api_watchdog.py` before installation. After installation inspect the unit journal, unchanged API PID during healthy checks, origin/public health and listen backlog. Do not deliberately hang production to validate the restart branch; use the controlled test.

Rollback: `sudo systemctl stop buglogin3-api-watchdog.service`. Inspect logs with `journalctl -u buglogin3-api-watchdog.service`. This does not stop the API. CPU quota containment is a separate runtime override recorded in CODEX_CHAT.md.

This automatically recovers a hung process; it does not guarantee uninterrupted requests or fix blocking I/O. Requests in progress may fail during recovery, and clients must reconcile ambiguous writes using existing idempotency/manifests. Watchdog expiry/budget exhaustion must be reported, not hidden as healthy.
