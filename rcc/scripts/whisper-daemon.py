#!/usr/bin/env python3
"""
whisper-daemon.py — Keep-warm Whisper HTTP daemon for SquirrelVoice STT
Loads the model once and serves transcription requests via HTTP on /transcribe.

Usage:
  python3 whisper-daemon.py [--model base] [--port 9876] [--language en]

API:
  POST /transcribe
    Content-Type: multipart/form-data
    Field: audio (audio file bytes)
    Returns: {"text": "transcribed text", "latency_ms": 234}

  GET /health
    Returns: {"ok": true, "model": "base", "warm": true}

Grounded in: wq-NAT-idea-20260404-WHISPER-REAL-SPEECH benchmark results
  - base model: 4.4s cold load + 3.4s inference → 8s total cold start
  - warm inference: ~3.5s (acceptable for SquirrelVoice UX)
  - Recommendation: keep model loaded, --language en to skip detection
"""

import argparse
import io
import json
import os
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

try:
    import whisper
except ImportError:
    print("ERROR: openai-whisper not installed. Run: pip install openai-whisper", file=sys.stderr)
    sys.exit(1)

# Global model (loaded once at startup)
_model = None
_model_name = "base"
_language = "en"
_start_time = time.time()
_request_count = 0

MULTIPART_BOUNDARY_PREFIX = b"--"


def load_model(name: str) -> None:
    global _model, _model_name
    print(f"[whisper-daemon] Loading model '{name}'...", flush=True)
    t0 = time.time()
    _model = whisper.load_model(name)
    _model_name = name
    elapsed = time.time() - t0
    print(f"[whisper-daemon] Model loaded in {elapsed:.1f}s. Serving on warm path.", flush=True)


def parse_multipart(data: bytes, content_type: str) -> bytes | None:
    """Extract the first file field from a multipart/form-data payload."""
    # Extract boundary from Content-Type header
    boundary = None
    for part in content_type.split(";"):
        part = part.strip()
        if part.startswith("boundary="):
            boundary = part[len("boundary="):].strip('"')
            break
    if not boundary:
        return None

    delimiter = f"--{boundary}".encode()
    end_delimiter = f"--{boundary}--".encode()

    # Split on delimiter
    parts = data.split(delimiter)
    for part in parts:
        if not part or part == b"--\r\n" or part.startswith(b"--"):
            continue
        # part starts with \r\n, then headers, then \r\n\r\n, then body
        if b"\r\n\r\n" not in part:
            continue
        headers_raw, body = part.split(b"\r\n\r\n", 1)
        # Strip trailing \r\n from body
        if body.endswith(b"\r\n"):
            body = body[:-2]
        # Only return if this looks like a file (Content-Disposition: form-data; name=...; filename=...)
        if b"filename=" in headers_raw or b"audio" in headers_raw.lower():
            return body
    return None


def transcribe_audio(audio_bytes: bytes) -> dict:
    """Write audio to a temp file and transcribe."""
    global _request_count
    _request_count += 1

    # Detect format from magic bytes
    if audio_bytes[:4] == b"RIFF":
        suffix = ".wav"
    elif audio_bytes[:3] == b"ID3" or audio_bytes[:2] == b"\xff\xfb":
        suffix = ".mp3"
    elif audio_bytes[:4] == b"OggS":
        suffix = ".ogg"
    elif audio_bytes[:4] in (b"fLaC", b"FLAC"):
        suffix = ".flac"
    else:
        suffix = ".wav"  # default

    t0 = time.time()
    with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as f:
        f.write(audio_bytes)
        tmp_path = f.name

    try:
        result = _model.transcribe(
            tmp_path,
            language=_language,
            fp16=False,  # GB10 aarch64 may not support fp16 in all modes
        )
        text = result.get("text", "").strip()
    finally:
        os.unlink(tmp_path)

    elapsed_ms = int((time.time() - t0) * 1000)
    return {"text": text, "latency_ms": elapsed_ms, "model": _model_name}


class RequestHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        # Suppress default access log noise; print our own
        pass

    def send_json(self, code: int, data: dict):
        body = json.dumps(data).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path in ("/health", "/v1/audio/health"):
            self.send_json(200, {
                "ok": True,
                "model": _model_name,
                "language": _language,
                "warm": _model is not None,
                "uptime_s": int(time.time() - _start_time),
                "requests_served": _request_count,
            })
        else:
            self.send_json(404, {"error": "not found"})

    def do_POST(self):
        if self.path not in ("/transcribe", "/v1/audio/transcriptions"):
            self.send_json(404, {"error": "not found"})
            return

        if _model is None:
            self.send_json(503, {"error": "model not loaded yet"})
            return

        content_length = int(self.headers.get("Content-Length", 0))
        if content_length == 0:
            self.send_json(400, {"error": "empty request body"})
            return

        body = self.rfile.read(content_length)
        content_type = self.headers.get("Content-Type", "")

        # Extract audio bytes
        if "multipart/form-data" in content_type:
            audio_bytes = parse_multipart(body, content_type)
            if audio_bytes is None:
                self.send_json(400, {"error": "no audio field in multipart body"})
                return
        else:
            # Treat raw body as audio bytes
            audio_bytes = body

        if len(audio_bytes) < 100:
            self.send_json(400, {"error": "audio payload too small"})
            return

        try:
            result = transcribe_audio(audio_bytes)
            print(f"[whisper-daemon] transcribed {len(audio_bytes)} bytes → {len(result['text'])} chars in {result['latency_ms']}ms", flush=True)
            self.send_json(200, result)
        except Exception as e:
            print(f"[whisper-daemon] transcription error: {e}", file=sys.stderr, flush=True)
            self.send_json(500, {"error": str(e)})


def main():
    parser = argparse.ArgumentParser(description="Whisper keep-warm HTTP daemon")
    parser.add_argument("--model", default="base", help="Whisper model size (tiny/base/small/medium/large)")
    parser.add_argument("--port", type=int, default=9876, help="HTTP port to listen on")
    parser.add_argument("--language", default="en", help="Language hint (skip detection for speed)")
    parser.add_argument("--model-dir", default=None, help="Directory to cache models (default: ~/.cache/whisper)")
    args = parser.parse_args()

    global _language
    _language = args.language

    if args.model_dir:
        os.environ["WHISPER_CACHE"] = args.model_dir

    # Load model synchronously before accepting requests
    load_model(args.model)

    server = HTTPServer(("0.0.0.0", args.port), RequestHandler)
    print(f"[whisper-daemon] Listening on port {args.port}", flush=True)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n[whisper-daemon] Shutting down.", flush=True)


if __name__ == "__main__":
    main()
