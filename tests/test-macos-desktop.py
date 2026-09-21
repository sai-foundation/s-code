#!/usr/bin/env python3
"""Build/package the native desktop and check its real engine with a local model.

Uses the existing Cargo target cache and SWIFT override, with all runtime state
outside the checkout. No provider account, network model, or Python dependency
is needed. --skip-build uses an already packaged debug app and DesktopChecks.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
SHUTDOWN_TIMEOUT = 12
# A fresh opener ignores contributor proxy settings for these loopback requests.
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def wait_until(check, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(f"Timed out waiting for {description}")


def running(pid):
    # Orphaned children can briefly be zombies until launchd reaps them.
    result = subprocess.run(["/bin/ps", "-p", str(pid), "-o", "stat="],
                            capture_output=True, text=True, timeout=5)
    return bool(result.stdout.strip()) and not result.stdout.lstrip().startswith("Z")


def kill_group(pid, sig):
    try:
        os.killpg(pid, sig)
    except ProcessLookupError:
        pass


def stop(process):
    """Bound cleanup, including children still in this owned process group."""
    # Reap an already exited leader before signaling: macOS can return EPERM
    # when a process group consists only of its unreaped zombie leader.
    process.poll()
    kill_group(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=SHUTDOWN_TIMEOUT)
    except subprocess.TimeoutExpired:
        kill_group(process.pid, signal.SIGKILL)
        process.wait(timeout=5)
    finally:
        kill_group(process.pid, signal.SIGKILL)
        if process.stdin:
            process.stdin.close()


def run(command, *, timeout, **kwargs):
    process = subprocess.Popen(command, start_new_session=True, **kwargs)
    try:
        code = process.wait(timeout=timeout)
        if code:
            raise subprocess.CalledProcessError(code, command)
    finally:
        stop(process)


def fixture_endpoint(process, timeout=30):
    """Read a complete port announcement without blocking past the deadline."""
    deadline = time.monotonic() + timeout
    announcement = bytearray()
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not selector.select(timeout=remaining):
                raise AssertionError("Model fixture did not announce its port before the startup deadline")
            chunk = os.read(process.stdout.fileno(), 128)
            if not chunk:
                raise AssertionError(f"Model fixture closed stdout before readiness (exit status: {process.poll()})")
            announcement.extend(chunk)
            if b"\n" in announcement:
                line = announcement.split(b"\n", 1)[0]
                if not line.isdigit() or not 1 <= int(line) <= 65535:
                    raise AssertionError(f"Invalid model fixture port announcement: {line!r}")
                return f"http://127.0.0.1:{int(line)}/v1"
            if len(announcement) >= 128:
                raise AssertionError("Model fixture port announcement was too long")


def request(base, path, token="", body=None):
    headers = {"Authorization": f"Bearer {token}", "X-S-Code-CSRF": "1"}
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=data, headers=headers)
    with HTTP.open(req, timeout=3) as response:
        return json.load(response)


def daemon_environment(directory, endpoint):
    home, state, runtime, workspaces = (directory / name for name in ("Engine", "state", "run", "Workspaces"))
    for path in (directory, home, state, runtime, workspaces):
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    # Mirror Engine.swift's isolated launch contract instead of inheriting local
    # provider secrets, config overrides, or the repository's runtime settings.
    return {
        "HOME": str(Path.home()), "USER": os.environ.get("USER", "desktop-test"),
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "en_US.UTF-8",
        "TMPDIR": str(directory), "S_CODE_HOME": str(home), "S_CODE_STATE_DIR": str(state),
        "S_CODE_RUNTIME_DIR": str(runtime), "S_CODE_WORKSPACES_DIR": str(workspaces),
        "S_CODE_DATABASE_URL": "sqlite://" + str(state / "desktop.sqlite"),
        "S_CODE_DAEMON_LISTEN": "127.0.0.1:0", "S_CODE_TOKEN": uuid.uuid4().hex,
        "S_CODE_MODEL_PROVIDER": "openai_compatible", "S_CODE_MODEL_BASE_URL": endpoint,
        "S_CODE_MODEL": "desktop-fixture", "S_CODE_DESKTOP_LIFETIME": "stdin",
    }


def connection_ready(directory, token, owner):
    if owner.poll() is not None:
        raise AssertionError(f"Daemon owner exited before readiness: {owner.returncode}")
    try:
        connection = json.loads((directory / "run/daemon.json").read_text())
        if connection["token"] != token:
            raise AssertionError("Unexpected daemon credentials")
        request(connection["daemon_url"], "/v1/health", token)
        return connection
    except (FileNotFoundError, json.JSONDecodeError, urllib.error.URLError, TimeoutError):
        return None


def start_terminal(connection, directory, child_pids):
    scope = {"organization_id": "local", "team_id": "local", "actor_id": "desktop-lifetime",
             "goal_id": None, "task_id": None}
    base, token = connection["daemon_url"], connection["token"]
    workspace = directory / "Workspaces"
    session = request(base, "/v1/sessions", token, {
        "scope": scope, "mode": "work", "workspace_uri": workspace.as_uri() + "/",
        "title": "Desktop lifetime", "model": "desktop-fixture",
    })
    terminal = {
        "session_id": session["id"], "program": "/bin/sh",
        "args": ["-c", "echo $$ > terminal.pid; /bin/sleep 120 & echo $! > child.pid; wait"],
        "environment_handles": {}, "working_directory_uri": workspace.as_uri() + "/",
        "rows": 24, "cols": 80, "max_runtime_seconds": 120,
    }
    preview = request(base, "/v1/background-terminals/preview", token, {"scope": scope, "terminal": terminal})
    result = request(base, "/v1/background-terminals", token, {
        "scope": scope, "terminal": terminal,
        "confirmation": {"confirmed": True, "permissions_sha256": preview["permissions_sha256"]},
    })
    if result["status"] != "running":
        raise AssertionError(f"Background terminal failed: {result}")
    for name in ("terminal.pid", "child.pid"):
        path = workspace / name
        def read_pid():
            try:
                text = path.read_text().strip()
                return int(text) if text else None
            except FileNotFoundError:
                return None
        pid = wait_until(read_pid, name)
        child_pids.append(pid)
        if not running(pid):
            raise AssertionError(f"Background child {pid} was not alive before owner exit")


def check_lifetime(daemon, endpoint, directory, *, kill_parent):
    environment = daemon_environment(directory, endpoint)
    child_pids = []
    # The helper alone owns the daemon's stdin writer. Killing it reproduces an
    # app crash/force quit, without sending any signal to the daemon itself.
    command = [sys.executable, str(Path(__file__).resolve()), "--lifetime-parent", str(daemon)] if kill_parent else [str(daemon)]
    with (directory / "engine.log").open("w") as log:
        process = subprocess.Popen(command, cwd=directory / "Workspaces", env=environment,
                                   stdin=subprocess.PIPE, stdout=log, stderr=log, start_new_session=True)
        try:
            connection = wait_until(lambda: connection_ready(directory, environment["S_CODE_TOKEN"], process), "owned daemon readiness")
            daemon_pid = connection["pid"]
            start_terminal(connection, directory, child_pids)
            if kill_parent:
                process.kill()  # Only the parent PID; deliberately do not kill its group.
                process.wait(timeout=5)
                wait_until(lambda: not running(daemon_pid), "daemon exit after parent death", SHUTDOWN_TIMEOUT)
            else:
                process.stdin.close()
                code = process.wait(timeout=SHUTDOWN_TIMEOUT)
                if code:
                    raise AssertionError(f"Daemon stdin EOF shutdown failed: {code}")
            wait_until(lambda: all(not running(pid) for pid in child_pids), "terminal and child shutdown", SHUTDOWN_TIMEOUT)
            if (directory / "run/daemon.json").exists():
                raise AssertionError("Daemon left its connection record after shutdown")
            print(f"PASS: {'killed parent' if kill_parent else 'stdin EOF'} stops owned daemon, terminal and child", flush=True)
        except BaseException:
            print((directory / "engine.log").read_text()[-12000:], file=sys.stderr)
            raise
        finally:
            stop(process)
            # Fallback cleanup runs after assertions, so it cannot hide a
            # daemon/terminal leak. Terminals use independent process groups.
            for path in (directory / "Workspaces").glob("*.pid"):
                try:
                    child_pids.append(int(path.read_text().strip()))
                except (ValueError, OSError):
                    pass
            for pid in set(child_pids):
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


def lifetime_parent(daemon):
    process = subprocess.Popen([daemon], stdin=subprocess.PIPE)
    # Keep the writer alive in this process only. The orchestrator kills this
    # parent once the real daemon and terminal child have demonstrated readiness.
    raise SystemExit(process.wait(timeout=90))


def check_theme_previews(app):
    bundle = app / "Contents/Resources/SCodeDesktop_SCodeDesktop.bundle"
    for theme in ("light", "dark", "terminal", "midnight", "nord"):
        image = bundle / f"theme-{theme}.png"
        if not image.is_file() or not image.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"):
            raise AssertionError(f"Missing or invalid packaged theme preview: {image}")
        decoded = subprocess.run(["/usr/bin/sips", "-g", "pixelWidth", "-g", "pixelHeight", str(image)],
                                 capture_output=True, text=True, check=True, timeout=10)
        if "pixelWidth: 944" not in decoded.stdout or "pixelHeight: 608" not in decoded.stdout:
            raise AssertionError(f"Packaged theme preview did not decode at its expected size: {image}")
    print("PASS: packaged bundle contains all five decodable native theme screenshots", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--lifetime-parent", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.lifetime_parent:
        lifetime_parent(args.lifetime_parent)
    if sys.platform != "darwin":
        parser.error("native desktop checks require macOS")
    if not args.skip_build:
        run([str(ROOT / "scripts/build-macos-app.sh"), "--debug"], cwd=ROOT, timeout=1200)
    swift = os.environ.get("SWIFT", "swift")
    binary_directory = Path(subprocess.check_output(
        [swift, "build", "--package-path", str(ROOT / "clients/macos"), "--configuration", "debug", "--show-bin-path"],
        cwd=ROOT, text=True, timeout=30).strip())
    daemon = ROOT / ".work/macos-dist/S-Code.app/Contents/Helpers/s-code-daemon"
    check_theme_previews(ROOT / ".work/macos-dist/S-Code.app")
    checks = binary_directory / "DesktopChecks"
    for executable in (daemon, checks):
        if not os.access(executable, os.X_OK):
            raise AssertionError(f"Missing built executable: {executable}")
    # Do not inherit CI's shared tmpdir inside the checkout: private engine data
    # must never sit under a project the Work test is authorized to modify.
    with tempfile.TemporaryDirectory(prefix="s-code-desktop-", dir="/tmp") as temporary:
        directory = Path(temporary).resolve()
        with (directory / "fixture.log").open("w") as log:
            fixture = subprocess.Popen([sys.executable, str(ROOT / "tests/macos-model-fixture.py")],
                                       stdout=subprocess.PIPE, stderr=log, start_new_session=True)
            try:
                endpoint = fixture_endpoint(fixture)
                request(endpoint, "/models")
                run([str(checks), "--engine", str(daemon), endpoint, str(directory / "integration")],
                    cwd=directory, timeout=180)
                check_lifetime(daemon, endpoint, directory / "eof", kill_parent=False)
                check_lifetime(daemon, endpoint, directory / "parent-kill", kill_parent=True)
            except BaseException:
                print("Model fixture diagnostics:\n" + (directory / "fixture.log").read_text()[-12000:], file=sys.stderr)
                raise
            finally:
                stop(fixture)
                fixture.stdout.close()
    print("Native macOS packaging, desktop core, real-engine integration and lifetime checks passed")


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, ValueError, subprocess.SubprocessError) as error:
        raise SystemExit(f"macOS desktop checks failed: {error}")
