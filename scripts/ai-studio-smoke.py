"""Exercise the packaged Studio AI workflow using only a local mock provider."""
import base64
import contextlib
import http.server
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import zlib

binary = str(Path(sys.argv[1]).resolve())

def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(b"\0\x20\x80\xc0\xff")) + chunk(b"IEND", b"")

class Provider(http.server.BaseHTTPRequestHandler):
    calls = 0
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        type(self).calls += 1
        body = json.dumps({"data": [{"b64_json": base64.b64encode(png).decode()}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass

with tempfile.TemporaryDirectory(prefix="pentool-ai-smoke-") as tmp:
    doc = Path(tmp) / "smoke.pen"
    env = dict(os.environ, PENTOOL_AI_CONFIG=str(Path(tmp) / "ai.json"), PENTOOL_AI_SMOKE_KEY="test-only")
    env.pop("PENTOOL_AI", None)
    subprocess.run([binary, "new", str(doc)], env=env, check=True, stdout=subprocess.DEVNULL)
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider) as provider:
        threading.Thread(target=provider.serve_forever, daemon=True).start()
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        server = subprocess.Popen([binary, "serve", str(doc), "--port", str(port)], env=env, stdout=subprocess.DEVNULL)
        base = f"http://127.0.0.1:{port}"
        def request(path, body=None):
            data = None if body is None else json.dumps(body).encode()
            req = urllib.request.Request(base + path, data=data, headers={"Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(req, timeout=15) as response:
                    return response.read()
            except urllib.error.HTTPError as error:
                raise RuntimeError(f"{path}: {error.read().decode()}") from error
        def call(path, body=None):
            return json.loads(request(path, body))
        try:
            for _ in range(100):
                try:
                    request("/api/health")
                    break
                except (urllib.error.URLError, ConnectionError):
                    time.sleep(0.1)
            call("/api/ai/connect", {"name": "mock", "adapter": "openai-compatible", "endpoint": f"http://127.0.0.1:{provider.server_port}/v1", "credential_env": "PENTOOL_AI_SMOKE_KEY", "default_model": "mock-image"})
            original = doc.read_bytes()
            body = {"name": "hero", "provider": "mock", "prompt": "blue square"}
            plan = call("/api/ai/generate", dict(body, dry_run=True))
            assert Provider.calls == 0 and plan["contacted_provider"] is False
            run = call("/api/ai/generate", dict(body, allow_model_call=True, reviewed_disclosure=plan["disclosure"]))["run"]
            assert Provider.calls == 1 and doc.read_bytes() == original
            assert request(f"/api/ai/runs/{run}/candidates/1").startswith(b"\x89PNG")
            revision = call("/api/document")["revision"]
            call(f"/api/ai/runs/{run}/accept", {"candidate": 1, "id": "hero", "if_revision": revision})
            assert doc.read_bytes() != original
            call("/api/undo", {})
            assert doc.read_bytes() == original
            call(f"/api/ai/runs/{run}/discard", {})
            assert call("/api/ai/runs")["runs"] == []
            print("Studio AI smoke passed (local mock; no external model calls)")
        finally:
            server.terminate()
            server.wait(timeout=10)
            provider.shutdown()
