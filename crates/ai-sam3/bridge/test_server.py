"""Tests for the loopback SAM 3 bridge. Run from this folder: python -m unittest test_server

The request tests need only the standard library. The image tests need Pillow and are
skipped without it.
"""
import base64
import http.client
import io
import json
import threading
import unittest
import urllib.error
import urllib.request

from server import _host_allowed, handler_for, make_server, open_image

try:
    from PIL import Image
except ImportError:  # The request tests still run without Pillow.
    Image = None


def png_bytes(width=4, height=3):
    out = io.BytesIO()
    Image.new("RGB", (width, height), (10, 20, 30)).save(out, format="PNG")
    return out.getvalue()


class BridgeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.seen = []

        def fake(raw, prompt, mime_type):
            cls.seen.append((raw, prompt, mime_type))
            return {"width": 12, "height": 8, "boxes": [[1, 2, 3, 4]], "scores": [0.7]}

        cls.server = make_server(0, handler_for(fake))
        cls.port = cls.server.server_port
        cls.url = "http://127.0.0.1:%s/v1" % cls.port
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=2)

    def body(self, mime_type="image/png", data=b"test image"):
        return json.dumps({"prompt": "a car", "image": {
            "mime_type": mime_type, "data": base64.b64encode(data).decode()}}).encode()

    def raw_request(self, method, path, headers, body=None):
        """Send a request with exactly the given headers (no automatic Host)."""
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        try:
            conn.putrequest(method, path, skip_host=True, skip_accept_encoding=True)
            for name, value in headers:
                conn.putheader(name, value)
            if body is not None:
                conn.putheader("Content-Length", str(len(body)))
            conn.endheaders(body)
            resp = conn.getresponse()
            return resp.status, resp.read()
        finally:
            conn.close()

    def post_detect(self, headers):
        prior = len(self.seen)
        status, _ = self.raw_request("POST", "/v1/detect", headers, self.body())
        self.assertEqual(len(self.seen), prior, "the predictor must not run")
        return status

    def host(self, name="127.0.0.1"):
        return ("Host", "%s:%s" % (name, self.port))

    def test_health_and_detection(self):
        with urllib.request.urlopen(self.url + "/health") as resp:
            self.assertEqual(json.load(resp), {"model": "sam3", "ready": True})
        req = urllib.request.Request(self.url + "/detect", data=self.body(),
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req) as resp:
            self.assertEqual(json.load(resp)["boxes"], [[1, 2, 3, 4]])
        self.assertEqual(self.seen[-1], (b"test image", "a car", "image/png"))

    def test_localhost_host_and_json_charset_are_accepted(self):
        status, _ = self.raw_request("POST", "/v1/detect", [
            self.host("LocalHost"), ("Content-Type", "application/json; charset=utf-8")],
            self.body())
        self.assertEqual(status, 200)

    def test_invalid_content_fails_without_running_inference(self):
        prior = len(self.seen)
        data = json.dumps({"prompt": "x", "image": {"mime_type": "image/png", "data": "!"}}).encode()
        req = urllib.request.Request(self.url + "/detect", data=data,
                                     headers={"Content-Type": "application/json"})
        with self.assertRaises(urllib.error.HTTPError) as err:
            urllib.request.urlopen(req)
        self.assertEqual(err.exception.code, 400)
        self.assertEqual(len(self.seen), prior)

    def test_oversized_request_fails_before_reading(self):
        req = urllib.request.Request(self.url + "/detect", data=b"{}", headers={
            "Content-Length": str(25 * 1024 * 1024), "Content-Type": "application/json"})
        with self.assertRaises(urllib.error.HTTPError) as err:
            urllib.request.urlopen(req)
        self.assertIn(err.exception.code, (400, 413))

    def test_any_origin_header_is_refused(self):
        for origin in ("https://evil.example", "null", "http://127.0.0.1:%s" % self.port):
            with self.subTest(origin=origin):
                status = self.post_detect([self.host(), ("Origin", origin),
                                           ("Content-Type", "application/json")])
                self.assertEqual(status, 403)
        status, _ = self.raw_request("GET", "/v1/health", [self.host(), ("Origin", "null")])
        self.assertEqual(status, 403)

    def test_foreign_or_missing_host_is_refused(self):
        json_type = ("Content-Type", "application/json")
        cases = {
            "foreign name": [("Host", "evil.example:%s" % self.port), json_type],
            "rebound name": [("Host", "attacker.test"), json_type],
            "wrong port": [("Host", "127.0.0.1:1"), json_type],
            "no port": [("Host", "127.0.0.1"), json_type],
            "missing": [json_type],
            "repeated": [self.host(), ("Host", "evil.example"), json_type],
        }
        for name, headers in cases.items():
            with self.subTest(case=name):
                self.assertEqual(self.post_detect(headers), 403)
        status, _ = self.raw_request("GET", "/v1/health", [("Host", "evil.example")])
        self.assertEqual(status, 403)

    def test_body_that_is_not_json_is_refused(self):
        for value in ("text/plain", "application/x-www-form-urlencoded",
                      "multipart/form-data; boundary=x", "application/jsonx"):
            with self.subTest(content_type=value):
                self.assertEqual(self.post_detect([self.host(), ("Content-Type", value)]), 415)
        self.assertEqual(self.post_detect([self.host()]), 415)


