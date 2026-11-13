import base64
import json
import socket
import threading
import time
import unittest
import urllib.error
import urllib.request
import server
from server import BridgeServer, handler_for


class BridgeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.seen = []

        def fake(raw, prompt):
            cls.seen.append((raw, prompt))
            return {"width": 12, "height": 8, "boxes": [[1, 2, 3, 4]], "scores": [0.7]}

        server.IDLE_SECONDS, server.BODY_SECONDS, server.HEADER_SECONDS = 1, 2, 2
        cls.server = BridgeServer(("127.0.0.1", 0), handler_for(fake))
        cls.url = "http://127.0.0.1:%s/v1" % cls.server.server_port
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=2)

    def test_health_and_detection(self):
        with urllib.request.urlopen(self.url + "/health") as resp:
            self.assertEqual(json.load(resp), {"model": "sam3", "ready": True})
        image = base64.b64encode(b"test image").decode()
        data = json.dumps({"prompt": "a car", "image": {"mime_type": "image/png", "data": image}}).encode()
        with urllib.request.urlopen(urllib.request.Request(self.url + "/detect", data=data)) as resp:
            self.assertEqual(json.load(resp)["boxes"], [[1, 2, 3, 4]])
        self.assertEqual(self.seen[-1], (b"test image", "a car"))

    def test_invalid_content_fails_without_running_inference(self):
        prior = len(self.seen)
        data = json.dumps({"prompt": "x", "image": {"mime_type": "image/png", "data": "!"}}).encode()
        with self.assertRaises(urllib.error.HTTPError) as err:
            urllib.request.urlopen(urllib.request.Request(self.url + "/detect", data=data))
        self.assertEqual(err.exception.code, 400)
        self.assertEqual(len(self.seen), prior)

    def test_oversized_request_fails_before_reading(self):
        req = urllib.request.Request(self.url + "/detect", data=b"{}", headers={"Content-Length": str(25 * 1024 * 1024)})
        with self.assertRaises(urllib.error.HTTPError) as err:
            urllib.request.urlopen(req)
        self.assertIn(err.exception.code, (400, 413))

    def test_a_stalled_body_neither_blocks_others_nor_holds_its_connection(self):
        port = self.server.server_port
        stalled = socket.create_connection(("127.0.0.1", port))
        self.addCleanup(stalled.close)
        stalled.sendall(b"POST /v1/detect HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n{")
        started = time.monotonic()
        with urllib.request.urlopen(self.url + "/health", timeout=5) as resp:
            self.assertEqual(resp.status, 200)
        self.assertLess(time.monotonic() - started, 1)
        stalled.settimeout(10)
        self.assertEqual(stalled.recv(1024), b"")  # closed by the server, not left open
        self.assertLess(time.monotonic() - started, 5)

    def test_slow_headers_are_dropped_at_a_total_deadline(self):
        stalled = socket.create_connection(("127.0.0.1", self.server.server_port))
        self.addCleanup(stalled.close)
        started = time.monotonic()
        stalled.settimeout(0.2)
        # One byte every 0.5 s never trips the 1 s per-read timeout; only the total deadline stops it.
        for byte in b"GET /v1/health HTTP/1.1\r\nX: " + b"a" * 100:
            try:
                stalled.sendall(bytes([byte]))
                if stalled.recv(1) == b"":
                    break
            except socket.timeout:
                pass
            except OSError:
                break
            time.sleep(0.5)
        self.assertLess(time.monotonic() - started, 6)

    def test_health_is_answered_while_every_inference_slot_is_busy(self):
        port = self.server.server_port
        for _ in range(server.MAX_CONNECTIONS):
            stalled = socket.create_connection(("127.0.0.1", port))
            self.addCleanup(stalled.close)
            stalled.sendall(
                b"POST /v1/detect HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n{"
            )
        time.sleep(0.2)  # let the accept loop take every connection
        started = time.monotonic()
        with urllib.request.urlopen(self.url + "/health", timeout=5) as resp:
            self.assertEqual(json.load(resp), {"model": "sam3", "ready": True})
        self.assertLess(time.monotonic() - started, 1, "health waited for a busy slot")

    def test_a_slow_health_probe_is_dropped_at_the_total_deadline(self):
        port = self.server.server_port
        for _ in range(server.MAX_CONNECTIONS):
            busy = socket.create_connection(("127.0.0.1", port))
            self.addCleanup(busy.close)
            busy.sendall(b"POST /v1/detect HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n{")
        time.sleep(0.2)
        probe = socket.create_connection(("127.0.0.1", port))
        self.addCleanup(probe.close)
        probe.settimeout(0.2)
        started = time.monotonic()
        # One byte every 0.5 s never trips the per-read timeout; the total deadline must.
        for byte in b"GET /v1/health HTTP/1.1\r\nX: " + b"a" * 100:
            try:
                probe.sendall(bytes([byte]))
                if probe.recv(1) == b"":
                    break
            except socket.timeout:
                pass
            except OSError:
                break
            time.sleep(0.5)
        self.assertLess(time.monotonic() - started, 6)

    def test_a_thread_that_cannot_start_gives_its_slot_back(self):
        from unittest import mock
        httpd = BridgeServer(("127.0.0.1", 0), handler_for(lambda raw, prompt: {}))
        self.addCleanup(httpd.server_close)
        with mock.patch("socketserver.ThreadingMixIn.process_request", side_effect=RuntimeError("no thread")):
            for _ in range(server.MAX_CONNECTIONS + 2):
                with self.assertRaises(RuntimeError):
                    httpd.process_request(None, ("127.0.0.1", 0))
        for _ in range(server.MAX_CONNECTIONS):
            self.assertTrue(httpd.slots.acquire(blocking=False))

    def test_only_one_prediction_runs_at_a_time(self):
        active, peak = 0, 0
        guard = threading.Lock()

        def slow(raw, prompt):
            nonlocal active, peak
            with guard:
                active += 1
                peak = max(peak, active)
            time.sleep(0.2)
            with guard:
                active -= 1
            return {"width": 1, "height": 1, "boxes": [], "scores": []}

        bridge = BridgeServer(("127.0.0.1", 0), handler_for(slow))
        thread = threading.Thread(target=bridge.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(bridge.server_close)
        self.addCleanup(bridge.shutdown)
        self.addCleanup(thread.join, 2)
        data = json.dumps(
            {"prompt": "x", "image": {"mime_type": "image/png",
                                        "data": base64.b64encode(b"i").decode()}}
        ).encode()
        url = "http://127.0.0.1:%s/v1/detect" % bridge.server_port
        statuses = []

        def call():
            with urllib.request.urlopen(urllib.request.Request(url, data=data)) as resp:
                statuses.append(resp.status)

        callers = [threading.Thread(target=call) for _ in range(2)]
        for caller in callers:
            caller.start()
        for caller in callers:
            caller.join(5)
        self.assertEqual(statuses, [200, 200])
        self.assertEqual(peak, 1, "two predictions ran at once")


if __name__ == "__main__":
    unittest.main()
