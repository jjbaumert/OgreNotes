"""Exercise teardown against real detached processes, including cancellation."""

import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

WRAPPER = Path(__file__).with_name("with-test-cleanup.py")
BACKGROUND = """
import ctypes, os, pathlib, signal, sys, time
ctypes.CDLL(None).prctl(4, 0, 0, 0, 0)  # Hide /proc/environ like sandboxed browser children.
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pathlib.Path(sys.argv[1]).write_text(str(os.getpid()))
while True: time.sleep(1)
"""
COMMAND = """
import pathlib, subprocess, sys, time
subprocess.Popen([sys.executable, '-c', sys.argv[1], sys.argv[2]],
                 start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                 env={} if sys.argv[4] == "scrubbed" else None)
while not pathlib.Path(sys.argv[2]).exists(): time.sleep(.01)
if sys.argv[3] == 'wait':
    while True: time.sleep(1)
sys.exit(int(sys.argv[3]))
"""


@unittest.skipUnless(sys.platform == "linux", "Linux process ownership")
class CleanupTests(unittest.TestCase):
    def exercise(self, mode, nested=False, interrupt=None, scrubbed=False):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "detached.pid"
            unrelated = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
            args = [sys.executable, str(WRAPPER), "--"]
            if nested:
                args += [sys.executable, str(WRAPPER), "--"]
            args += [sys.executable, "-c", COMMAND, BACKGROUND, str(pid_file), mode, "scrubbed" if scrubbed else "inherited"]
            process = subprocess.Popen(args)
            owned_pid = None
            try:
                deadline = time.monotonic() + 10
                while not pid_file.exists():
                    self.assertIsNone(process.poll(), "command exited before fixture started")
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.02)
                owned_pid = int(pid_file.read_text())
                if interrupt is not None:
                    process.send_signal(interrupt)
                expected = -interrupt if interrupt else int(mode)
                self.assertEqual(process.wait(timeout=15), expected)
                self.assertFalse(Path(f"/proc/{owned_pid}").exists(), "detached child survived cleanup")
                self.assertIsNone(unrelated.poll(), "cleanup killed another process")
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                if owned_pid and Path(f"/proc/{owned_pid}").exists():
                    os.kill(owned_pid, signal.SIGKILL)
                unrelated.terminate()
                unrelated.wait()

    def test_success_cleans_detached_child(self):
        self.exercise("0")

    def test_failure_preserves_exit_code(self):
        self.exercise("7")

    def test_sigint_cleans_children(self):
        self.exercise("wait", interrupt=signal.SIGINT)

    def test_sigterm_cleans_children(self):
        self.exercise("wait", interrupt=signal.SIGTERM)

    def test_nested_wrappers_clean_all_children(self):
        self.exercise("0", nested=True)

    def test_scrubbed_environment_is_still_cleaned(self):
        self.exercise("0", scrubbed=True)

    def test_concurrent_run_stays_alive(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "other.pid"
            other = subprocess.Popen([sys.executable, str(WRAPPER), "--", sys.executable,
                                      "-c", COMMAND, BACKGROUND, str(pid_file), "wait", "inherited"])
            try:
                deadline = time.monotonic() + 10
                while not pid_file.exists():
                    self.assertIsNone(other.poll())
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.02)
                pid = int(pid_file.read_text())
                self.exercise("0")
                self.assertIsNone(other.poll())
                self.assertTrue(Path(f"/proc/{pid}").exists())
            finally:
                other.terminate()
                self.assertEqual(other.wait(timeout=15), -signal.SIGTERM)

    def test_unexecutable_command_returns_126(self):
        with tempfile.TemporaryDirectory() as directory:
            command = Path(directory) / "command"
            command.write_text("#!/bin/sh\nexit 0\n")
            command.chmod(0o600)
            result = subprocess.run([sys.executable, str(WRAPPER), "--", str(command)],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.assertEqual(result.returncode, 126)

    def test_invalid_executable_returns_126(self):
        with tempfile.TemporaryDirectory() as directory:
            command = Path(directory) / "command"
            command.write_text("invalid executable")
            command.chmod(0o700)
            result = subprocess.run([sys.executable, str(WRAPPER), "--", str(command)],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.assertEqual(result.returncode, 126)

    def test_observer_preserves_early_build_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            command = Path(directory) / "wasm-pack"
            command.write_text("#!/bin/sh\nexit 7\n")
            command.chmod(0o700)
            observer = WRAPPER.with_name("test_browser_cleanup.py")
            env = dict(os.environ, PATH=directory + os.pathsep + os.environ["PATH"])
            result = subprocess.run([sys.executable, str(observer), "firefox", "--complete"],
                                    env=env, capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 7)
            self.assertIn("browser tests failed with status 7", result.stderr)
            self.assertNotIn("Traceback", result.stderr)

    def test_nohup_signal_ignore_is_preserved(self):
        command = "import signal,sys; sys.exit(42 if signal.getsignal(signal.SIGHUP) == signal.SIG_IGN else 43)"
        result = subprocess.run([sys.executable, str(WRAPPER), "--", sys.executable, "-c", command],
                                preexec_fn=lambda: signal.signal(signal.SIGHUP, signal.SIG_IGN))
        self.assertEqual(result.returncode, 42)

    def test_missing_command_returns_127(self):
        result = subprocess.run([sys.executable, str(WRAPPER), "--", "/nonexistent/ogre-test"],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.assertEqual(result.returncode, 127)


if __name__ == "__main__":
    unittest.main()
