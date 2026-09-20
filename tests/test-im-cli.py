#!/usr/bin/env python3
"""Exercise the compiled IM CLI against an isolated HTTP daemon contract.

No Telegram account, bot token, daemon process, or network outside loopback is
needed. The PTY checks exercise the real hidden-input code and terminal restore.
"""
import fcntl
import http.server
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
from urllib.parse import parse_qs, urlsplit


SCOPE = {
    "organization_id": "org-im-cli",
    "team_id": "team-im-cli",
    "actor_id": "actor-im-cli",
    "goal_id": None,
    "task_id": None,
}
BOT_TOKEN = "123456:TEST_ONLY_never_a_real_bot_token"
DAEMON_TOKEN = "im-cli-local-auth-fixture"
CAPABILITIES = {
    "protocol_version": "1.0",
    "server_version": "test",
    "capabilities": [
        {"id": name, "version": "1.0", "maturity": "stable", "enabled": True}
        for name in ("scope.team", "session.persistence", "event.sse_replay", "im.telegram.v1")
    ],
}


class Daemon(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), Handler)
        self.requests = []
        self.response_status = 200
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    def finish(self):
        self.shutdown()
        self.server_close()
        self.thread.join(timeout=5)


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.respond()

    def do_POST(self):
        self.respond()

    def respond(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        body = json.loads(raw) if raw else None
        self.server.requests.append((self.command, self.path, dict(self.headers), body))
        path = urlsplit(self.path).path
        if self.headers.get("Authorization") != f"Bearer {DAEMON_TOKEN}":
            status, result = 401, {"error": "unauthorized"}
        elif self.headers.get("x-s-code-csrf") != "1":
            status, result = 403, {"error": "missing CSRF header"}
        elif path == "/v1/capabilities":
            status, result = 200, CAPABILITIES
        elif self.server.response_status != 200:
            status, result = self.server.response_status, {"error": "fixture request rejected"}
        elif path == "/v1/sessions" and self.command == "GET":
            status, result = 200, [{
                "id": "session-cli-fixture", "scope": SCOPE,
                "workspace_uri": "file:///fixture-workspace", "title": "Phone coding",
                "model": "fixture", "status": "active",
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            }]
        elif path == "/v1/im/telegram":
            result = {
                "channel": "telegram", "configured": True,
                "bot_username": "fixture_bot", "paired_user": 42,
                "pending_user": 42, "pending_identity": {"user_id": 42, "first_name": "Alice", "username": "alice"}, "allowed_sessions": ["session-cli-fixture"],
                "selected_session": "session-cli-fixture", "active_turn": None,
            }
            if body and body["action"] in ("connect", "pair"):
                result["pairing_link"] = "https://t.me/fixture_bot?start=fixture-pair-code"
            status = 200
        else:
            status, result = 404, {"error": "unexpected route"}
        payload = json.dumps(result).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def run(binary, environment, directory, *args, ok=True):
    process = subprocess.run(
        [binary, "im", *args], cwd=directory, env=environment,
        input="", capture_output=True, text=True, timeout=15,
    )
    assert (process.returncode == 0) == ok, (args, process.returncode, process.stdout, process.stderr)
    assert BOT_TOKEN not in process.stdout + process.stderr, "bot credential leaked to terminal"
    return process.stdout + process.stderr


def management(daemon):
    return [request for request in daemon.requests if urlsplit(request[1]).path != "/v1/capabilities"]


def check_scope_query(path):
    assert parse_qs(urlsplit(path).query) == {
        key: [value] for key, value in SCOPE.items() if value is not None
    }


def hidden_input(binary, environment, directory, daemon, cancel=False):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    original = termios.tcgetattr(slave)
    process = subprocess.Popen(
        [binary, "im", "telegram", "connect"], cwd=directory, env=environment,
        stdin=slave, stdout=slave, stderr=slave, close_fds=True,
    )
    output = bytearray()
    sent = False
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                output.extend(os.read(master, 65536))
            # The prompt is flushed before raw mode. Wait for disabled echo,
            # rather than relying on scheduling luck or leaking fixture input.
            if not sent and b"Bot token (hidden):" in output:
                if not termios.tcgetattr(slave)[3] & termios.ECHO:
                    if cancel:
                        os.write(master, b"\x03")
                    else:
                        # Backspace must replace the mistyped final character.
                        os.write(master, BOT_TOKEN[:-1].encode() + b"X\x7f" + BOT_TOKEN[-1:].encode() + b"\r")
                    sent = True
            if process.poll() is not None:
                while select.select([master], [], [], 0)[0]:
                    output.extend(os.read(master, 65536))
                break
        else:
            raise AssertionError("hidden token prompt timed out")
        assert sent, "hidden token prompt never disabled terminal echo"
        assert (process.returncode == 0) != cancel, output.decode(errors="replace")
        assert BOT_TOKEN.encode() not in output, "hidden token appeared in PTY output"
        assert b"TEST_ONLY_never" not in output, "partial hidden token echoed"
        restored = termios.tcgetattr(slave)
        assert restored[3] & (termios.ECHO | termios.ICANON) == original[3] & (termios.ECHO | termios.ICANON), "terminal mode not restored"
        requests = management(daemon)
        if cancel:
            assert not requests, "cancelled prompt sent a management request"
        else:
            assert len(requests) == 1
            assert requests[0][3] == {"scope": SCOPE, "action": "connect", "token": BOT_TOKEN}
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)
        os.close(master)
        os.close(slave)


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: test-im-cli.py COMPILED_CLI")
    binary = str(Path(sys.argv[1]).resolve(strict=True))
    daemon = Daemon()
    try:
        with tempfile.TemporaryDirectory(prefix="s-code-im-cli-") as directory:
            environment = {key: value for key, value in os.environ.items() if not key.startswith("S_CODE_")}
            environment.update({
                "HOME": directory, "S_CODE_HOME": directory,
                "S_CODE_URL": f"http://127.0.0.1:{daemon.server_port}",
                "S_CODE_TOKEN": DAEMON_TOKEN, "S_CODE_ORGANIZATION": SCOPE["organization_id"],
                "S_CODE_TEAM": SCOPE["team_id"], "S_CODE_ACTOR": SCOPE["actor_id"],
                "S_CODE_WORKSPACE": Path(directory).as_uri(), "TERM": "xterm-256color",
                "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
                "IM_CLI_FIXTURE_TOKEN": BOT_TOKEN,
            })
            for command, expected_text, expected_route in [
                ("status", '"paired_user": 42', "/v1/im/telegram"),
                ("sessions", "session-cli-fixture  Phone coding", "/v1/sessions"),
            ]:
                daemon.requests.clear()
                output = run(binary, environment, directory, "telegram", command)
                assert expected_text in output
                if command == "status":
                    assert '"first_name": "Alice"' in output and '"username": "alice"' in output
                    assert "Compare pending_identity.user_id" in output and "Names are display-only" in output
                requests = management(daemon)
                assert len(requests) == 1 and requests[0][0] == "GET"
                assert urlsplit(requests[0][1]).path == expected_route
                check_scope_query(requests[0][1])

            for args, fields in [
                (("connect", "--credential-handle", "IM_CLI_FIXTURE_TOKEN"), {"token": BOT_TOKEN}),
                (("pair",), {}), (("approve", "42"), {"user_id": 42}),
                (("allow", "session-cli-fixture"), {"session_id": "session-cli-fixture"}),
                (("disallow", "session-cli-fixture"), {"session_id": "session-cli-fixture"}),
                (("revoke",), {}), (("disconnect",), {}),
            ]:
                daemon.requests.clear()
                output = run(binary, environment, directory, "telegram", *args)
                requests = management(daemon)
                assert len(requests) == 1
                method, path, headers, body = requests[0]
                assert method == "POST" and path == "/v1/im/telegram"
                assert body == {"scope": SCOPE, "action": args[0], **fields}
                assert BOT_TOKEN not in path + json.dumps(headers)
                if args[0] in ("connect", "pair"):
                    assert "https://t.me/fixture_bot?start=fixture-pair-code" in output
                if args[0] in ("revoke", "disconnect", "disallow"):
                    assert "Already started tasks remain visible" in output

            for args, expected in [
                (("other-channel", "status"), "Usage:"),
                (("telegram", "unknown"), "Usage:"),
                (("telegram", "approve"), "Usage:"),
                (("telegram", "approve", "not-a-number"), "must be numeric"),
                (("telegram", "allow"), "Usage:"),
                (("telegram", "status", "extra"), "Usage:"),
                (("telegram", "status", "--credential-handle", "IM_CLI_FIXTURE_TOKEN"), "only used with connect"),
                (("telegram", "connect", "--credential-handle", "IM_CLI_ABSENT_TOKEN"), "not set"),
                (("telegram", "connect"), "non-interactive setup"),
            ]:
                daemon.requests.clear()
                environment.pop("IM_CLI_ABSENT_TOKEN", None)
                output = run(binary, environment, directory, *args, ok=False)
                assert expected in output, (args, output)
                assert not management(daemon), "invalid command reached a management route"

            daemon.response_status = 403
            output = run(binary, environment, directory, "telegram", "revoke", ok=False)
            assert "403" in output and "fixture request rejected" in output
            daemon.response_status = 200
            for cancel in (False, True):
                daemon.requests.clear()
                hidden_input(binary, environment, directory, daemon, cancel=cancel)
    finally:
        daemon.finish()
    print("IM CLI HTTP contracts, scope/auth, management commands, errors, hidden token and terminal restoration passed")


if __name__ == "__main__":
    main()
