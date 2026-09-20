#!/usr/bin/env python3
"""Local-only onboarding regression: a real daemon, CLI PTY and mock provider.
Run after cargo build -p s-code-cli -p s-code-daemon. No real keys or model spend.
"""
import http.server
import json
import os
from pathlib import Path
import pty
import select
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve() / "debug"
KEY = "synthetic-onboarding-only"

class Provider(http.server.BaseHTTPRequestHandler):
    empty_catalog = False
    def log_message(self, *args):
        pass

    def do_GET(self):
        authorized = self.headers.get("Authorization") == "Bearer " + KEY
        body = {"data": [{"id": "coding-model"}]} if authorized else {"error": KEY}
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
        home = Path(temporary)
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
            cli_env = dict(env, S_CODE_HOME=str(cli_home))
            master, slave = pty.openpty()
            cli = subprocess.Popen([TARGET / "s-code-cli", "setup", "--provider", "openai-compatible", "--base-url", endpoint], env=cli_env, stdin=slave, stdout=slave, stderr=slave)
            os.close(slave)
            output = b""
            try:
                def expect(text):
                    nonlocal output
                    until = time.monotonic() + 25
                    start = len(output)
                    while text.encode() not in output[start:]:
                        if time.monotonic() > until:
                            raise AssertionError(f"Wizard did not reach {text!r}")
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
                os.write(master, b"r\n")
                expect("Filter models")
                os.write(master, b"*\n")
                expect("Model number")
                os.write(master, b"1\n")
                expect("You're connected")
                assert cli.wait(timeout=10) == 0
                assert KEY.encode() not in output, "CLI echoed the secret"
                assert json.loads((cli_home / "config.provider-credentials.json").read_text())["model"] == "coding-model"
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
    print("Provider onboarding: authenticated discovery, atomic save, redaction, reload and hidden CLI input passed")

if __name__ == "__main__":
    main()
