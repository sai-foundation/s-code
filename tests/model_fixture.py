#!/usr/bin/env python3
import http.server
import json
import os
import sys
import threading
import time


class State:
    lock = threading.Lock()
    requests = 0


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, _format, *_args):
        return

    def do_GET(self):
        if self.path == "/models":
            if self.headers.get("authorization") != "Bearer fixture-secret":
                self.send_error(401)
                return
            body = b'{"object":"list","data":[{"id":"fixture/model","object":"model"}]}'
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path != "/requests":
            self.send_error(404)
            return
        with State.lock:
            count = State.requests
        body = json.dumps({"requests": count}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if self.path != "/chat/completions":
            self.send_error(404)
            return
        if self.headers.get("authorization") != "Bearer fixture-secret":
            self.send_error(401)
            return
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length))
        tool_names = {
            tool.get("function", {}).get("name") for tool in request.get("tools", [])
        }
        review = any(
            message.get("role") == "user"
            and isinstance(message.get("content"), str)
            and message["content"].startswith("Review ")
            for message in request.get("messages", [])
        )
        if review:
            if "apply_patch" in tool_names or "run_command" in tool_names:
                self.send_error(400, "review exposed a mutating tool")
                return
            if "git_diff" not in tool_names or "read_file" not in tool_names:
                self.send_error(400, "review read-only tools missing")
                return
        elif "apply_patch" not in tool_names:
            self.send_error(400, "apply_patch tool missing")
            return
        last_user_message = next(
            (
                message
                for message in reversed(request.get("messages", []))
                if message.get("role") == "user"
            ),
            {},
        )
        viewport_stream = (
            last_user_message.get("content") == "stream terminal viewport"
        )
        if viewport_stream:
            self.send_viewport_stream()
            return
        with State.lock:
            State.requests += 1
            number = State.requests
        if number == 1:
            arguments = json.dumps(
                {
                    "path": "tracked.txt",
                    "expected_sha256": os.environ["FIXTURE_EXPECTED_SHA256"],
                    "content": "approved via cli\n",
                },
                separators=(",", ":"),
            )
            frames = [
                {
                    "choices": [
                        {
                            "delta": {
                                "tool_calls": [
                                    {
                                        "index": 0,
                                        "id": "call_tui_e2e",
                                        "function": {
                                            "name": "apply_patch",
                                            "arguments": arguments,
                                        },
                                    }
                                ]
                            },
                            "finish_reason": None,
                        }
                    ]
                },
                {"choices": [], "usage": {"prompt_tokens": 20, "completion_tokens": 8}},
                {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]},
            ]
        else:
            if number == 2 and not any(
                message.get("role") == "tool" for message in request.get("messages", [])
            ):
                self.send_error(400, "tool result missing")
                return
            frames = [
                {
                    "choices": [
                        {
                            "delta": {"content": "write complete"},
                            "finish_reason": None,
                        }
                    ]
                },
                {"choices": [], "usage": {"prompt_tokens": 30, "completion_tokens": 4}},
                {"choices": [{"delta": {}, "finish_reason": "stop"}]},
            ]
        body = "".join(f"data: {json.dumps(frame, separators=(',', ':'))}\n\n" for frame in frames)
        body += "data: [DONE]\n\n"
        encoded = body.encode()
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def send_viewport_stream(self):
        gate = os.environ.get("FIXTURE_STREAM_GATE")
        if not gate:
            self.send_error(500, "FIXTURE_STREAM_GATE is required")
            return
        phase_one = "\n".join(
            [f"VIEWPORT_ROW_{index:03}" for index in range(80)]
            + ["PHASE_ONE_TAIL"]
        )
        phase_two = "\n" + "\n".join(
            [f"VIEWPORT_ROW_{index:03}" for index in range(80, 95)]
            + ["PHASE_TWO_TAIL"]
        )
        final_phase = "\n" + "\n".join(
            [f"VIEWPORT_ROW_{index:03}" for index in range(95, 110)]
            + ["FINAL_STREAM_TAIL"]
        )
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()

        first = {
            "choices": [
                {"delta": {"content": phase_one}, "finish_reason": None}
            ]
        }
        self.wfile.write(
            f"data: {json.dumps(first, separators=(',', ':'))}\n\n".encode()
        )
        self.wfile.flush()

        phase_two_gate = f"{gate}.phase-two"
        finish_gate = f"{gate}.finish"
        deadline = time.monotonic() + 20
        while not os.path.exists(phase_two_gate):
            if time.monotonic() >= deadline:
                raise TimeoutError("terminal viewport phase-two gate was not released")
            time.sleep(0.02)

        second = {
            "choices": [
                {"delta": {"content": phase_two}, "finish_reason": None}
            ]
        }
        self.wfile.write(
            f"data: {json.dumps(second, separators=(',', ':'))}\n\n".encode()
        )
        self.wfile.flush()

        deadline = time.monotonic() + 20
        while not os.path.exists(finish_gate):
            if time.monotonic() >= deadline:
                raise TimeoutError("terminal viewport finish gate was not released")
            time.sleep(0.02)

        frames = [
            {
                "choices": [
                    {"delta": {"content": final_phase}, "finish_reason": None}
                ]
            },
            {
                "choices": [
                    {
                        "delta": {
                            "reasoning_details": [
                                {
                                    "type": "reasoning.summary",
                                    "summary": "Checked the terminal viewport fixture.",
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [],
                "usage": {"prompt_tokens": 20, "completion_tokens": 110},
            },
            {"choices": [{"delta": {}, "finish_reason": "stop"}]},
        ]
        for frame in frames:
            self.wfile.write(
                f"data: {json.dumps(frame, separators=(',', ':'))}\n\n".encode()
            )
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: model_fixture.py READY_FILE")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    with open(sys.argv[1], "w", encoding="utf-8") as ready:
        ready.write(f"127.0.0.1:{server.server_port}\n")
    server.serve_forever()


if __name__ == "__main__":
    main()
