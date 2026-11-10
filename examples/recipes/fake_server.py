#!/usr/bin/env python3
"""A stand-in server for trying a recipe with no account: it prints every request and answers 200.

    python3 fake_server.py [PORT]         # default 9999, loopback only

Copy a recipe, change the scheme and host of each url to http://127.0.0.1:9999, add it in ClawDot's Tools
tab (or the configuration file) and use the tab's Test button. The request line, the headers (the
Authorization value is hidden) and the body are printed here, so you can compare them with the service's
documentation before any real account is involved.
"""

from __future__ import annotations

import http.server
import sys

ANSWER = b'{"fake":true,"note":"this is the stand-in server, not the real service"}'


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args) -> None:  # noqa: A002
        pass

    def handle_any(self) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length).decode(errors="replace") if length else ""
        print(f"{self.command} {self.path}")
        for name, value in self.headers.items():
            shown = "<hidden>" if name.lower() in ("authorization", "x-subscription-token") else value
            print(f"  {name}: {shown}")
        if body:
            print(f"  body: {body}")
        print(flush=True)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(ANSWER)))
        self.end_headers()
        self.wfile.write(ANSWER)

    do_GET = do_POST = do_PUT = do_DELETE = handle_any  # noqa: N815


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9999
    print(f"Listening on 127.0.0.1:{port}", flush=True)
    http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
