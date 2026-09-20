#!/usr/bin/env python3
"""Local-only onboarding regression: a real daemon, CLI PTY and mock provider.
Run after cargo build -p s-code-cli -p s-code-daemon. No real keys or model spend.
"""
import fcntl
import struct
import termios
import http.server
import json
import os
from pathlib import Path
import pty
import re
import select
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from cli_pty_driver import TerminalScreen

ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve() / "debug"
KEY = "synthetic-onboarding-only"

class PickerScreen(TerminalScreen):
    def apply_csi(self, command, parameters):
        # The selector enters a fresh alternate screen after line-based prompts.
        if command == "h" and parameters == "?1049":
            self.cells = [[" "] * self.cols for _ in range(self.rows)]
            self.row = self.col = 0
        super().apply_csi(command, parameters)


class Provider(http.server.BaseHTTPRequestHandler):
    empty_catalog = False
    large_catalog = False
    reply_allowed = threading.Event()
    finish_allowed = threading.Event()
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        self.rfile.read(length)
        if self.headers.get("Authorization") != "Bearer " + KEY:
            self.send_error(401)
            return
        if not Provider.reply_allowed.wait(timeout=30):
            self.send_error(504)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            frame = {"choices": [{"index": 0, "delta": {"content": "Onboarding reply arrived."}, "finish_reason": None}]}
            self.wfile.write(("data: " + json.dumps(frame) + "\n\n").encode())
            self.wfile.flush()
            if not Provider.finish_allowed.wait(timeout=30):
                return
            frame = {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}
            self.wfile.write(("data: " + json.dumps(frame) + "\n\ndata: [DONE]\n\n").encode())
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        authorized = self.headers.get("Authorization") == "Bearer " + KEY
        body = {"data": [{"id": "coding-model"}]} if authorized else {"error": KEY}
        if authorized and Provider.large_catalog:
            body = {"data": [{"id": f"model-{i:04}", "name": f"GLM Coding {i}"} for i in range(3000)]}
        if authorized and Provider.empty_catalog:
            body = {"data": []}
        self.send_response(200 if authorized else 401)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(body).encode())


def request(base, path, method="GET", data=None, token="local-test"):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    req = urllib.request.Request(base + path, method=method, headers=headers,
                                 data=None if data is None else json.dumps(data).encode())
    try:
        with urllib.request.urlopen(req, timeout=20) as result:
            return result.status, json.load(result)
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode()


