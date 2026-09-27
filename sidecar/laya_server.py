"""Local Laya sidecar exposing the System One route, so keenwake talks to Laya like to Jev.

Stdlib HTTP server, one model loaded once, calls serialised by a lock (one forward pass at a time).
"""
import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MAX_BODY_BYTES = 1_048_576

MODEL_REPO = "convaiinnovations/laya"
# A commit of MODEL_REPO, so a push to its main branch never changes the weights keenwake runs.
# Change it only after re-measuring the corpus (tests/corpus.rs) against the new checkpoint.
MODEL_REVISION = "55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851"
# The files one checkpoint needs, as laya itself downloads them.
CHECKPOINT_FILES = ("rl_agent_config.json", "model.safetensors", "tokenizer/*", "encoder/*")


def load_agent(laya, snapshot_download, sub):
    """laya.load() takes no revision: download the pinned snapshot here, then load it from disk."""
    prefix = f"{sub}/" if sub else ""
    path = snapshot_download(
        MODEL_REPO,
        revision=MODEL_REVISION,
        allow_patterns=[prefix + name for name in CHECKPOINT_FILES],
        token=os.environ.get("HF_TOKEN") or None,
    )
    return laya.load(path, **({"subfolder": sub} if sub else {}))


def make_handler(agent):
    lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        # A slow or silent client must not hold a request thread forever.
        timeout = 30

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

            raw_length = self.headers.get("Content-Length")
            try:
                length = int(raw_length) if raw_length is not None else 0
            except ValueError:
                return self._send(400, {"error": "bad Content-Length"})
            if length < 0:
                return self._send(400, {"error": "bad Content-Length"})
            if length > MAX_BODY_BYTES:
                # Reject on the declared size alone: never read a body that big.
                return self._send(413, {"error": "body too large"})

            try:
                payload = json.loads(self.rfile.read(length))
            except ValueError:
                return self._send(400, {"error": "expected JSON with state and questions"})

            questions = payload.get("questions") if isinstance(payload, dict) else None
            shape_ok = (
                isinstance(payload, dict)
                and "state" in payload
                and isinstance(questions, dict)
                and all(isinstance(v, dict) for v in questions.values())
            )
            if not shape_ok:
                return self._send(400, {"error": "expected JSON with state and questions"})
            state = payload["state"]

            try:
                with lock:
                    result = agent.predict(state, questions)
            except Exception as exc:
                print(f"laya sidecar: prediction failed: {exc!r}", file=sys.stderr)
                return self._send(500, {"error": "prediction failed"})
            self._send(200, result)

        def log_message(self, *args):
            pass

    return Handler


def main():
    os.environ.setdefault("USE_TF", "0")
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    import laya
    from huggingface_hub import snapshot_download
    sub = os.environ.get("LAYA_SUBFOLDER", "typed-decisions")
    agent = load_agent(laya, snapshot_download, sub)
    agent.predict("warm up", {"q": {"type": "noul", "instructions": "Is this a warm-up?"}})
    host, port = os.environ.get("LAYA_HOST", "127.0.0.1"), int(os.environ.get("LAYA_PORT", "8771"))
    print(f"laya sidecar ({sub} at {MODEL_REVISION[:12]}) on {host}:{port}", flush=True)
    ThreadingHTTPServer((host, port), make_handler(agent)).serve_forever()


if __name__ == "__main__":
    main()
