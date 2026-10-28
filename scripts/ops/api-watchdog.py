#!/usr/bin/env python3
"""Temporary origin-liveness recovery for the existing BugLogin API binary."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

UNIT = "buglogin3-api.service"
BINARY = Path("/opt/buglogin3/buglogin-api")
URL = "http://127.0.0.1:12343/v1/health"


def log(event, **fields):
    print(json.dumps({"event": event, **fields}, sort_keys=True), flush=True)


def command(args, timeout=5):
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout, check=False)


def service_state():
    result = command([
        "/usr/bin/systemctl", "show", UNIT,
        "-p", "ActiveState", "-p", "SubState", "-p", "MainPID",
        "-p", "ExecMainStartTimestampMonotonic",
    ])
    if result.returncode:
        raise RuntimeError("service_state_unavailable")
    return dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)


def healthy():
    result = command([
        "/usr/bin/curl", "--silent", "--show-error", "--noproxy", "*",
        "--connect-timeout", "1", "--max-time", "4", "--max-filesize", "4096",
        "--write-out", "\n%{http_code}", URL,
    ], timeout=6)
    if result.returncode:
        return False
    try:
        body, status = result.stdout.rsplit("\n", 1)
        payload = json.loads(body)
        return status == "200" and payload.get("status") == "ok" and payload.get("service") == "buglogin-api"
    except (ValueError, AttributeError):
        return False


def recovery_allowed(history, now):
    recent = [stamp for stamp in history if now - stamp < 900]
    # Clock moving backwards conservatively suppresses recovery too.
    return len(recent) < 3 and (not recent or now - max(recent) >= 120)


def save_history(path, history):
    temporary = path.with_suffix(".tmp")
    with temporary.open("w", encoding="utf-8") as stream:
        os.chmod(temporary, 0o600)
        json.dump(history, stream)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


def load_history(path):
    if not path.exists():
        return []
    history = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(history, list) or any(type(value) not in (int, float) for value in history):
        raise ValueError("invalid_restart_history")
    return history


def binary_digest(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class Watchdog:
    def __init__(self, expected_sha256, expires_at, state_path):
        self.expected_sha256 = expected_sha256
        self.expires_at = expires_at
        self.state_path = state_path
        self.history = load_history(state_path)
        self.binary_stat = None
        self.pid = None
        self.failures = 0

    def tick(self):
        now = time.time()
        if now >= self.expires_at:
            log("expired")
            return False
        stat = BINARY.stat()
        identity = (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns)
        if identity != self.binary_stat:
            if binary_digest(BINARY) != self.expected_sha256:
                log("retired_binary_changed")
                return False
            self.binary_stat = identity
        state = service_state()
        pid = state.get("MainPID", "0")
        if pid != self.pid:
            self.pid = pid
            self.failures = 0
            log("observed_pid", pid=pid)
        # Never start an intentionally stopped service or interfere with deployment/startup.
        if state.get("ActiveState") != "active" or state.get("SubState") != "running" or pid == "0":
            self.failures = 0
            return True
        started = int(state.get("ExecMainStartTimestampMonotonic", "0")) / 1_000_000
        if not started or time.monotonic() - started < 60:
            self.failures = 0
            return True
        if healthy():
            if self.failures:
                log("health_recovered", previous_failures=self.failures)
            self.failures = 0
            return True
        self.failures += 1
        log("health_failed", pid=pid, consecutive=self.failures)
        if self.failures < 3:
            return True
        self.history = [stamp for stamp in self.history if now - stamp < 900]
        if not recovery_allowed(self.history, now):
            log("restart_rate_limited", recent_restarts=len(self.history))
            return True
        # Recheck identity and state after the probe so manual stops/deploys win.
        latest = service_state()
        if (latest.get("MainPID") != pid or latest.get("ActiveState") != "active"
                or latest.get("SubState") != "running"):
            self.failures = 0
            return True
        if binary_digest(BINARY) != self.expected_sha256:
            log("retired_binary_changed")
            return False
        diagnostic = command(["/usr/bin/ss", "-lnt", "( sport = :12343 )"])
        log("restart_evidence", pid=pid, listen=diagnostic.stdout.strip()[:1500])
        # Persist before requesting restart, preventing storms if the watchdog itself crashes.
        self.history.append(time.time())
        save_history(self.state_path, self.history)
        result = command(["/usr/bin/systemctl", "--no-block", "try-restart", UNIT])
        log("restart_requested", pid=pid, accepted=result.returncode == 0)
        self.failures = 0
        return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--expires-at", type=int, required=True)
    parser.add_argument("--state-path", type=Path, default=Path("/run/buglogin3-api-watchdog/history.json"))
    args = parser.parse_args()
    if len(args.expected_sha256) != 64 or any(c not in "0123456789abcdef" for c in args.expected_sha256):
        parser.error("expected-sha256 must be a lowercase SHA-256 digest")
    watchdog = Watchdog(args.expected_sha256, args.expires_at, args.state_path)
    log("started", expires_at=args.expires_at, unit=UNIT, failure_threshold=3, interval_seconds=10)
    while True:
        started = time.monotonic()
        try:
            if not watchdog.tick():
                return
        except (OSError, ValueError, subprocess.SubprocessError, RuntimeError) as error:
            # Failed diagnostics never authorize a restart. Log class only, no environment/body.
            watchdog.failures = 0
            log("monitor_error", error_type=type(error).__name__)
        time.sleep(max(0.1, 10 - (time.monotonic() - started)))


if __name__ == "__main__":
    main()
