import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("api_watchdog", Path(__file__).with_name("api-watchdog.py"))
watchdog = importlib.util.module_from_spec(spec)
spec.loader.exec_module(watchdog)


class RecoveryTests(unittest.TestCase):
    def test_recovery_budget_limits_and_cooldown(self):
        self.assertTrue(watchdog.recovery_allowed([], 1000))
        self.assertFalse(watchdog.recovery_allowed([950], 1000))
        self.assertTrue(watchdog.recovery_allowed([800], 1000))
        self.assertFalse(watchdog.recovery_allowed([200, 400, 600], 1000))
        self.assertTrue(watchdog.recovery_allowed([50, 400, 600], 1000))

    def test_three_failures_request_only_one_bounded_restart(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "api"
            binary.write_bytes(b"known-api")
            monitor = watchdog.Watchdog(watchdog.binary_digest(binary), 2000, Path(directory) / "history.json")
            state = {"MainPID": "12", "ActiveState": "active", "SubState": "running", "ExecMainStartTimestampMonotonic": "1000000"}
            with patch.object(watchdog, "BINARY", binary), patch.object(watchdog, "service_state", return_value=state), patch.object(watchdog, "healthy", return_value=False), patch.object(watchdog.time, "time", return_value=1000), patch.object(watchdog.time, "monotonic", return_value=100), patch.object(watchdog, "command") as run, patch.object(watchdog, "log"):
                run.return_value.stdout = "LISTEN 128 128"
                run.return_value.returncode = 0
                monitor.tick()
                monitor.tick()
                self.assertEqual(run.call_count, 0)
                monitor.tick()
                self.assertEqual(run.call_count, 2)
                self.assertEqual(run.call_args.args[0][-2:], ["try-restart", watchdog.UNIT])
                for _ in range(3):
                    monitor.tick()
                self.assertEqual(run.call_count, 2)
                self.assertEqual(watchdog.load_history(monitor.state_path), [1000])

    def test_inactive_service_is_not_started_and_new_binary_retires_monitor(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "api"
            binary.write_bytes(b"old-api")
            monitor = watchdog.Watchdog(watchdog.binary_digest(binary), 2000, Path(directory) / "history.json")
            state = {"MainPID": "0", "ActiveState": "inactive", "SubState": "dead"}
            with patch.object(watchdog, "BINARY", binary), patch.object(watchdog, "service_state", return_value=state), patch.object(watchdog.time, "time", return_value=1000), patch.object(watchdog, "healthy") as probe, patch.object(watchdog, "command") as run, patch.object(watchdog, "log"):
                self.assertTrue(monitor.tick())
                probe.assert_not_called()
                run.assert_not_called()
                binary.write_bytes(b"new-release-api")
                self.assertFalse(monitor.tick())
                run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
