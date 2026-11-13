"""Loopback SAM 3 image detection bridge. No downloads or model installs at runtime.

Run with a licensed checkpoint: python server.py --checkpoint PATH_TO_sam3.pt
Install `requirements.txt` beside this file, plus SAM 3's own Python/CUDA dependencies, in
that Python environment. Only boxes and scores are returned; masks/tracking are not advertised.

Who may call it: only a local program that names this server as `127.0.0.1:<port>` or
`localhost:<port>` and sends a JSON body. Each guard below says what it prevents:
- No `Origin` header: browsers add one to cross-site requests, so a web page cannot use it.
- `Host` must be this server: a page that rebinds its own domain name to 127.0.0.1 still
  sends that domain as the Host, so DNS rebinding cannot read replies.
- `Content-Type: application/json`: a page can send text/plain or a form without asking the
  browser first, but not JSON.
- Only PNG, JPEG and WebP decoders, matched to the declared type, with the pixel limit checked
  before decoding: other Pillow formats (EPS starts Ghostscript) are never reached.
"""
import argparse
import base64
import binascii
import io
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

MAX_REQUEST = 24 * 1024 * 1024
MAX_IMAGE = 16 * 1024 * 1024
MAX_PIXELS = 36_000_000
PIL_FORMATS = {"image/png": "PNG", "image/jpeg": "JPEG", "image/webp": "WEBP"}


class ImageRejected(ValueError):
    """The bytes are not a usable picture of the declared type."""


def _check_pixels(width, height):
    if width <= 0 or height <= 0 or width * height > MAX_PIXELS:
        raise ImageRejected("image dimensions exceed local detection limit")


def _webp_size(raw):
    """Canvas width and height from a WebP file's first chunk, without decoding anything.

    Layout from the WebP container specification: a RIFF header, then VP8 (lossy), VP8L
    (lossless) or VP8X (extended, which gives the canvas of every frame). Anything else is
    not a WebP picture this bridge accepts.
    """
    if len(raw) < 30 or raw[0:4] != b"RIFF" or raw[8:12] != b"WEBP":
        raise ImageRejected("image does not match its declared type")
    chunk = raw[12:16]
    if chunk == b"VP8X":
        width = 1 + int.from_bytes(raw[24:27], "little")
        height = 1 + int.from_bytes(raw[27:30], "little")
    elif chunk == b"VP8 ":
        if raw[23:26] != b"\x9d\x01\x2a":
            raise ImageRejected("image does not match its declared type")
        width = int.from_bytes(raw[26:28], "little") & 0x3FFF
        height = int.from_bytes(raw[28:30], "little") & 0x3FFF
    elif chunk == b"VP8L":
        if raw[20] != 0x2F:
            raise ImageRejected("image does not match its declared type")
        bits = int.from_bytes(raw[21:25], "little")
        width = 1 + (bits & 0x3FFF)
        height = 1 + ((bits >> 14) & 0x3FFF)
    else:
        raise ImageRejected("image does not match its declared type")
    return width, height


def open_image(raw, mime_type):
    """Decode `raw` as exactly the declared type, or raise ImageRejected."""
    from PIL import Image, UnidentifiedImageError

    expected = PIL_FORMATS.get(mime_type)
    if expected is None:
        raise ImageRejected("unsupported image type")
    if expected == "WEBP":
        # Pillow builds the WebP decoder and its canvas inside Image.open, so the size is read
        # from the header first. PNG and JPEG open lazily and are checked below.
        _check_pixels(*_webp_size(raw))
    try:
        image = Image.open(io.BytesIO(raw), formats=[expected])
    except (UnidentifiedImageError, OSError, SyntaxError, Image.DecompressionBombError) as err:
        raise ImageRejected("image does not match its declared type") from err
    if image.format != expected:
        raise ImageRejected("image does not match its declared type")
    _check_pixels(*image.size)
    try:
        image.load()
    except (OSError, SyntaxError, Image.DecompressionBombError) as err:
        raise ImageRejected("image could not be decoded") from err
    return image