def main():
    # Fresh environment-managed CLI installations must bypass the interactive wizard.
    for setting, value in [("S_CODE_MODEL_CREDENTIAL_HANDLE", "TEST_MODEL_KEY"),
                           ("S_CODE_MODEL_PROVIDER", "openai_compatible"),
                           ("S_CODE_MODEL_BASE_URL", "http://127.0.0.1:1/v1")]:
        with tempfile.TemporaryDirectory(prefix="s-code-managed-setup-") as directory:
            managed_env = {k: v for k, v in os.environ.items() if not k.startswith("S_CODE_")}
            managed_env.update(S_CODE_HOME=directory, TERM="xterm", NO_COLOR="1")
            managed_env[setting] = value
            master, slave = pty.openpty()
            cli = subprocess.Popen([TARGET / "s-code-cli"], env=managed_env, stdin=slave, stdout=slave, stderr=slave)
            os.close(slave)
            output = b""
            try:
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    if select.select([master], [], [], .1)[0]:
                        try:
                            chunk = os.read(master, 65536)
                        except OSError:
                            break
                        if not chunk:
                            break
                        output += chunk
                    elif cli.poll() is not None:
                        break
                assert cli.wait(timeout=2) == 1
                assert b"local service was not discovered" in output, output.decode(errors="replace")
                assert b"Provider settings are managed" not in output
                assert b"API key (hidden)" not in output
            finally:
                if cli.poll() is None:
                    cli.terminate()
                    cli.wait(timeout=5)
                os.close(master)
    provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    with tempfile.TemporaryDirectory(prefix="s-code-onboarding-") as temporary:
        installation = Path(temporary)
        home = installation / ".test-home"
        home.mkdir()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        env = {k: v for k, v in os.environ.items() if not k.startswith("S_CODE_")}
        env.update(S_CODE_HOME=str(home), S_CODE_DAEMON_LISTEN=f"127.0.0.1:{port}",
                   S_CODE_DATABASE_URL="sqlite::memory:", S_CODE_TOKEN="local-test", NO_COLOR="1")
        endpoint = f"http://127.0.0.1:{provider.server_port}/v1"
        base = f"http://127.0.0.1:{port}"
        log = open(home / "daemon.log", "w+")
        daemon = subprocess.Popen([TARGET / "s-code-daemon"], env=env, stdout=log, stderr=log)
        try:
            for _ in range(100):
                try:
                    if request(base, "/v1/provider-setup")[0] == 200:
                        break
                except OSError:
                    time.sleep(.1)
            else:
                raise AssertionError("Test daemon did not start")
            assert request(base, "/v1/provider-setup", token=None)[0] == 401
            assert request(base, "/v1/provider-setup")[1]["needs_setup"] is True
            connection = {"preset": "openai-compatible", "base_url": endpoint, "api_key": "wrong"}
            status, body = request(base, "/v1/provider-setup/models", "POST", connection)
            assert status == 400 and KEY not in body
            connection["api_key"] = KEY
            assert request(base, "/v1/provider-setup/models", "POST", connection)[1]["models"][0]["id"] == "coding-model"
            status, body = request(base, "/v1/provider-setup", "PUT", {"connection": connection, "model": "coding-model"})
            assert status == 200 and KEY not in json.dumps(body)
            saved = home / "config.provider-credentials.json"
            assert json.loads(saved.read_text())["api_key"] == KEY
            assert saved.stat().st_mode & 0o777 == 0o600
            assert request(base, "/v1/provider-setup")[1]["configured"] is True
            assert request(base, "/v1/provider-setup")[1]["needs_setup"] is False
            assert request(base, "/v1/settings")[1]["default_model"] == "coding-model"
            effective = subprocess.check_output([TARGET / "s-code-daemon", "--config-print-effective"], env=env, text=True)
            assert KEY not in effective and "coding-model" in effective
            # Replacing the key with a rejected key must preserve the working connection.
            before = saved.read_bytes()
            connection["api_key"] = "wrong"
            assert request(base, "/v1/provider-setup", "PUT", {"connection": connection, "model": "coding-model"})[0] == 400
            assert saved.read_bytes() == before
            # Successful empty catalogs must not overwrite saved credentials.
            Provider.empty_catalog = True
            connection["api_key"] = KEY
            status, body = request(base, "/v1/provider-setup/models", "POST", connection)
            assert status == 400 and "empty model list" in body and KEY not in body
            assert request(base, "/v1/provider-setup", "PUT", {"connection": connection, "model": "coding-model"})[0] == 400
            assert saved.read_bytes() == before
            # CLI wizard gets a separate private installation, but the same mock provider.
            cli_home = home / "cli"
            cli_home.mkdir()
            cli_env = dict(env, S_CODE_HOME=str(cli_home), TERM="xterm")
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
            original_terminal = termios.tcgetattr(slave)
            cli = subprocess.Popen([TARGET / "s-code-cli", "setup", "--provider", "openai-compatible", "--base-url", endpoint], env=cli_env, stdin=slave, stdout=slave, stderr=slave)
            os.close(slave)
            output = b""
            screen = PickerScreen(24, 100)
            try:
                def expect(text, visible=False):
                    nonlocal output
                    until = time.monotonic() + 25
                    start = len(output)
                    def plain(data):
                        return re.sub(rb"\s+", b" ", re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b" ", data))
                    while True:
                        screen.feed_new(output)
                        if (text in screen.text()) if visible else (text.encode() in plain(output[start:])):
                            return
                        if time.monotonic() > until:
                            raise AssertionError(f"Wizard did not reach {text!r}: {screen.text()!r}")
                        if select.select([master], [], [], .2)[0]:
                            output += os.read(master, 65536)
                expect("Does this endpoint need a key?")
                os.write(master, b"y\n")
                expect("API key (hidden)")
                os.write(master, KEY.encode() + b"\r")
                expect("[r] Retry, [k] change key, [q] cancel")
                assert b"empty model list" in output
                assert b"Try another key" not in output
                assert not (cli_home / "config.provider-credentials.json").exists()
                Provider.empty_catalog = False
                Provider.large_catalog = True
                os.write(master, b"r\n")
                expect("Esc cancel", visible=True)
                assert "Search models" in screen.text()
                # Initial suggestions and controls appear without submitting a filter.
                assert b"model-0000" in output
                assert b"PgUp/PgDn" in output
                os.write(master, b"\x1b[6~")
                expect("> model-0010", visible=True)
                os.write(master, b"zzzz")
                expect("No matches", visible=True)
                os.write(master, b"\r")
                time.sleep(.1)
                assert cli.poll() is None
                assert not (cli_home / "config.provider-credentials.json").exists()
                # Ctrl+U clears, then typing updates results without pressing Enter.
                os.write(master, b"\x15gLm 2999")
                expect("model-2999", visible=True)
                os.write(master, b"\r")
                expect("You're connected")
                assert cli.wait(timeout=10) == 0
                assert KEY.encode() not in output, "CLI echoed the secret"
                assert json.loads((cli_home / "config.provider-credentials.json").read_text())["model"] == "model-2999"
                restored = termios.tcgetattr(master)
                assert restored[3] & (termios.ECHO | termios.ICANON) == original_terminal[3] & (termios.ECHO | termios.ICANON)
            finally:
                if cli.poll() is None:
                    cli.terminate()
                    cli.wait(timeout=10)
                os.close(master)
            # Provider cancellation must leave no credentials and restore the terminal.
            for cancel_key in (b"\x1b", b"\x03"):
                cancel_home = home / ("provider-escape" if cancel_key == b"\x1b" else "provider-interrupt")
                cancel_env = dict(env, S_CODE_HOME=str(cancel_home), TERM="xterm")
                master, slave = pty.openpty()
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
                original_terminal = termios.tcgetattr(slave)
                cli = subprocess.Popen([TARGET / "s-code-cli", "setup"], env=cancel_env, stdin=slave, stdout=slave, stderr=slave)
                os.close(slave)
                output = b""
                screen = PickerScreen(24, 100)
                try:
                    expect("> 1  SAI", visible=True)
                    expect("Enter select", visible=True)
                    os.write(master, cancel_key)
                    expect("Setup cancelled")
                    assert cli.wait(timeout=10) == 0
                    assert not (cancel_home / "config.provider-credentials.json").exists()
                    restored = termios.tcgetattr(master)
                    assert restored[3] & (termios.ECHO | termios.ICANON) == original_terminal[3] & (termios.ECHO | termios.ICANON)
                    assert not re.search(rb"\x1b\[(?:38;|3[0-7]m)", output), "NO_COLOR must disable colors"
                finally:
                    if cli.poll() is None:
                        cli.terminate()
                        cli.wait(timeout=10)
                    os.close(master)
            # Cancellation restores terminal modes; dumb terminals get a paged fallback.
            for plain in (False, True):
                extra_home = home / ("cli-plain" if plain else "cli-cancel")
                extra_home.mkdir()
                extra_env = dict(env, S_CODE_HOME=str(extra_home), TERM="dumb" if plain else "xterm")
                if not plain:
                    extra_env.pop("NO_COLOR", None)
                    extra_env.pop("CLICOLOR", None)
                master, slave = pty.openpty()
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 12, 60, 0, 0))
                original_terminal = termios.tcgetattr(slave)
                cli = subprocess.Popen([TARGET / "s-code-cli", "setup", "--base-url", endpoint], env=extra_env, stdin=slave, stdout=slave, stderr=slave)
                os.close(slave)
                output = b""
                screen = PickerScreen(12, 60)
                try:
                    if plain:
                        expect("Provider (number or ID, q cancel)")
                        os.write(master, b"99\n")
                        expect("Choose a listed provider number or ID")
                        os.write(master, b"8\n")
                    else:
                        # Small terminals scroll to keep the highlighted provider visible.
                        expect("> 1  SAI", visible=True)
                        os.write(master, b"\x1b[B")
                        expect("> 2  OpenAI", visible=True)
                        os.write(master, b"\x1b[A")
                        expect("> 1  SAI", visible=True)
                        os.write(master, b"\x1b[F")
                        expect("> 8  Custom", visible=True)
                        os.write(master, b"\x1b[H")
                        expect("> 1  SAI", visible=True)
                        os.write(master, b"7")
                        expect("> 7  Local", visible=True)
                        os.write(master, b"\x1b[B")
                        expect("> 8  Custom", visible=True)
                        os.write(master, b"\r")
                    expect("Does this endpoint need a key?")
                    assert b"Connect Custom" in output
                    os.write(master, b"y\n")
                    expect("API key (hidden)")
                    os.write(master, KEY.encode() + b"\r")
                    if plain:
                        expect("q cancel")
                        assert b"model-0000" in output
                        os.write(master, b"n\n")
                        expect("model-0010")
                        os.write(master, b"/2999\n")
                        expect("model-2999")
                        os.write(master, b"1\n")
                        expect("You're connected")
                        assert json.loads((extra_home / "config.provider-credentials.json").read_text())["model"] == "model-2999"
                    else:
                        expect("Search models", visible=True)
                        os.write(master, b"\x1b")
                        expect("Setup cancelled")
                        assert not (extra_home / "config.provider-credentials.json").exists()
                    assert cli.wait(timeout=10) == 0
                    assert KEY.encode() not in output
                    if not plain:
                        assert "✨".encode() in output and "🧭".encode() in output
                        assert re.search(rb"\x1b\[[0-9;]*m", output)
                    else:
                        assert not re.search(rb"\x1b\[[0-9;]*m", output)
                    restored = termios.tcgetattr(master)
                    assert restored[3] & (termios.ECHO | termios.ICANON) == original_terminal[3] & (termios.ECHO | termios.ICANON)
                finally:
                    if cli.poll() is None:
                        cli.terminate()
                        cli.wait(timeout=10)
                    os.close(master)
            # Launch after setup from the installation containing private data.
            # Use the real daemon discovery contract, not an explicit remote token.
            launch_env = dict(env, TERM="xterm")
            launch_env.pop("S_CODE_TOKEN", None)
            scope = {"organization_id": "org_local", "team_id": "team_local", "actor_id": "user_local"}
            status, _ = request(base, "/v1/sessions", "POST", {"scope": scope, "workspace_uri": installation.as_uri(), "title": "Unsafe", "model": "coding-model", "mode": "work"})
            assert status == 400, "Daemon must continue rejecting private-directory overlap"
            saved_before_launch = saved.read_bytes()
            noninteractive = subprocess.run([TARGET / "s-code-cli", "--print", "Do not send this"], cwd=installation, env=launch_env, input="", capture_output=True, text=True, timeout=15)
            assert noninteractive.returncode != 0
            assert "Run S-Code from a separate project folder" in noninteractive.stderr
            assert not (installation / "s-code-workspace").exists()
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
            cli = subprocess.Popen([TARGET / "s-code-cli"], cwd=installation, env=launch_env, stdin=slave, stdout=slave, stderr=slave)
            os.close(slave)
            output = b""
            screen = PickerScreen(24, 100)
            try:
                expect("Work folder (absolute path")
                os.write(master, (str(home / "forbidden") + "\n").encode())
                expect("Choose a folder outside")
                assert not (home / "forbidden").exists()
                os.write(master, b"\n")
                expect("Work folder:")
                session_path = "/v1/sessions?" + urllib.parse.urlencode(scope)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    status, sessions = request(base, session_path)
                    if status == 200 and sessions:
                        break
                    if select.select([master], [], [], .1)[0]:
                        output += os.read(master, 65536)
                assert status == 200 and len(sessions) == 1, sessions
                assert sessions[0]["workspace_uri"].rstrip("/") == (installation / "s-code-workspace").resolve().as_uri()
                assert saved.read_bytes() == saved_before_launch
                expect("Message S-Code", visible=True)
                os.write(master, b"hello\r")
                expect("Waiting for model response", visible=True)
                expect("1s", visible=True)
                assert "Esc cancel" in screen.text()
                Provider.reply_allowed.set()
                expect("Onboarding reply arrived.", visible=True)
                expect("Receiving response", visible=True)
                Provider.finish_allowed.set()
                expect("completed", visible=True)
                assert KEY.encode() not in output
            finally:
                if cli.poll() is None:
                    cli.terminate()
                    cli.wait(timeout=10)
                os.close(master)
            log.flush()
            log.seek(0)
            assert KEY not in log.read(), "Daemon logged the provider key"
        finally:
            daemon.terminate()
            daemon.wait(timeout=15)
            log.close()
    provider.shutdown()
    provider.server_close()
    print("Provider onboarding: discovery, save, redaction, keyboard selection and first streamed CLI reply passed")

if __name__ == "__main__":
    main()
