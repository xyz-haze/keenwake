import http.client
import json
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request
from contextlib import contextmanager
from http.server import ThreadingHTTPServer

from laya_server import make_handler


class FakeAgent:
    def predict(self, state, questions):
        return {"model": "fake", "answers": {q: {"type": "noul", "noul": 0.25} for q in questions},
                "usage": {"input_tokens": len(state)}}


class RaisingAgent:
    def predict(self, state, questions):
        raise RuntimeError("boom")


@contextmanager
def running_server(agent):
    srv = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(agent))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{srv.server_port}"
    finally:
        srv.shutdown()
        srv.server_close()


class SidecarTest(unittest.TestCase):
    def setUp(self):
        self.srv = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FakeAgent()))
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.srv.server_port}"

    def tearDown(self):
        self.srv.shutdown()
        self.srv.server_close()

    def post(self, path, payload):
        req = urllib.request.Request(self.url + path, data=json.dumps(payload).encode(), method="POST",
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            with e:
                return e.code, json.loads(e.read())

    def raw_post(self, path, content_length, body):
        """POST with a hand-picked Content-Length header, possibly not matching len(body)."""
        parsed = urllib.parse.urlsplit(self.url)
        conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=5)
        try:
            conn.putrequest("POST", path)
            if content_length is not None:
                conn.putheader("Content-Length", str(content_length))
            conn.endheaders()
            if body:
                conn.send(body)
            resp = conn.getresponse()
            data = resp.read()
            resp.close()
            return resp.status, data
        finally:
            conn.close()

    def test_same_shape_as_jev(self):
        status, body = self.post("/v1/systemone", {"model": "x", "state": "abc",
                                                   "questions": {"page_now": {"type": "noul", "instructions": "?"}}})
        self.assertEqual(status, 200)
        self.assertEqual(body["answers"]["page_now"]["noul"], 0.25)
        self.assertEqual(body["usage"]["input_tokens"], 3)

    def test_bad_json_is_400(self):
        status, body = self.raw_post("/v1/systemone", len(b"nope"), b"nope")
        self.assertEqual(status, 400)
        self.assertEqual(json.loads(body)["error"], "expected JSON with state and questions")

    def test_healthz(self):
        with urllib.request.urlopen(self.url + "/healthz") as r:
            self.assertEqual(r.read(), b"ok")

    def test_negative_content_length_is_400(self):
        status, body = self.raw_post("/v1/systemone", -5, b"{}")
        self.assertEqual(status, 400)
        self.assertEqual(json.loads(body)["error"], "bad Content-Length")

    def test_non_integer_content_length_is_400(self):
        status, body = self.raw_post("/v1/systemone", "not-a-number", b"{}")
        self.assertEqual(status, 400)
        self.assertEqual(json.loads(body)["error"], "bad Content-Length")

    def test_oversized_content_length_is_413(self):
        # Declares a body far bigger than the 1 MiB limit but only sends a couple of bytes:
        # the server must reject on the header alone, never blocking on rfile.read().
        status, body = self.raw_post("/v1/systemone", 1_048_577, b"{}")
        self.assertEqual(status, 413)
        self.assertEqual(json.loads(body)["error"], "body too large")

    def test_questions_as_list_is_400(self):
        status, body = self.post("/v1/systemone", {"model": "x", "state": "abc", "questions": [1, 2, 3]})
        self.assertEqual(status, 400)
        self.assertEqual(body["error"], "expected JSON with state and questions")

    def test_missing_state_is_400(self):
        status, body = self.post("/v1/systemone", {"model": "x",
                                                   "questions": {"page_now": {"type": "noul"}}})
        self.assertEqual(status, 400)
        self.assertEqual(body["error"], "expected JSON with state and questions")

    def test_predict_error_is_500(self):
        with running_server(RaisingAgent()) as url:
            req = urllib.request.Request(url + "/v1/systemone",
                                         data=json.dumps({"model": "x", "state": "abc",
                                                          "questions": {"page_now": {"type": "noul"}}}).encode(),
                                         method="POST", headers={"Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(req) as r:
                    status, body = r.status, json.loads(r.read())
            except urllib.error.HTTPError as e:
                with e:
                    status, body = e.code, json.loads(e.read())
        self.assertEqual(status, 500)
        self.assertEqual(body["error"], "prediction failed")


if __name__ == "__main__":
    unittest.main()
