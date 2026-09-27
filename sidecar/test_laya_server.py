import json
import threading
import unittest
import urllib.request
from http.server import ThreadingHTTPServer

from laya_server import make_handler


class FakeAgent:
    def predict(self, state, questions):
        return {"model": "fake", "answers": {q: {"type": "noul", "noul": 0.25} for q in questions},
                "usage": {"input_tokens": len(state)}}


class SidecarTest(unittest.TestCase):
    def setUp(self):
        self.srv = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(FakeAgent()))
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.srv.server_port}"

    def tearDown(self):
        self.srv.shutdown()

    def post(self, path, body):
        req = urllib.request.Request(self.url + path, data=json.dumps(body).encode(), method="POST",
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req) as r:
            return r.status, json.loads(r.read())

    def test_same_shape_as_jev(self):
        status, body = self.post("/v1/systemone", {"model": "x", "state": "abc",
                                                   "questions": {"page_now": {"type": "noul", "instructions": "?"}}})
        self.assertEqual(status, 200)
        self.assertEqual(body["answers"]["page_now"]["noul"], 0.25)
        self.assertEqual(body["usage"]["input_tokens"], 3)

    def test_bad_json_is_400(self):
        req = urllib.request.Request(self.url + "/v1/systemone", data=b"nope", method="POST")
        with self.assertRaises(urllib.error.HTTPError) as e:
            urllib.request.urlopen(req)
        self.assertEqual(e.exception.code, 400)

    def test_healthz(self):
        with urllib.request.urlopen(self.url + "/healthz") as r:
            self.assertEqual(r.read(), b"ok")


if __name__ == "__main__":
    unittest.main()
