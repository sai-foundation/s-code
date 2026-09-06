"""Content-free transport diagnostics: local fake HTTP/SSE and injected faults only."""
import contextlib
from email.message import Message
import errno
import http.client
import http.server
import io
import json
from pathlib import Path
import socket
import ssl
import tempfile
import threading
import unittest
from unittest import mock
import urllib.error
import urllib.request

from run import (Meter, DIAGNOSTIC_COUNTER_MAX, diagnostic_error,
                 diagnostic_generation_ids, diagnostic_headers,
                 diagnostic_increment, observe_event, provider_totals,
                 reserved_or_charged, transport_diagnostics)


def event(value):
    return b"data: " + json.dumps(value).encode() + b"\n\n"


@contextlib.contextmanager
def local_server(handler):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": .01})
    thread.start()
    try:
        yield server
    finally:
        server.shutdown(); server.server_close(); thread.join(3)
        if thread.is_alive(): raise AssertionError("Local fixture server did not stop")


class FakeResponse:
    def __init__(self, lines=(), *, ids=(), failure=None):
        self.status, self.headers = 200, Message()
        for value in ids: self.headers.add_header("X-Generation-Id", value)
        self.lines, self.failure = lines, failure

    def __enter__(self): return self
    def __exit__(self, *_): return False
    def __iter__(self):
        yield from self.lines
        if self.failure is not None: raise self.failure


class TransportTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        meter = Meter.__new__(Meter)
        meter.output = Path(self.temporary.name)
        meter.key, meter.proxy_token = "private-fixture-credential", "private-fixture-proxy"
        meter.model, meter.provider = "fixture/model", "fixture/provider"
        meter.prices = {"prompt": .000001, "completion": .000001}
        meter.max_cost, meter.max_calls = 5, 10
        meter.lock = threading.Lock()
        meter.records, meter.denials, meter.active = [], [], 0
        meter.phase_start, meter.phase_max_cost = 0, 1
        meter.context = dict(task="fixture-dev", phase="dev", arm="off", seed=17)
        meter.raw = None
        self.meter = meter
        self.body = dict(model=meter.model, max_tokens=1024, messages=[dict(role="user", content="synthetic task")])

    def record(self):
        self.assertEqual(self.meter.active, 0)
        self.assertEqual(len(self.meter.records), 1)
        record = json.loads((self.meter.output / "requests/0000/meter.json").read_text())
        self.assertEqual(json.loads((self.meter.output / "requests.json").read_text()), [record])
        self.assertEqual(record, self.meter.records[0])
        self.assertEqual(record["transport"]["stage"], "finalized")
        self.assertIsInstance(record["finished_at"], float)
        return record

    def injected_exchange(self, response=None, failure=None, writer=None, fail_headers=False):
        """Exercise the real POST handler without relying on socket-close races."""
        handler = self.meter.handler().__new__(self.meter.handler())
        handler.headers = {"Authorization": "Bearer " + self.meter.proxy_token,
                           "Content-Length": str(len(json.dumps(self.body).encode()))}
        handler.path = "/v1/chat/completions"
        handler.rfile = io.BytesIO(json.dumps(self.body).encode())
        handler.wfile = writer or io.BytesIO()
        handler.send_response = lambda *_: None
        handler.send_header = lambda *_: None
        def headers():
            if fail_headers: raise BrokenPipeError(errno.EPIPE, "private header error")
        handler.end_headers = headers
        handler.send_error = lambda *_: None
        def open_upstream(request, timeout):
            self.assertEqual(request.full_url, "https://openrouter.ai/api/v1/chat/completions")
            self.assertEqual(timeout, 180)
            if failure is not None: raise failure
            return response
        with mock.patch("run.urllib.request.urlopen", side_effect=open_upstream) as opened:
            handler.do_POST()
        self.assertEqual(opened.call_count, 1)  # Diagnostics must never retry.
        return self.record()

    def http_exchange(self, payload, *, ids=(), status=200, truncated_chunk=False):
        """Real localhost upstream and proxy; all external opens are intercepted."""
        received = []
        class Upstream(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_POST(self):
                received.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
                self.send_response(status)
                self.send_header("Content-Type", "text/event-stream")
                for value in ids: self.send_header("X-Generation-Id", value)
                if truncated_chunk: self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                self.wfile.write(b"5\r\nab" if truncated_chunk else payload)
                self.wfile.flush()
                self.close_connection = True
        real_urlopen = urllib.request.urlopen
        with local_server(Upstream) as upstream:
            local_url = f"http://127.0.0.1:{upstream.server_port}/fixture"
            def redirect_to_fixture(request, timeout):
                self.assertEqual(request.full_url, "https://openrouter.ai/api/v1/chat/completions")
                self.assertEqual(timeout, 180)
                # Never forward even the fake credential outside the proxy.
                local_request = urllib.request.Request(local_url, data=request.data, headers={"Content-Type": "application/json"})
                return real_urlopen(local_request, timeout=3)
            with mock.patch("run.urllib.request.urlopen", side_effect=redirect_to_fixture) as opened:
                with local_server(self.meter.handler()) as proxy:
                    connection = http.client.HTTPConnection("127.0.0.1", proxy.server_port, timeout=3)
                    try:
                        connection.request("POST", "/v1/chat/completions", json.dumps(self.body),
                            {"Authorization": "Bearer " + self.meter.proxy_token, "Content-Type": "application/json"})
                        connection.getresponse().read()
                    finally:
                        connection.close()
                    self.meter.settle()
            self.assertEqual(opened.call_count, 1)
        self.assertEqual(len(received), 1)
        self.assertEqual(received[0]["messages"], self.body["messages"])
        self.assertEqual(received[0]["provider"], dict(only=[self.meter.provider], order=[self.meter.provider], allow_fallbacks=False))
        return self.record()

    def test_real_sse_records_header_and_body_without_changing_usage(self):
        payload = event(dict(id="gen-fixture", choices=[])) + event(dict(id="gen-fixture", usage=dict(prompt_tokens=11, completion_tokens=7, cost=.1))) + b"data: [DONE]\n\n"
        record = self.http_exchange(payload, ids=["gen-fixture"])
        d = record["transport"]
        self.assertEqual((d["header_generation_id"], d["sse_generation_id"]), ("gen-fixture", "gen-fixture"))
        self.assertEqual((d["header_generation_id_count"], d["sse_generation_id_count"]), (1, 2))
        self.assertEqual(d["http_status"], 200)
        self.assertEqual(d["received_bytes"], len(payload))
        self.assertEqual((d["sse_data_lines"], d["parsed_events"]), (2, 2))
        self.assertTrue(all(d[key] for key in ("headers_received", "body_received", "usage_event_received", "done_received", "upstream_eof")))
        self.assertFalse(d["generation_id_conflict"])
        self.assertIsNone(d["error_category"])
        self.assertEqual(provider_totals([record])["total_tokens"], 18)
        self.assertEqual(reserved_or_charged(record), .1)

    def test_real_headers_then_truncated_body_retains_id_and_unknown_usage(self):
        record = self.http_exchange(b"", ids=["gen-before-disconnect"], truncated_chunk=True)
        d = record["transport"]
        self.assertEqual(d["header_generation_id"], "gen-before-disconnect")
        self.assertEqual(d["sse_generation_id_state"], "missing")
        self.assertEqual(d["failure_stage"], "upstream_body")
        self.assertEqual(d["error_category"], "http_protocol")
        self.assertEqual(record["error"], "IncompleteRead")
        self.assertTrue(d["headers_received"])
        self.assertFalse(d["upstream_eof"])
        self.assertIsNone(record["usage"]); self.assertIsNone(record["cost"])
        self.assertIsNone(provider_totals([record]))
        self.assertEqual(reserved_or_charged(record), record["reserved_cost"])

    def test_open_failure_keeps_nested_dns_reason_without_private_error_text(self):
        private = "private hostname and credential " + self.meter.key
        record = self.injected_exchange(failure=urllib.error.URLError(socket.gaierror(socket.EAI_AGAIN, private)))
        d = record["transport"]
        self.assertEqual(record["error"], "URLError")
        self.assertEqual((d["error_category"], d["reason_category"], d["failure_stage"]), ("url_error", "name_resolution", "upstream_open"))
        self.assertEqual(d["reason_errno"], socket.EAI_AGAIN)
        self.assertFalse(d["headers_received"])
        self.assertEqual(d["header_generation_id_state"], "missing")
        self.assertIsNone(provider_totals([record]))
        self.assertEqual(reserved_or_charged(record), record["reserved_cost"])
        self.assertNotIn("private hostname", json.dumps(d))
        self.assertNotIn(self.meter.key, json.dumps(d))

    def test_safe_nested_categories_and_bounded_codes(self):
        certificate = ssl.SSLCertVerificationError(1, "private TLS detail")
        certificate.verify_code = 20
        cases = [(certificate, "tls_verification"), (ssl.SSLError(1, "private"), "tls"),
                 (TimeoutError(errno.ETIMEDOUT, "private"), "timeout"),
                 (ConnectionRefusedError(errno.ECONNREFUSED, "private"), "connection_refused"),
                 ("private URL including credential", "unknown")]
        for cause, expected in cases:
            d = transport_diagnostics(); d["stage"] = "upstream_open"
            diagnostic_error(d, urllib.error.URLError(urllib.error.URLError(cause)))
            self.assertEqual(d["reason_category"], expected)
            self.assertEqual(d["failure_stage"], "upstream_open")
            self.assertNotIn("private", json.dumps(d))
            if cause is certificate: self.assertEqual(d["tls_verify_code"], 20)
        cycle = urllib.error.URLError("private"); cycle.reason = cycle
        d = transport_diagnostics(); diagnostic_error(d, cycle)
        self.assertEqual(d["reason_category"], "url_error")
        large = OSError(); large.errno = 1 << 100
        d = transport_diagnostics(); diagnostic_error(d, large)
        self.assertIsNone(d["error_errno"])

    def test_real_http_error_also_captures_header_without_replacing_legacy_body(self):
        payload = ("fixture error " + self.meter.key).encode()
        record = self.http_exchange(payload, ids=["gen-http-error"], status=503)
        self.assertEqual(record["http_status"], 503)
        self.assertEqual(record["provider_error"], "fixture error [REDACTED]")
        self.assertEqual(record["transport"]["http_status"], 503)
        self.assertEqual(record["transport"]["header_generation_id"], "gen-http-error")
        self.assertEqual(record["transport"]["error_category"], "http_error")
        self.assertIsNone(provider_totals([record]))

    def test_duplicate_header_conflicts_are_not_silently_resolved(self):
        record = self.http_exchange(event(dict(id="gen-one", usage=dict(prompt_tokens=1, completion_tokens=2, cost=.01))), ids=["gen-one", "gen-two"])
        d = record["transport"]
        self.assertEqual(d["header_generation_id_state"], "conflict")
        self.assertIsNone(d["header_generation_id"])
        self.assertEqual(d["header_generation_id_count"], 2)
        self.assertTrue(d["generation_id_conflict"])
        # Identity diagnostics do not rewrite existing accounting policy.
        self.assertEqual(record["generation_id"], "gen-one")
        self.assertEqual(provider_totals([record])["total_tokens"], 3)

    def test_missing_invalid_repeated_and_conflicting_ids_stay_distinct(self):
        d = transport_diagnostics(); diagnostic_headers(d, 200, Message())
        self.assertEqual(d["header_generation_id_state"], "missing")
        headers = Message(); headers.add_header("X-Generation-Id", "gen-same"); headers.add_header("X-Generation-Id", "gen-same")
        diagnostic_headers(d, 200, headers)
        self.assertEqual(d["header_generation_id"], "gen-same")
        self.assertFalse(d["generation_id_conflict"])
        record = {"transport": d}
        observe_event(record, {"id": "gen-other"})
        self.assertTrue(d["generation_id_conflict"])
        observe_event(record, {"id": "gen-third"})
        self.assertEqual(d["sse_generation_id_state"], "conflict")
        self.assertIsNone(d["sse_generation_id"])
        self.assertEqual(record["generation_id"], "gen-third")
        for value in ("", "gen-private-fixture-credential", "gen-private-fixture-proxy", "gen-" + "x" * 201, "private header\nvalue", None):
            d = transport_diagnostics()
            diagnostic_generation_ids(d, "header", [value, "gen-valid-later"], (self.meter.key, self.meter.proxy_token))
            self.assertEqual(d["header_generation_id_state"], "invalid")
            self.assertIsNone(d["header_generation_id"])
            self.assertNotIn("private", json.dumps(d))
        self.assertLess(len(json.dumps(d)), 2048)

    def test_downstream_disconnect_is_finalized_even_if_upstream_later_fails(self):
        class DisconnectedWriter:
            def write(self, _): raise BrokenPipeError(errno.EPIPE, "private downstream")
            def flush(self): pass
        line = event(dict(id="gen-known", usage=dict(prompt_tokens=11, completion_tokens=7, cost=.1)))
        record = self.injected_exchange(FakeResponse([line], ids=["gen-known"], failure=TimeoutError("private read timeout")), writer=DisconnectedWriter())
        d = record["transport"]
        self.assertTrue(record["client_disconnected"]); self.assertTrue(d["client_disconnected"])
        self.assertEqual((d["error_category"], d["failure_stage"]), ("timeout", "upstream_body"))
        self.assertTrue(d["usage_event_received"])
        self.assertEqual(d["received_bytes"], len(line))
        self.assertEqual(record["usage"]["prompt_tokens"], 11)
        self.assertEqual(record["cost"], .1)
        self.assertEqual(record["error"], "TimeoutError")
        self.assertIsNone(provider_totals([record]))

    def test_header_forward_disconnect_and_first_provider_error_remain_observable(self):
        record = self.injected_exchange(FakeResponse([], ids=["gen-header"]), fail_headers=True)
        self.assertTrue(record["client_disconnected"])
        self.assertEqual(record["transport"]["failure_stage"], "downstream_headers")
        self.assertEqual(record["transport"]["header_generation_id"], "gen-header")
        d = transport_diagnostics(); d["stage"] = "upstream_body"
        record = {"transport": d}
        observe_event(record, {"usage": {"prompt_tokens": 1, "completion_tokens": 2, "cost": .01}, "error": {"message": "private provider text"}})
        diagnostic_error(d, TimeoutError("private later error"))
        self.assertEqual(d["error_category"], "provider_stream_error")
        self.assertIsNone(provider_totals([record]))
        self.assertNotIn("private", json.dumps(d))

    def test_counters_saturate_and_diagnostics_are_optional_for_legacy_records(self):
        d = transport_diagnostics()
        diagnostic_increment(d, "received_bytes", DIAGNOSTIC_COUNTER_MAX)
        diagnostic_increment(d, "received_bytes", 100)
        self.assertEqual(d["received_bytes"], DIAGNOSTIC_COUNTER_MAX)
        self.assertTrue(d["counters_saturated"])
        legacy = {}
        observe_event(legacy, {"id": "legacy-id", "usage": {"prompt_tokens": 1, "completion_tokens": 2, "cost": 0}})
        self.assertNotIn("transport", legacy)
        self.assertEqual(provider_totals([legacy])["total_tokens"], 3)


if __name__ == "__main__":
    unittest.main()
