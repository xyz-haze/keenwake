"""Receives alertsift's outgoing webhooks, prints them and keeps them in out/sink.jsonl."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        print(f"{self.path}: {body.get('text')}", flush=True)
        with open("/out/sink.jsonl", "a") as f:
            f.write(json.dumps({"path": self.path, "body": body}) + "\n")
        self.send_response(200)
        self.end_headers()

    def log_message(self, *a):
        pass


ThreadingHTTPServer(("0.0.0.0", 9000), H).serve_forever()
