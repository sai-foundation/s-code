#!/usr/bin/env python3
"""Loopback-only deterministic model for the native desktop's real-daemon tests."""
import hashlib
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import sys
import time

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path.rstrip('/') != '/v1/models':
            self.send_error(404)
            return
        data = json.dumps({'data': [{'id': 'desktop-fixture'}]}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        if self.path != '/v1/chat/completions':
            self.send_error(404)
            return
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages = request.get('messages', [])
        last_user = max((i for i, m in enumerate(messages) if m.get('role') == 'user'), default=-1)
        content = messages[last_user].get('content', '') if last_user >= 0 else ''
        if content == 'desktop failure':
            self.send_error(503, 'Fixture provider unavailable')
            return
        has_result = any(m.get('role') == 'tool' for m in messages[last_user + 1:])
        tool = None
        if content == 'desktop edit' and not has_result:
            tool = ('apply_patch', {'path': 'desktop-demo.txt', 'expected_sha256': hashlib.sha256(b'before desktop\n').hexdigest(), 'content': 'after desktop\n'})
        elif content == 'desktop question' and not has_result:
            tool = ('request_user_input', {'questions': [{'id': 'choice', 'header': 'Choose', 'question': 'Which option?', 'options': [{'label': 'Option A', 'description': 'First option'}, {'label': 'Option B', 'description': 'Second option'}]}], 'allow_other': True})
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        def emit(delta, finish=None):
            self.wfile.write(('data: ' + json.dumps({'choices': [{'delta': delta, 'finish_reason': finish}]}) + '\n\n').encode())
            self.wfile.flush()
        try:
            if tool:
                emit({'tool_calls': [{'index': 0, 'id': 'call_desktop', 'type': 'function', 'function': {'name': tool[0], 'arguments': json.dumps(tool[1])}}]})
                emit({}, 'tool_calls')
            else:
                text = 'Hello from the desktop fixture. 你好！\n\nThis is a real streamed response through the bundled Rust engine.\n\n```rust\nfn main() {\n    println!("Hello, S-Code!");\n}\n```\n\nReady for your next idea.'
                if has_result:
                    text = 'Your requested action is complete.'
                if content == 'desktop long':
                    text = '\n\n'.join(f'Paragraph {n}: A smooth desktop conversation with readable text and preserved history.' for n in range(150))
                for i in range(0, len(text), 5):
                    emit({'content': text[i:i + 5]})
                    time.sleep(2 if content == 'desktop slow' else 0.008)
                emit({}, 'stop')
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass

if __name__ == '__main__':
    server = ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1]) if len(sys.argv) > 1 else 0), Handler)
    print(server.server_port, flush=True)
    server.serve_forever()
