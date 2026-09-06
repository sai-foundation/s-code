#!/usr/bin/env python3
"""Real S-Code daemon experiment; isolated state, metered provider, external grading.

No model credential is given to the coding agent or persisted in artifacts.
Artifacts contain synthetic task prompts/patches and are private by default.
"""
from __future__ import annotations

import argparse
import http.client
import hashlib
import http.server
import json
import math
import os
from pathlib import Path
import secrets
import re
import socket
import ssl
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SCOPE = {"organization_id": "bench", "team_id": "bench", "actor_id": "bench"}
QUERY = urllib.parse.urlencode(SCOPE)


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def tree_hash(root):
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if path.is_file() and not any(p in {".git", "__pycache__"} for p in path.relative_to(root).parts):
            digest.update(str(path.relative_to(root)).encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def snapshot_source(output):
    # The Git index defines publishable source. Read its working contents so a
    # modified/staged file changes the digest, but never copy unrelated local
    # documents or ignored experiment artifacts into a source snapshot.
    names = subprocess.check_output(["git", "-C", str(ROOT), "ls-files", "--cached", "-z"]).decode().split("\0")
    records = {}
    sources = []
    for name in sorted(set(names) - {""}):
        relative = Path(name)
        source = ROOT / relative
        if relative.is_absolute() or ".." in relative.parts or any(
            ROOT.joinpath(*relative.parts[0:index]).is_symlink()
            for index in range(1, len(relative.parts) + 1)
        ):
            raise RuntimeError(f"Tracked source must not traverse a symlink: {name}")
        if not source.is_file():
            raise RuntimeError(f"Tracked source is missing or is not a regular file: {name}")
        if source.stat().st_size > 5_000_000:
            raise RuntimeError(f"Tracked source exceeds the 5 MB snapshot limit: {name}")
        data = source.read_bytes()
        records[name] = hashlib.sha256(data).hexdigest()
        sources.append((name, data))
    # Validate every entry before writing any part of the snapshot.
    for name, data in sources:
        destination = output / "source" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
    write_json(output / "source.json", records)
    return hashlib.sha256(json.dumps(records, sort_keys=True).encode()).hexdigest()


def reserved_or_charged(record):
    cost = record.get("cost")
    return cost if type(cost) in (int,float) and math.isfinite(cost) and cost >= 0 else record["reserved_cost"]


def provider_totals(records):
    def valid(record):
        usage = record.get("usage")
        return isinstance(usage, dict) and all(type(usage.get(key)) is int and usage[key] >= 0 for key in ("prompt_tokens", "completion_tokens")) and not record.get("error") and not record.get("invalid_event")
    if not records or not all(valid(record) for record in records): return None
    costs = [r.get("cost") for r in records]
    complete_cost = all(type(cost) in (int,float) and math.isfinite(cost) and cost >= 0 for cost in costs)
    return {"input_tokens":sum(r["usage"]["prompt_tokens"] for r in records), "output_tokens":sum(r["usage"]["completion_tokens"] for r in records), "total_tokens":sum(r["usage"]["prompt_tokens"] + r["usage"]["completion_tokens"] for r in records), "cost_usd":sum(costs) if complete_cost else None, "model_calls":len(records)}


# Additive private diagnostics. These never certify usage, replace an error, or
# participate in reservation/denominator decisions. Missing on legacy records
# means diagnostics unavailable, not that a transport event did not occur.
DIAGNOSTIC_COUNTER_MAX = (1 << 63) - 1


def transport_diagnostics():
    # Byte/event counters cover complete lines returned by the response iterator;
    # an HTTP read failure can consume additional bytes inside that library.
    # IDs stay private: public reporting must use only presence/counts/digests.
    return {"schema_version": 1, "stage": "reserved", "failure_stage": None,
            "headers_received": False, "http_status": None,
            "body_received": False, "received_bytes": 0, "sse_data_lines": 0,
            "parsed_events": 0, "counters_saturated": False,
            "usage_event_received": False, "done_received": False,
            "upstream_eof": False, "client_disconnected": False,
            "error_category": None, "reason_category": None,
            "error_errno": None, "reason_errno": None, "tls_verify_code": None,
            "header_generation_id": None, "header_generation_id_state": "missing", "header_generation_id_count": 0,
            "sse_generation_id": None, "sse_generation_id_state": "missing", "sse_generation_id_count": 0,
            "generation_id_conflict": False}


def diagnostic_increment(diagnostic, field, amount=1):
    value = diagnostic[field] + amount
    diagnostic[field] = min(value, DIAGNOSTIC_COUNTER_MAX)
    diagnostic["counters_saturated"] |= value > DIAGNOSTIC_COUNTER_MAX


def diagnostic_generation_ids(diagnostic, source, values, known_secrets=()):
    """Keep only bounded OpenRouter IDs; never arbitrary header/event strings."""
    field, state = source + "_generation_id", source + "_generation_id_state"
    for value in values:
        diagnostic_increment(diagnostic, source + "_generation_id_count")
        valid = (isinstance(value, str) and re.fullmatch(r"gen-[A-Za-z0-9_-]{1,200}", value) is not None
                 and not any(secret and secret in value for secret in known_secrets))
        if not valid:
            if diagnostic[state] != "conflict": diagnostic[state] = "invalid"
            diagnostic[field] = None
        elif diagnostic[state] == "missing":
            diagnostic[field], diagnostic[state] = value, "valid"
        elif diagnostic[state] == "valid" and diagnostic[field] != value:
            diagnostic[field], diagnostic[state] = None, "conflict"
            diagnostic["generation_id_conflict"] = True
        # Invalid/conflicting values never become trusted after another value.
    header, sse = diagnostic["header_generation_id"], diagnostic["sse_generation_id"]
    if header is not None and sse is not None and header != sse:
        diagnostic["generation_id_conflict"] = True


def diagnostic_headers(diagnostic, status, headers, known_secrets=()):
    diagnostic["headers_received"] = True
    diagnostic["http_status"] = status if type(status) is int and 100 <= status <= 599 else None
    values = headers.get_all("X-Generation-Id", []) if headers is not None else []
    diagnostic_generation_ids(diagnostic, "header", values, known_secrets)


def diagnostic_error_category(error):
    # Do not serialize exception messages, arbitrary class names, URLs or hosts.
    if isinstance(error, urllib.error.HTTPError): return "http_error"
    if isinstance(error, urllib.error.URLError): return "url_error"
    if isinstance(error, socket.gaierror): return "name_resolution"
    if isinstance(error, ssl.SSLCertVerificationError): return "tls_verification"
    if isinstance(error, ssl.SSLError): return "tls"
    if isinstance(error, TimeoutError): return "timeout"
    if isinstance(error, ConnectionRefusedError): return "connection_refused"
    if isinstance(error, ConnectionResetError): return "connection_reset"
    if isinstance(error, BrokenPipeError): return "broken_pipe"
    if isinstance(error, http.client.HTTPException): return "http_protocol"
    if isinstance(error, OSError): return "os_error"
    return "unknown"


def diagnostic_error(diagnostic, error):
    if diagnostic["error_category"] is not None:
        return  # Keep the first diagnostic cause; preserve legacy error separately.
    diagnostic["failure_stage"] = diagnostic["stage"]
    diagnostic["error_category"] = diagnostic_error_category(error)
    def numeric_code(value):
        return value if type(value) is int and -(1 << 31) <= value < (1 << 31) else None
    diagnostic["error_errno"] = numeric_code(getattr(error, "errno", None))
    reason = error
    # A fixed traversal also terminates cyclic/custom URLError.reason chains.
    for _ in range(4):
        if not isinstance(reason, urllib.error.URLError) or isinstance(reason, urllib.error.HTTPError): break
        reason = reason.reason
    if isinstance(error, urllib.error.URLError) and not isinstance(error, urllib.error.HTTPError):
        diagnostic["reason_category"] = diagnostic_error_category(reason)
        diagnostic["reason_errno"] = numeric_code(getattr(reason, "errno", None))
    diagnostic["tls_verify_code"] = numeric_code(getattr(reason, "verify_code", None))


def observe_event(record, event, known_secrets=()):
    """Keep provider metadata without treating a partial/error stream as complete."""
    if not isinstance(event, dict):
        record["invalid_event"] = True
        return
    diagnostic = record.get("transport")
    if diagnostic is not None:
        if "id" in event:
            diagnostic_generation_ids(diagnostic, "sse", [event["id"]], known_secrets)
        if event.get("usage"):
            diagnostic["usage_event_received"] = True
    if event.get("id"): record["generation_id"] = event["id"]
    if event.get("provider"): record["provider"] = event["provider"]
    if event.get("usage"):
        record["usage"] = event["usage"]
        if isinstance(event["usage"], dict): record["cost"] = event["usage"].get("cost")
        else: record["invalid_event"] = True
    choices = event.get("choices") or []
    if not isinstance(choices, list) or any(not isinstance(choice, dict) for choice in choices):
        record["invalid_event"] = True
        return
    if event.get("error") is not None or any(choice.get("finish_reason") == "error" for choice in choices):
        record["error"] = "ProviderStreamError"
        if diagnostic is not None and diagnostic["error_category"] is None:
            diagnostic["error_category"] = "provider_stream_error"
            diagnostic["failure_stage"] = diagnostic["stage"]


def copy_tree(source, destination):
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(source, destination, ignore=shutil.ignore_patterns(".git", "__pycache__", "*.pyc"))
    subprocess.run(["git", "init", "-q", str(destination)], check=True)
    subprocess.run(["git", "-C", str(destination), "add", "."], check=True)
    subprocess.run(["git", "-C", str(destination), "-c", "user.name=Benchmark", "-c", "user.email=benchmark@example.invalid", "commit", "-qm", "Fixture"], check=True)


class Meter:
    def __init__(self, key, output, model, max_cost, max_calls, provider="z-ai/fp8"):
        if not math.isfinite(max_cost) or max_cost <= 0 or type(max_calls) is not int or max_calls <= 0:
            raise ValueError("Experiment budgets must be finite and positive")
        self.key, self.output, self.model = key, output, model
        self.binary = output / "s-code-daemon"
        shutil.copy2(ROOT / "target/debug/s-code-daemon", self.binary)
        self.max_cost, self.max_calls = max_cost, max_calls
        self.provider = provider
        self.lock = threading.Lock()
        self.denials = []
        self.records, self.active = [], 0
        self.context, self.raw = {}, None
        self.phase_max_cost = None
        self.phase_start = 0
        self.proxy_token = secrets.token_urlsafe(32)
        # Freeze live provider prices with the experiment; this is reservation only.
        with urllib.request.urlopen("https://openrouter.ai/api/v1/models", timeout=30) as response:
            models = json.load(response)["data"]
        self.model_info = next(item for item in models if item["id"] == model)
        write_json(output / "model.json", self.model_info)
        self.prices = {name: float(self.model_info["pricing"][name]) for name in ("prompt", "completion")}

    def reserve(self, body):
        # UTF-8 byte count conservatively bounds input tokens; all tools included.
        estimate = len(json.dumps(body).encode()) * self.prices["prompt"] + body.get("max_tokens", 8192) * self.prices["completion"]
        with self.lock:
            committed = sum(reserved_or_charged(record) for record in self.records)
            phase_committed = sum(reserved_or_charged(record) for record in self.records[self.phase_start:])
            if len(self.records) >= self.max_calls or committed + estimate > self.max_cost or (self.phase_max_cost is not None and phase_committed + estimate > self.phase_max_cost):
                self.denials.append({**self.context, "time":time.time(), "reason":"budget", "requested_reservation":estimate})
                write_json(self.output / "budget-denials.json", self.denials)
                return None
            record = {"index":len(self.records), **self.context, "reserved_cost":estimate, "started_at":time.time(), "usage":None, "cost":None, "transport":transport_diagnostics()}
            self.records.append(record)
            self.active += 1
            return record

    def handler(self):
        meter = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                if self.headers.get("Authorization") != "Bearer " + meter.proxy_token or self.path != "/v1/chat/completions":
                    self.send_error(403); return
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                if body.get("model") != meter.model:
                    self.send_error(400); return
                body["stream_options"] = {"include_usage": True}
                if "reasoning_effort" in body:
                    body["reasoning"] = {"effort":body.pop("reasoning_effort")}
                body["seed"] = int(meter.context.get("seed", 17))
                body["provider"] = {"only":[meter.provider], "order":[meter.provider], "allow_fallbacks": False}
                raw = meter.raw() if callable(meter.raw) and body.get("tools") else meter.raw
                if raw is not None and body.get("tools"):
                    if len(raw) > 6_000: raise RuntimeError("raw control exceeded its envelope")
                    index = next((i for i,m in enumerate(body["messages"]) if m["role"] != "system"), len(body["messages"]))
                    body["messages"].insert(index, {"role":"user", "content":raw})
                record = meter.reserve(body)
                if record is None:
                    self.send_error(429, "Experiment budget exhausted"); return
                directory = meter.output / "requests" / f'{record["index"]:04d}'
                write_json(directory / "request.json", body)
                upstream = urllib.request.Request("https://openrouter.ai/api/v1/chat/completions", data=json.dumps(body).encode(), headers={"Authorization":"Bearer " + meter.key, "Content-Type":"application/json", "HTTP-Referer":"https://github.com/sl-7qx/s-code", "X-Title":"S-Code self-evolving evaluation"})
                client_open, chunks = True, []
                diagnostic = record["transport"]
                known_secrets = (meter.key, meter.proxy_token)
                try:
                    diagnostic["stage"] = "upstream_open"
                    with urllib.request.urlopen(upstream, timeout=180) as response:
                        diagnostic_headers(diagnostic, response.status, response.headers, known_secrets)
                        diagnostic["stage"] = "downstream_headers"
                        self.send_response(response.status)
                        self.send_header("Content-Type", "text/event-stream")
                        self.end_headers()
                        diagnostic["stage"] = "upstream_body"
                        for line in response:
                            chunks.append(line)
                            diagnostic["body_received"] = True
                            diagnostic_increment(diagnostic, "received_bytes", len(line))
                            if line.strip() == b"data: [DONE]":
                                diagnostic["done_received"] = True
                            if line.startswith(b"data: ") and line.strip() != b"data: [DONE]":
                                diagnostic_increment(diagnostic, "sse_data_lines")
                                try:
                                    event = json.loads(line[6:])
                                    diagnostic_increment(diagnostic, "parsed_events")
                                    observe_event(record,event,known_secrets)
                                except (ValueError, TypeError):
                                    record["invalid_event"] = True
                            if client_open:
                                diagnostic["stage"] = "downstream_body"
                                try:
                                    self.wfile.write(line); self.wfile.flush()
                                except (BrokenPipeError, ConnectionResetError):
                                    client_open = False
                            diagnostic["stage"] = "upstream_body"
                        diagnostic["upstream_eof"] = True
                        diagnostic["stage"] = "upstream_eof"
                except Exception as error:
                    diagnostic_error(diagnostic, error)
                    if diagnostic["stage"] in ("downstream_headers", "downstream_body") and isinstance(error, (BrokenPipeError, ConnectionResetError)):
                        client_open = False
                    record["error"] = type(error).__name__
                    if isinstance(error, urllib.error.HTTPError):
                        diagnostic_headers(diagnostic, error.code, error.headers, known_secrets)
                        record["http_status"] = error.code
                        record["provider_error"] = error.read(8000).decode("utf-8", errors="replace").replace(meter.key,"[REDACTED]")
                    if client_open:
                        try: self.send_error(502, "Provider request failed")
                        except (BrokenPipeError, ConnectionResetError): client_open = False
                        except Exception: pass
                finally:
                    # False means no disconnect was observed, not confirmed receipt.
                    record["client_disconnected"] = not client_open
                    diagnostic["client_disconnected"] = not client_open
                    diagnostic["stage"] = "finalized"
                    record["finished_at"] = time.time()
                    record["elapsed_seconds"] = record["finished_at"] - record["started_at"]
                    directory.mkdir(parents=True, exist_ok=True)
                    (directory / "response.sse").write_bytes(b"".join(chunks))
                    write_json(directory / "meter.json", record)
                    with meter.lock:
                        meter.active -= 1
                        write_json(meter.output / "requests.json", meter.records)
        return Handler

    def settle(self):
        deadline = time.monotonic() + 190
        while self.active and time.monotonic() < deadline:
            time.sleep(.1)
        if self.active:
            raise RuntimeError("Provider accounting did not settle")


class Daemon:
    def __init__(self, root, meter, proxy_url):
        self.root, self.meter = root, meter
        root.mkdir(parents=True, exist_ok=True)
        (root / "tmp").mkdir(exist_ok=True)
        self.token = secrets.token_urlsafe(32)
        # Use the user's runtime executables but a fresh S-Code identity/store/cache.
        environment = {key:value for key,value in os.environ.items() if not key.startswith(("S_CODE_", "OPENCODING_")) and key not in {"OPENAI_API_KEY", "OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"}}
        environment.update({"TMPDIR":str(root / "tmp"), "S_CODE_HOME":str(root / "home"), "S_CODE_TOOL_CACHE_DIR":str(root / "tool-cache"), "S_CODE_STATE_DIR":str(root / "state"), "S_CODE_RUNTIME_DIR":str(root / "runtime"), "S_CODE_DAEMON_LISTEN":"127.0.0.1:0", "S_CODE_TOKEN":self.token, "S_CODE_DAEMON_AUTH_MODE":"development_token", "S_CODE_MODEL_PROVIDER":"openai_compatible", "S_CODE_MODEL_REASONING_EFFORT":"low", "S_CODE_MODEL_BASE_URL":proxy_url, "S_CODE_MODEL_CREDENTIAL_HANDLE":"BENCH_PROXY_TOKEN", "BENCH_PROXY_TOKEN":meter.proxy_token, "S_CODE_MCP_ENABLED":"false"})
        self.log = (root / "daemon.log").open("w+")
        self.process = subprocess.Popen([str(meter.binary)], cwd=root, env=environment, stdout=self.log, stderr=self.log)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            self.log.flush()
            lines = (root / "daemon.log").read_text().splitlines()
            address = next((line.split("S_CODE_ADDR=", 1)[1].strip() for line in lines if "S_CODE_ADDR=" in line), None)
            if address:
                self.url = "http://" + address
                return
            if self.process.poll() is not None:
                raise RuntimeError("Daemon startup failed; inspect its private log")
            time.sleep(.1)
        raise RuntimeError("Daemon startup timed out")

    def api(self, method, path, value=None):
        request = urllib.request.Request(self.url + path, method=method, data=None if value is None else json.dumps(value).encode(), headers={"Authorization":"Bearer " + self.token, "Content-Type":"application/json"})
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = response.read()
        return json.loads(payload) if payload else None

    def session(self, workspace, name, mode):
        session = self.api("POST", "/v1/sessions", {"scope":SCOPE, "workspace_uri":workspace.as_uri(), "title":name, "model":self.meter.model})
        sid = session["id"]
        self.api("PATCH", f"/v1/sessions/{sid}/preferences", {"scope":SCOPE, "permission_mode":"workspace"})
        settings = self.api("GET", f"/v1/sessions/{sid}/learning?{QUERY}")
        if settings["mode"] != mode:
            self.api("PUT", f"/v1/sessions/{sid}/learning", {"scope":SCOPE, "mode":mode})
        return sid

    def run(self, workspace, task, mode, output, seed=17, arm=None):
        sid = self.session(workspace, task["id"], mode)
        self.meter.context = {"task":task["id"], "phase":task["phase"], "arm":arm or mode, "seed":seed}
        before = len(self.meter.records)
        self.meter.phase_start = before
        denied_before = len(self.meter.denials)
        copy_tree(workspace, output / "initial")
        started = time.monotonic()
        turn = self.api("POST", f"/v1/sessions/{sid}/turns", {"scope":SCOPE, "content":task["prompt"], "generate_title":False})
        turn_id = turn["id"]
        while True:
            turn = self.api("GET", f"/v1/turns/{turn_id}?{QUERY}")
            if turn["status"] in {"completed", "failed", "cancelled", "awaiting_approval", "awaiting_input"}:
                break
            if time.monotonic() - started > 900:
                self.api("POST", f"/v1/turns/{turn_id}/cancel", {"scope":SCOPE})
                drain_deadline = time.monotonic() + 30
                while time.monotonic() < drain_deadline:
                    turn = self.api("GET", f"/v1/turns/{turn_id}?{QUERY}")
                    if turn["status"] in {"completed", "failed", "cancelled"}: break
                    time.sleep(.2)
                if turn["status"] not in {"completed", "failed", "cancelled"}:
                    raise RuntimeError("Cancelled task did not reach a terminal state; artifact cannot be graded")
                break
            time.sleep(.5)
        elapsed = time.monotonic() - started
        self.meter.settle()
        snapshot = self.api("GET", f"/v1/sessions/{sid}/snapshot?{QUERY}")
        lessons = self.api("GET", f"/v1/sessions/{sid}/lessons?{QUERY}")
        # Capture the outcome before a later learn/reuse mode change clears it.
        learning = self.api("GET", f"/v1/sessions/{sid}/learning?{QUERY}")
        write_json(output / "learning.json", learning)
        write_json(output / "turn.json", turn)
        write_json(output / "snapshot.json", snapshot)
        write_json(output / "lessons.json", lessons)
        # Freeze filesystem state outside the agent's root, including new files and
        # changes already committed by the agent. Never trust its Git index/HEAD.
        final = output / "final"
        shutil.copytree(workspace, final, ignore=shutil.ignore_patterns(".git", "__pycache__", "*.pyc"), symlinks=True)
        if any(path.is_symlink() for path in final.rglob("*")):
            raise RuntimeError("Candidate contains a symlink; supervisor isolation requires review")
        supervisor = output / "patch-tree"
        copy_tree(output / "initial", supervisor)
        for entry in supervisor.iterdir():
            if entry.name == ".git": continue
            if entry.is_dir(): shutil.rmtree(entry)
            else: entry.unlink()
        shutil.copytree(final, supervisor, dirs_exist_ok=True)
        subprocess.run(["git", "-C", str(supervisor), "add", "--all"], check=True)
        for path in final.rglob("*"):
            if path.is_file():
                subprocess.run(["git", "-C", str(supervisor), "add", "-f", "--", str(path.relative_to(final))], check=True)
        patch = subprocess.check_output(["git", "-C", str(supervisor), "diff", "--cached", "--binary", "HEAD"])
        (output / "patch.diff").write_bytes(patch)
        records = self.meter.records[before:]
        totals = provider_totals(records)
        usage_complete = totals is not None
        result = {"task":task["id"], "mode":mode, "seed":seed, "status":turn["status"], "elapsed_seconds":elapsed, "daemon_usage":snapshot["usage"], "provider_usage":totals, "provider_usage_complete":usage_complete, "cost_usd":totals["cost_usd"] if usage_complete else None, "requests":[record["index"] for record in records], "lesson_count":len(lessons), "workspace_hash":tree_hash(workspace), "budget_denied":len(self.meter.denials) > denied_before}
        write_json(output / "result.json", result)
        print(json.dumps(result), flush=True)
        return result

    def close(self):
        self.process.terminate()
        try: self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill(); self.process.wait()
        self.log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--key-file", type=Path, required=True)
    parser.add_argument("--model", default="z-ai/glm-5.3")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--task", type=Path, required=True, help="JSON with id, phase, prompt and fixture path")
    parser.add_argument("--mode", choices=["learn", "reuse", "off"], default="learn")
    parser.add_argument("--provider", default="z-ai/fp8")
    parser.add_argument("--max-cost", type=float, default=5.0)
    parser.add_argument("--max-calls", type=int, default=60)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists(): parser.error("output must be new, to preserve all attempts")
    output.mkdir(parents=True, mode=0o700)
    key = args.key_file.read_text().strip()
    if not key: parser.error("credential file is empty")
    task = json.loads(args.task.read_text())
    fixture = (args.task.parent / task["fixture"]).resolve()
    write_json(output / "task.json", task)
    write_json(output / "manifest.json", {"revision":subprocess.check_output(["git","-C",str(ROOT),"rev-parse","HEAD"],text=True).strip(), "fixture_hash":tree_hash(fixture), "model":args.model, "mode":args.mode, "max_cost":args.max_cost, "max_calls":args.max_calls, "phase":"pilot", "provider":args.provider, "daemon_sha256":hashlib.sha256((ROOT / "target/debug/s-code-daemon").read_bytes()).hexdigest(), "runner_sha256":hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "source_sha256":snapshot_source(output)})
    meter = Meter(key, output, args.model, args.max_cost, args.max_calls, args.provider)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), meter.handler())
    threading.Thread(target=server.serve_forever, daemon=True).start()
    daemon = None
    try:
        workspace = output / "workspace"
        copy_tree(fixture, workspace)
        daemon = Daemon(output / "daemon", meter, f"http://127.0.0.1:{server.server_port}/v1")
        daemon.run(workspace, task, args.mode, output / "task-result")
    finally:
        if daemon: daemon.close()
        server.shutdown(); server.server_close()


if __name__ == "__main__":
    main()