def predict_with_sam3(checkpoint):
    from sam3.model_builder import build_sam3_image_model
    from sam3.model.sam3_image_processor import Sam3Processor

    model = build_sam3_image_model(checkpoint_path=str(checkpoint), load_from_HF=False)
    processor = Sam3Processor(model)

    def predict(raw, prompt, mime_type):
        image = open_image(raw, mime_type).convert("RGB")
        state = processor.set_image(image)
        result = processor.set_text_prompt(state=state, prompt=prompt)
        boxes = result["boxes"].detach().cpu().tolist()
        scores = result["scores"].detach().cpu().tolist()
        if len(boxes) > 256:
            boxes, scores = boxes[:256], scores[:256]
        return {"width": image.width, "height": image.height,
                "boxes": boxes, "scores": scores}

    return predict


def _host_allowed(values, port):
    """True only for one Host header naming this server by loopback address and port."""
    if len(values) != 1:
        return False
    value = values[0].strip().lower()
    name, sep, given_port = value.rpartition(":")
    if not sep:
        # Clients leave out the default port, so a bare name is this server only on port 80.
        name, given_port = value, "80"
    return name in ("127.0.0.1", "localhost") and given_port == str(port)


def _is_json(values):
    if len(values) != 1:
        return False
    return values[0].split(";", 1)[0].strip().lower() == "application/json"


def handler_for(predict):
    class Handler(BaseHTTPRequestHandler):
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

        def caller_allowed(self):
            """Refuse browsers and foreign host names before any other work."""
            if self.headers.get_all("Origin") or not _host_allowed(
                    self.headers.get_all("Host") or [], self.server.server_address[1]):
                self.close_connection = True
                self.reply(403, {"error": "forbidden"})
                return False
            return True

        def do_GET(self):
            if not self.caller_allowed():
                return
            if self.path == "/v1/health":
                self.reply(200, {"model": "sam3", "ready": True})
            else:
                self.reply(404, {"error": "not found"})

        def do_POST(self):
            if not self.caller_allowed():
                return
            if self.path != "/v1/detect":
                self.reply(404, {"error": "not found"})
                return
            if not _is_json(self.headers.get_all("Content-Type") or []):
                self.close_connection = True
                self.reply(415, {"error": "expected application/json"})
                return
            try:
                size = int(self.headers.get("Content-Length", "-1"))
                if not 0 < size <= MAX_REQUEST:
                    self.close_connection = True
                    self.reply(413, {"error": "request too large"})
                    return
                body = json.loads(self.rfile.read(size))
                prompt = body["prompt"]
                image = body["image"]
                mime_type = image["mime_type"]
                if not isinstance(prompt, str) or not 0 < len(prompt) <= 4096:
                    raise ValueError("invalid prompt")
                if mime_type not in PIL_FORMATS:
                    raise ValueError("unsupported image type")
                raw = base64.b64decode(image["data"], validate=True)
                if not 0 < len(raw) <= MAX_IMAGE:
                    raise ValueError("image too large")
            except (KeyError, TypeError, ValueError, json.JSONDecodeError, binascii.Error):
                self.reply(400, {"error": "invalid detection request"})
                return
            try:
                self.reply(200, predict(raw, prompt, mime_type))
            except ImageRejected:
                self.reply(400, {"error": "invalid detection request"})
            except Exception:
                # Detailed exceptions can contain paths or image data; do not reflect them.
                self.reply(500, {"error": "SAM 3 inference failed"})

    return Handler


def make_server(port, handler):
    """Loopback only."""
    return HTTPServer(("127.0.0.1", port), handler)


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
    server = make_server(args.port, handler_for(backend))
    print(f"SAM 3 bridge ready at http://127.0.0.1:{args.port}/v1", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
