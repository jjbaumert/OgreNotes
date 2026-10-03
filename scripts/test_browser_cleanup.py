#!/usr/bin/env python3
"""Interrupt a real browser test and prove that its processes are gone."""

import argparse
import importlib.util
from pathlib import Path
import signal
import subprocess
import sys
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("browser", choices=["firefox", "chrome"])
parser.add_argument("--startup-timeout", type=float, default=900, help="seconds allowed for build and browser startup")
parser.add_argument("--complete", action="store_true", help="verify a successful run instead of interrupting")
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
wrapper = root / "scripts/with-test-cleanup.py"
spec = importlib.util.spec_from_file_location("test_cleanup", wrapper)
cleanup = importlib.util.module_from_spec(spec)
sys.dont_write_bytecode = True
spec.loader.exec_module(cleanup)
command = (["wasm-pack", "test", "--headless", "--firefox", "--", "--test", "browser"]
           if args.browser == "firefox" else [str(root / "scripts/wasm-test-chrome.sh"), "--test", "browser"])
process = subprocess.Popen([sys.executable, str(wrapper), "--", *command], cwd=root / "frontend")
identities = set()
try:
    deadline = time.monotonic() + args.startup_timeout
    while time.monotonic() < deadline:
        identities.update(cleanup.owned_processes(None, owner=process.pid))
        browsers = []
        for pid, start in identities:
            try:
                name = Path(f"/proc/{pid}/comm").read_text().strip()
                if name.startswith("firefox") or name in ("chrome", "chromium", "chromium-browse", "chrome-headless"):
                    browsers.append(pid)
            except (FileNotFoundError, ProcessLookupError, PermissionError):
                pass
        if browsers:
            break
        if process.poll() is not None:
            if args.complete and process.returncode:
                break  # Preserve build/driver failures after checking descendants.
            raise RuntimeError("test exited before a real browser started")
        time.sleep(.05)
    else:
        raise RuntimeError("browser startup timed out")
    # Keep collecting helpers while the browser starts, before cancellation.
    for _ in range(10):
        identities.update(cleanup.owned_processes(None, owner=process.pid))
        time.sleep(.05)
    if not args.complete:
        process.send_signal(signal.SIGTERM)
    deadline = None if args.complete else time.monotonic() + 15
    while process.poll() is None:
        identities.update(cleanup.owned_processes(None, owner=process.pid))
        if deadline is not None and time.monotonic() > deadline:
            raise RuntimeError("test exit timed out")
        time.sleep(.05)
    expected = 0 if args.complete else -signal.SIGTERM
    if not args.complete and process.wait() != expected:
        raise RuntimeError(f"test interruption status was not preserved: {process.returncode}")
    for pid, start in identities:
        try:
            fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
            if fields[19] == start:
                raise RuntimeError(f"test descendant survived: {pid}")
        except (FileNotFoundError, ProcessLookupError):
            pass
    if args.complete and process.returncode:
        print(f"browser tests failed with status {process.returncode}", file=sys.stderr)
        sys.exit(process.returncode if process.returncode > 0 else 128 - process.returncode)
    print(f"PASS {args.browser}: {'completed' if args.complete else 'interrupted'} real test, zero observed owned processes")
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=15)
