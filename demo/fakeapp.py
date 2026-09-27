"""Tiny exporter whose gauges the chaos script sets. Stdlib only."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

GAUGES = {}


class H(BaseHTTPRequestHandler):
    def do_GET(self):
        lines = []
        for (metric, labels), v in GAUGES.items():
            lab = ",".join(f'{k}="{val}"' for k, val in labels)
            lines.append(f"{metric}{{{lab}}} {v}")
        body = ("\n".join(lines) + "\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        GAUGES[(req["metric"], tuple(sorted(req["labels"].items())))] = req["value"]
        self.send_response(204)
        self.end_headers()

    def log_message(self, *a):
        pass


ThreadingHTTPServer(("0.0.0.0", 9100), H).serve_forever()
