"""Loopback SAM 3 image detection bridge. No downloads or model installs at runtime.

Run with a licensed checkpoint: python server.py --checkpoint PATH_TO_sam3.pt
SAM 3's Python/CUDA dependencies must be installed in that Python environment.
Only boxes and scores are returned; masks/tracking are not advertised.
"""
import argparse
import base64
import binascii
import io
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

MAX_REQUEST = 24 * 1024 * 1024
MAX_IMAGE = 16 * 1024 * 1024
# A client that stalls is dropped: each read waits at most IDLE_SECONDS, and the whole body must
# arrive within BODY_SECONDS. At most MAX_CONNECTIONS are served at once; more are closed, except
# that a health check is read on one of MAX_PROBES extra slots so a busy bridge still answers it.
IDLE_SECONDS = 15
BODY_SECONDS = 60
# The request line and headers together must arrive within HEADER_SECONDS (a byte at a time does not help).
HEADER_SECONDS = 30
MAX_CONNECTIONS = 8
MAX_PROBES = 4
HEALTH_BODY = b'{"model":"sam3","ready":true}'
HEALTH_REPLY = (b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n"
                b"Content-Length: " + str(len(HEALTH_BODY)).encode("ascii") +
                b"\r\nConnection: close\r\n\r\n" + HEALTH_BODY)


def predict_with_sam3(checkpoint):
    from PIL import Image
    from sam3.model_builder import build_sam3_image_model
    from sam3.model.sam3_image_processor import Sam3Processor

    model = build_sam3_image_model(checkpoint_path=str(checkpoint), load_from_HF=False)
    processor = Sam3Processor(model)

    def predict(raw, prompt):
        image = Image.open(io.BytesIO(raw))
        image.load()
        if image.width * image.height > 36_000_000:
            raise ValueError("image dimensions exceed local detection limit")
        image = image.convert("RGB")
        state = processor.set_image(image)
        result = processor.set_text_prompt(state=state, prompt=prompt)
        boxes = result["boxes"].detach().cpu().tolist()
        scores = result["scores"].detach().cpu().tolist()
        if len(boxes) > 256:
            boxes, scores = boxes[:256], scores[:256]
        return {"width": image.width, "height": image.height,
                "boxes": boxes, "scores": scores}

    return predict


class BridgeServer(ThreadingHTTPServer):
    """One thread per connection, so a slow client cannot block the others, up to a fixed count."""

    daemon_threads = True

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.slots = threading.BoundedSemaphore(MAX_CONNECTIONS)
        self.probes = threading.BoundedSemaphore(MAX_PROBES)

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            # Every inference slot is busy. A health check is answered on one of the probe slots,
            # since reporting a busy bridge as unreachable helps nobody; anything else is closed
            # straight away, as before.
            if self.probes.acquire(blocking=False):
                try:
                    threading.Thread(target=self.answer_probe, args=(request,),
                                     daemon=True).start()
                except RuntimeError:
                    self.probes.release()
                    self.shutdown_request(request)
            else:
                self.shutdown_request(request)
            return
        super().process_request(request, client_address)

    def answer_probe(self, request):
        """Answers `GET /v1/health` on a connection taken while every inference slot is busy."""
        try:
            request.settimeout(IDLE_SECONDS)
            with request.makefile("rb") as reader:
                line = reader.readline(1024)
            parts = line.rstrip(b"\r\n").split(b" ")
            if len(parts) == 3 and parts[0] == b"GET" and parts[1] == b"/v1/health":
                request.sendall(HEALTH_REPLY)
        except OSError:
            pass
        finally:
            self.probes.release()
            self.shutdown_request(request)

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.slots.release()


class HeadReader(io.RawIOBase):
    """Socket reader that stops once the request line and headers have taken too long overall."""

    def __init__(self, sock):
        self.sock = sock
        self.deadline = time.monotonic() + HEADER_SECONDS

    def readable(self):
        return True

    def readinto(self, buf):
        if self.deadline is not None:
            remaining = self.deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("request head too slow")
            self.sock.settimeout(min(IDLE_SECONDS, remaining))
        return self.sock.recv_into(buf)


def read_body(rfile, size, deadline, sock):
    """Exactly `size` bytes, or None if the client stops sending or runs past `deadline`."""
    chunks, have = [], 0
    while have < size:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        try:
            # Each blocking read waits for at most what the body has left, so a read that starts
            # just before the deadline cannot run IDLE_SECONDS past it.
            sock.settimeout(min(IDLE_SECONDS, remaining))
            chunk = rfile.read1(min(size - have, 1 << 20))
        except OSError:
            return None
        if not chunk:
            return None
        chunks.append(chunk)
        have += len(chunk)
    return b"".join(chunks)


def handler_for(predict):
    # The model is not shared between threads: one prediction at a time.
    predicting = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        timeout = IDLE_SECONDS

        def setup(self):
            super().setup()
            self.rfile = io.BufferedReader(HeadReader(self.connection))

        def parse_request(self):
            ok = super().parse_request()
            self.rfile.raw.deadline = None  # the body has its own deadline (read_body)
            return ok

        def log_message(self, *_args):
            # Do not log prompts, image contents, or arbitrary request paths.
            pass

        def reply(self, status, data):
            raw = json.dumps(data, separators=(",", ":")).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self):
            if self.path == "/v1/health":
                self.reply(200, {"model": "sam3", "ready": True})
            else:
                self.reply(404, {"error": "not found"})

        def do_POST(self):
            if self.path != "/v1/detect":
                self.reply(404, {"error": "not found"})
                return
            try:
                size = int(self.headers.get("Content-Length", "-1"))
                if not 0 < size <= MAX_REQUEST:
                    self.reply(413, {"error": "request too large"})
                    return
                raw_body = read_body(
                    self.rfile, size, time.monotonic() + BODY_SECONDS, self.connection
                )
                if raw_body is None:
                    self.close_connection = True
                    return
                body = json.loads(raw_body)
                prompt = body["prompt"]
                image = body["image"]
                if not isinstance(prompt, str) or not 0 < len(prompt) <= 4096:
                    raise ValueError("invalid prompt")
                if image["mime_type"] not in ("image/png", "image/jpeg", "image/webp"):
                    raise ValueError("unsupported image type")
                raw = base64.b64decode(image["data"], validate=True)
                if not 0 < len(raw) <= MAX_IMAGE:
                    raise ValueError("image too large")
            except (KeyError, TypeError, ValueError, json.JSONDecodeError, binascii.Error):
                self.reply(400, {"error": "invalid detection request"})
                return
            try:
                with predicting:
                    result = predict(raw, prompt)
                self.reply(200, result)
            except Exception:
                # Detailed exceptions can contain paths or image data; do not reflect them.
                self.reply(500, {"error": "SAM 3 inference failed"})

    return Handler


def main():
    ap = argparse.ArgumentParser(description="SAM 3 local object-detection bridge")
    ap.add_argument("--checkpoint", type=Path, required=True)
    ap.add_argument("--port", type=int, default=8343)
    args = ap.parse_args()
    if not args.checkpoint.is_file():
        ap.error("checkpoint not found; request access to facebook/sam3 and download it yourself")
    if not 1 <= args.port <= 65535:
        ap.error("port must be 1..65535")
    backend = predict_with_sam3(args.checkpoint)
    server = BridgeServer(("127.0.0.1", args.port), handler_for(backend))
    print(f"SAM 3 bridge ready at http://127.0.0.1:{args.port}/v1", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
