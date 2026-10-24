#!/usr/bin/env python3
"""Serve the Vox test guide on this computer and keep your feedback on disk.

Usage:
    python3 docs/manual-testing/serve.py [port]

Then open the address it prints (default http://127.0.0.1:8765/).

What it does:
  * serves experimental-features.html
  * GET  /api/state  returns your saved feedback (feedback.json next to this file)
  * POST /api/state  saves your feedback. The previous copy is kept as
                     feedback.prev.json, and a timestamped copy goes into
                     feedback-history/ at most every 10 minutes (last 30 kept).

It listens on 127.0.0.1 only, so nothing outside this computer can reach it.
Needs only the Python standard library.
"""
import http.server
import json
import os
import shutil
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
PAGE = os.path.join(HERE, "experimental-features.html")
STATE = os.path.join(HERE, "feedback.json")
PREV = os.path.join(HERE, "feedback.prev.json")
HISTORY = os.path.join(HERE, "feedback-history")
HISTORY_EVERY_SECONDS = 600
HISTORY_KEEP = 30
MAX_BYTES = 5_000_000
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765


def keep_history():
    os.makedirs(HISTORY, exist_ok=True)
    files = sorted(f for f in os.listdir(HISTORY) if f.startswith("feedback-") and f.endswith(".json"))
    if files:
        newest = os.path.getmtime(os.path.join(HISTORY, files[-1]))
        if time.time() - newest < HISTORY_EVERY_SECONDS:
            return
    stamp = time.strftime("%Y%m%d-%H%M%S")
    shutil.copyfile(STATE, os.path.join(HISTORY, f"feedback-{stamp}.json"))
    files = sorted(f for f in os.listdir(HISTORY) if f.startswith("feedback-") and f.endswith(".json"))
    for old in files[:-HISTORY_KEEP]:
        os.remove(os.path.join(HISTORY, old))


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "VoxGuide/1"

    def log_message(self, fmt, *args):  # keep the terminal quiet
        pass

    def _host_ok(self):
        host = (self.headers.get("Host") or "").rsplit(":", 1)[0]
        return host in ("127.0.0.1", "localhost")

    def _origin_ok(self):
        origin = self.headers.get("Origin")
        if not origin:
            return True
        return origin in (f"http://127.0.0.1:{PORT}", f"http://localhost:{PORT}")

    def _send(self, code, body=b"", ctype="application/json"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if not self._host_ok():
            return self._send(403, b"{}")
        path = self.path.split("?", 1)[0]
        if path in ("/", "/index.html", "/experimental-features.html"):
            with open(PAGE, "rb") as f:
                return self._send(200, f.read(), "text/html; charset=utf-8")
        if path == "/api/state":
            if os.path.exists(STATE):
                with open(STATE, "rb") as f:
                    return self._send(200, f.read())
            return self._send(200, b"{}")
        self._send(404, b"{}")

    def do_POST(self):
        if not self._host_ok() or not self._origin_ok():
            return self._send(403, b"{}")
        if self.path.split("?", 1)[0] != "/api/state":
            return self._send(404, b"{}")
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            return self._send(400, b"{}")
        if length <= 0 or length > MAX_BYTES:
            return self._send(413, b"{}")
        raw = self.rfile.read(length)
        try:
            data = json.loads(raw)
            if not isinstance(data, dict) or not isinstance(data.get("tests"), dict):
                raise ValueError("not a progress state")
        except ValueError:
            return self._send(400, b"{}")
        # Keep the previous copy, plus a timestamped history copy now and then, so a
        # single bad save (for example from an empty page) can never destroy feedback.
        if os.path.exists(STATE):
            keep_history()
            os.replace(STATE, PREV)
        tmp = STATE + ".tmp"
        with open(tmp, "wb") as f:
            f.write(raw)
        os.replace(tmp, STATE)
        self._send(200, b'{"ok":true}')


def main():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"Vox test guide: http://127.0.0.1:{PORT}/")
    print(f"Your feedback is saved to: {STATE}")
    print("Press Ctrl+C to stop.")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
