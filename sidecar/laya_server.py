"""Local Laya sidecar exposing the System One route, so alertsift talks to Laya like to Jev.

Stdlib HTTP server, one model loaded once, calls serialised by a lock (one forward pass at a time).
"""
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def make_handler(agent):
    lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        def _send(self, code, body):
            data = body if isinstance(body, bytes) else json.dumps(body).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path == "/healthz":
                self._send(200, b"ok")
            else:
                self._send(404, {"error": "not found"})

        def do_POST(self):
            if self.path != "/v1/systemone":
                return self._send(404, {"error": "not found"})
            try:
                req = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
                state, questions = req["state"], req["questions"]
            except (ValueError, KeyError, TypeError):
                return self._send(400, {"error": "expected JSON with state and questions"})
            with lock:
                result = agent.predict(state, questions)
            self._send(200, result)

        def log_message(self, *args):
            pass

    return Handler


def main():
    os.environ.setdefault("USE_TF", "0")
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    import laya
    sub = os.environ.get("LAYA_SUBFOLDER", "typed-decisions")
    agent = laya.load("convaiinnovations/laya", **({"subfolder": sub} if sub else {}))
    agent.predict("warm up", {"q": {"type": "noul", "instructions": "Is this a warm-up?"}})
    host, port = os.environ.get("LAYA_HOST", "127.0.0.1"), int(os.environ.get("LAYA_PORT", "8771"))
    print(f"laya sidecar ({sub}) on {host}:{port}", flush=True)
    ThreadingHTTPServer((host, port), make_handler(agent)).serve_forever()


if __name__ == "__main__":
    main()
