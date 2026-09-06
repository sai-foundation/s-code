"""Capture grading processes with a shared byte quota and process-group cleanup."""
from __future__ import annotations

import contextlib
import math
import os
import selectors
import signal
import subprocess
import time

OUTPUT_LIMIT = 125
DEADLINE = 124
DEFAULT_OUTPUT_BYTES = 2 * 1024 * 1024


def _kill_group(group):
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(group, signal.SIGKILL)


def _terminate(process, include_nested):
    groups = {process.pid}
    if include_nested and process.poll() is None:
        # The external grader starts separately bounded CLI groups. Find those
        # groups before killing their parent, while the ancestry is still known.
        try:
            listing = subprocess.run(
                ["/bin/ps", "-eo", "pid=,ppid=,pgid="],
                capture_output=True, text=True, timeout=2, check=True,
            ).stdout
            rows = [tuple(map(int, line.split())) for line in listing.splitlines()]
            descendants = {process.pid}
            while True:
                children = {pid for pid, parent, _ in rows if parent in descendants}
                enlarged = descendants | children
                if enlarged == descendants:
                    break
                descendants = enlarged
            # Never signal a group inherited from an unrelated parent process.
            groups.update(group for pid, _, group in rows
                          if pid in descendants and group in descendants)
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
    for group in sorted(groups, reverse=True):
        _kill_group(group)
    with contextlib.suppress(subprocess.TimeoutExpired):
        process.wait(timeout=2)


def run(argv, *, cwd=None, env=None, timeout=20,
        max_output_bytes=DEFAULT_OUTPUT_BYTES):
    """Return CompletedProcess; 124 is a deadline, 125 is an output quota.

    The quota covers stdout and stderr together, before decoding. Only a short
    diagnostic is appended beyond it. Pipes are read without unbounded buffers;
    every invocation owns a process group, including nested candidate CLIs.
    """
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("timeout must be finite and positive")
    if type(max_output_bytes) is not int or max_output_bytes <= 0:
        raise ValueError("output quota must be a positive integer")
    process = subprocess.Popen(
        argv, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        start_new_session=True,
    )
    selector = selectors.DefaultSelector()
    captured = {"stdout": bytearray(), "stderr": bytearray()}
    used = 0
    reason = None
    deadline = time.monotonic() + timeout
    try:
        for name, pipe in (("stdout", process.stdout), ("stderr", process.stderr)):
            os.set_blocking(pipe.fileno(), False)
            selector.register(pipe, selectors.EVENT_READ, name)
        while selector.get_map() or process.poll() is None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                reason = DEADLINE
                break
            for key, _ in selector.select(min(remaining, 0.1)):
                data = os.read(key.fileobj.fileno(), 64 * 1024)
                if not data:
                    selector.unregister(key.fileobj)
                    continue
                kept = data[:max_output_bytes - used]
                captured[key.data].extend(kept)
                used += len(kept)
                if len(kept) != len(data):
                    reason = OUTPUT_LIMIT
                    break
            if reason is not None:
                break
        if reason is not None:
            _terminate(process, include_nested=True)
        else:
            process.wait()
    finally:
        selector.close()
        process.stdout.close()
        process.stderr.close()
        # Also clean up children left in the group after a successful parent exit.
        _terminate(process, include_nested=process.poll() is None)
    stdout = captured["stdout"].decode("utf-8", errors="replace")
    stderr = captured["stderr"].decode("utf-8", errors="replace")
    if reason == OUTPUT_LIMIT:
        stderr += "\ncandidate-output-limit: process output exceeded its byte quota\n"
    elif reason == DEADLINE:
        stderr += "\nprocess-deadline: process exceeded its time limit\n"
    return subprocess.CompletedProcess(argv, reason or process.returncode, stdout, stderr)
