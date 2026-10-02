#!/usr/bin/env python3
"""Run a test command on Linux and clean up its browser/driver descendants.

Usage: python3 scripts/with-test-cleanup.py -- COMMAND [ARG ...]
Each invocation has a private environment marker, including detached children.
Only that invocation's processes are eligible for cleanup.
"""

import ctypes
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import uuid


def identity(entry):
    try:
        fields = (entry / "stat").read_text().rsplit(") ", 1)[1].split()
        if fields[0] == "Z":
            return None
        return int(entry.name), int(fields[1]), fields[19]
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        return None


def owned_processes(marker, owner=None):
    """Include detached/adopted descendants even if their environment is hidden."""
    processes = {}
    children = {}
    owned = set()
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        info = identity(entry)
        if info is None:
            continue
        pid, parent, start = info
        processes[pid] = start
        children.setdefault(parent, []).append(pid)
        try:
            if marker is not None and marker in (entry / "environ").read_bytes().split(b"\0"):
                owned.add(pid)
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            pass
    pending = list(children.get(os.getpid() if owner is None else owner, []))
    while pending:
        pid = pending.pop()
        owned.add(pid)
        pending.extend(children.get(pid, []))
    return [(pid, processes[pid]) for pid in owned]


def stop_owned(identities, sig):
    for pid, start in identities:
        # Recheck start time immediately before signalling to guard PID reuse.
        info = identity(Path("/proc") / str(pid))
        if info is not None and info[2] == start:
            try:
                os.kill(pid, sig)
            except (ProcessLookupError, PermissionError):
                pass


def cleanup(marker):
    deadline = time.monotonic() + 3
    while identities := owned_processes(marker):
        stop_owned(identities, signal.SIGTERM)
        if time.monotonic() >= deadline:
            stop_owned(owned_processes(marker), signal.SIGKILL)
            kill_deadline = time.monotonic() + 2
            while owned_processes(marker) and time.monotonic() < kill_deadline:
                time.sleep(0.05)
            if owned_processes(marker):
                raise RuntimeError("test processes survived cleanup")
            break
        time.sleep(0.05)


def main():
    args = sys.argv[1:]
    if args and args[0] == "--":
        args.pop(0)
    if not args:
        print(__doc__, file=sys.stderr)
        return 2
    if sys.platform != "linux":
        print("with-test-cleanup requires Linux /proc", file=sys.stderr)
        return 2
    # Adopt orphaned grandchildren so killed browsers can be reaped here.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "cannot adopt test descendants")
    marker_value = uuid.uuid4().hex
    marker = f"OGRE_TEST_RUN_ID={marker_value}".encode()
    env = dict(os.environ, OGRE_TEST_RUN_ID=marker_value)
    interrupted = None
    child = None
    cleanup_failed = False

    def interrupt(sig, _frame):
        nonlocal interrupted
        interrupted = sig
        if child is not None and child.poll() is None:
            try:
                child.send_signal(sig)
            except ProcessLookupError:
                pass

    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP, signal.SIGQUIT):
        if signal.getsignal(sig) != signal.SIG_IGN:
            signal.signal(sig, interrupt)
    try:
        child = subprocess.Popen(args, env=env)
        while child.poll() is None and interrupted is None:
            time.sleep(0.05)
    except FileNotFoundError as error:
        print(error, file=sys.stderr)
        return 127
    except OSError as error:
        print(error, file=sys.stderr)
        return 126
    finally:
        try:
            cleanup(marker)
        except (OSError, RuntimeError) as error:
            cleanup_failed = True
            print(f"test cleanup failed: {error}", file=sys.stderr)
        if child is not None:
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        # All remaining children are adopted descendants of this command.
        while True:
            try:
                pid, _ = os.waitpid(-1, os.WNOHANG)
                if pid == 0:
                    break
            except ChildProcessError:
                break
    if interrupted is not None:
        signal.signal(interrupted, signal.SIG_DFL)
        os.kill(os.getpid(), interrupted)
        return 128 + interrupted
    if cleanup_failed and child.returncode == 0:
        return 125
    return child.returncode if child.returncode >= 0 else 128 - child.returncode


if __name__ == "__main__":
    sys.exit(main())
