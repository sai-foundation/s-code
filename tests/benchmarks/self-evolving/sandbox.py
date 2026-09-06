"""Workspace-external grading with no network or access to user files."""
from __future__ import annotations
import json
import os
import re
from pathlib import Path
import shutil
import sys

from bounded_process import run as bounded_run


def command(argv, readable, scratch, workspace=None):
    readable = {str(Path(path).resolve()) for path in readable}
    readable.add(str(Path(__file__).with_name("bounded_process.py").resolve()))
    readable.update(str(Path(path).resolve()) for path in (sys.prefix, sys.base_prefix))
    if sys.platform == "darwin":
        profile = '(version 1)(deny default)(import "system.sb")(allow process*)(allow signal (target children))(allow sysctl-read)(allow mach-lookup)(allow file-read-metadata)'
        readable.update(("/System", "/usr/bin", "/usr/lib", "/usr/share", "/bin", "/sbin", "/dev", "/Library/Developer/CommandLineTools"))
        for path in sorted(readable):
            profile += f'(allow file-read* (subpath {json.dumps(path)}))'
        profile += f'(allow file-read* file-write* (subpath {json.dumps(str(scratch.resolve()))}))'
        if workspace is not None:
            # Existing fixture tests create TemporaryDirectory(dir=workspace).
            # Permit those scratch children, keeping source and tests read-only.
            pattern = "^" + re.escape(str(workspace.resolve())) + "/tmp[^/]+(/|$)"
            profile += f'(allow file-write* (regex {json.dumps(pattern)}))'
        return ["/usr/bin/sandbox-exec", "-p", profile, *argv]
    if sys.platform.startswith("linux"):
        bwrap = shutil.which("bwrap")
        if not bwrap: raise RuntimeError("Bubblewrap is required for isolated grading")
        args = [bwrap, "--die-with-parent", "--unshare-all", "--new-session", "--proc", "/proc", "--dev", "/dev"]
        readable.update(path for path in ("/usr", "/bin", "/lib", "/lib64") if Path(path).exists())
        for path in sorted(readable): args.extend(["--ro-bind",path,path])
        if workspace is not None:
            # A private tmpfs root supports TemporaryDirectory(dir=workspace),
            # while every existing source/test entry remains a read-only mount.
            # Unlike macOS's name filter, Linux permits other *new* top-level
            # entries too; they are ephemeral and cannot alter the candidate.
            workspace = Path(workspace).resolve()
            args.extend(["--tmpfs", str(workspace)])
            for entry in sorted(workspace.iterdir()):
                if entry.is_symlink():
                    raise RuntimeError("Candidate symlinks are not permitted during grading")
                args.extend(["--ro-bind", str(entry), str(entry)])
        return [*args, "--bind",str(scratch),str(scratch), "--chdir",str(scratch), "--", *argv]
    raise RuntimeError("Grading requires the macOS or Linux sandbox")


def run(argv, readable, scratch, timeout=150, workspace=None):
    scratch.mkdir(parents=True, exist_ok=True)
    environment = {name:os.environ[name] for name in ("PATH", "LANG", "SYSTEMROOT", "WINDIR") if name in os.environ}
    environment.update(TMPDIR=str(scratch.resolve()), PYTHONDONTWRITEBYTECODE="1", PYTHONUTF8="1")
    return bounded_run(command(argv,readable,scratch,workspace), cwd=scratch,
                       env=environment, timeout=timeout,
                       max_output_bytes=4 * 1024 * 1024)
