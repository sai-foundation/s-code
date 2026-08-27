#!/usr/bin/env python3
import http.server
import json
import os
import sys
import threading


class State:
    lock = threading.Lock()
    requests = 0


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, _format, *_args):
        return

    def do_GET(self):
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


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: model_fixture.py READY_FILE")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    with open(sys.argv[1], "w", encoding="utf-8") as ready:
        ready.write(f"127.0.0.1:{server.server_port}\n")
    server.serve_forever()


if __name__ == "__main__":
    main()