class HostRuleTests(unittest.TestCase):
    def test_bare_name_means_port_80(self):
        self.assertTrue(_host_allowed(["127.0.0.1"], 80))
        self.assertTrue(_host_allowed(["localhost:80"], 80))
        self.assertFalse(_host_allowed(["127.0.0.1"], 8343))
        self.assertFalse(_host_allowed(["evil.example"], 80))
        self.assertFalse(_host_allowed([":80"], 80))


@unittest.skipIf(Image is None, "Pillow is not installed")
class ImageTests(unittest.TestCase):
    def test_declared_format_opens(self):
        image = open_image(png_bytes(), "image/png")
        self.assertEqual(image.size, (4, 3))

    def test_formats_outside_the_three_are_never_opened(self):
        eps = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 1 1\nshowpage\n"
        gif = io.BytesIO()
        Image.new("RGB", (2, 2)).save(gif, format="GIF")
        for raw in (eps, gif.getvalue(), b"BM" + b"\0" * 64):
            for mime_type in ("image/png", "image/jpeg", "image/webp"):
                with self.subTest(head=raw[:4], mime_type=mime_type):
                    with self.assertRaises(ValueError):
                        open_image(raw, mime_type)

    def test_bytes_must_match_the_declared_type(self):
        jpeg = io.BytesIO()
        Image.new("RGB", (2, 2)).save(jpeg, format="JPEG")
        with self.assertRaises(ValueError):
            open_image(png_bytes(), "image/jpeg")
        with self.assertRaises(ValueError):
            open_image(jpeg.getvalue(), "image/png")
        with self.assertRaises(ValueError):
            open_image(png_bytes(), "image/gif")

    def test_pixel_limit_is_checked_before_decoding(self):
        # A valid header claiming 7000 x 7000 pixels with almost no data behind it.
        # Decoding it would fail with a read error; the size check must refuse it first.
        raw = png_bytes(1, 1)
        ihdr = raw.index(b"IHDR")
        width_height = (7000).to_bytes(4, "big") * 2
        chunk = b"IHDR" + width_height + raw[ihdr + 12:ihdr + 17]
        import zlib
        crc = zlib.crc32(chunk).to_bytes(4, "big")
        forged = raw[:ihdr] + chunk + crc + raw[ihdr + 21:]
        with self.assertRaisesRegex(ValueError, "dimensions"):
            open_image(forged, "image/png")

    def test_rejected_picture_is_a_bad_request_not_a_failure(self):
        server = make_server(0, handler_for(lambda raw, _prompt, mime: {
            "size": list(open_image(raw, mime).size)}))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = "http://127.0.0.1:%s/v1/detect" % server.server_port

            def post(raw):
                data = json.dumps({"prompt": "a car", "image": {
                    "mime_type": "image/png", "data": base64.b64encode(raw).decode()}}).encode()
                req = urllib.request.Request(url, data=data,
                                             headers={"Content-Type": "application/json"})
                return urllib.request.urlopen(req, timeout=5)

            with post(png_bytes()) as resp:
                self.assertEqual(json.load(resp), {"size": [4, 3]})
            with self.assertRaises(urllib.error.HTTPError) as err:
                post(b"%!PS-Adobe-3.0 EPSF-3.0\nshowpage\n")
            self.assertEqual(err.exception.code, 400)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
