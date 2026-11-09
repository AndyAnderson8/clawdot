#!/usr/bin/env python3
"""Keeps a Spotify access token fresh for ClawDot's HTTP tools. Runs outside the core.

ClawDot's HTTP tools send a fixed bearer token read from an environment variable at start-up, and a Spotify
access token lasts one hour. This small program holds the long-lived refresh token, refreshes the access
token itself, and forwards a short list of Spotify Web API calls. ClawDot talks to it on the loopback address
with a shared key (SPOTIFY_PROXY_KEY), so the Spotify credentials never reach the core or its configuration.

    python3 spotify_token_proxy.py login --client-id ID          # once: authorise in a browser (PKCE)
    SPOTIFY_PROXY_KEY=... python3 spotify_token_proxy.py serve --client-id ID

Read docs/recipes/spotify.md first: Spotify's Developer Policy restricts voice-controlled apps.
Python 3 standard library only. Spotify documentation used: the PKCE flow and refresh-token pages.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import http.server
import json
import os
import secrets
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import webbrowser
from pathlib import Path

ACCOUNTS_URL = os.environ.get("SPOTIFY_ACCOUNTS_URL", "https://accounts.spotify.com")
API_URL = os.environ.get("SPOTIFY_API_URL", "https://api.spotify.com")
SCOPES = "user-read-playback-state user-modify-playback-state user-read-currently-playing"
DEFAULT_TOKEN_FILE = Path.home() / ".config" / "clawdot" / "spotify-refresh.json"
MAX_BODY = 4096
TIMEOUT = 10

# The only calls that are forwarded: (method, path). Everything else is refused.
ALLOWED = {
    ("GET", "/v1/me/player/currently-playing"),
    ("GET", "/v1/me/player/devices"),
    ("GET", "/v1/search"),
    ("PUT", "/v1/me/player/play"),
    ("PUT", "/v1/me/player/pause"),
    ("POST", "/v1/me/player/next"),
    ("POST", "/v1/me/player/previous"),
    ("PUT", "/v1/me/player/volume"),
}


def pkce_pair() -> tuple[str, str]:
    """A code verifier and its S256 challenge."""
    verifier = base64.urlsafe_b64encode(secrets.token_bytes(48)).rstrip(b"=").decode()
    digest = hashlib.sha256(verifier.encode()).digest()
    return verifier, base64.urlsafe_b64encode(digest).rstrip(b"=").decode()


def token_request(params: dict[str, str]) -> dict:
    """POST to the token endpoint and return its JSON."""
    data = urllib.parse.urlencode(params).encode()
    req = urllib.request.Request(  # noqa: S310 - the URL is a fixed https endpoint (or a test server)
        f"{ACCOUNTS_URL}/api/token",
        data=data,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:  # noqa: S310
        return json.load(resp)


def save_token_file(path: Path, refresh_token: str) -> None:
    """Write the refresh token readable by the owner only."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump({"refresh_token": refresh_token}, f)


class Tokens:
    """The access token, refreshed when it is about to expire."""

    def __init__(self, client_id: str, token_file: Path, clock=time.monotonic) -> None:
        self.client_id = client_id
        self.token_file = token_file
        self.clock = clock
        self.access = ""
        self.expires = 0.0
        self.lock = threading.Lock()

    def get(self) -> str:
        with self.lock:
            if self.access and self.clock() < self.expires - 60:
                return self.access
            refresh = json.loads(self.token_file.read_text())["refresh_token"]
            reply = token_request(
                {"grant_type": "refresh_token", "refresh_token": refresh, "client_id": self.client_id}
            )
            self.access = reply["access_token"]
            self.expires = self.clock() + int(reply.get("expires_in", 3600))
            # Spotify may rotate the refresh token; keep the new one when it does.
            if reply.get("refresh_token") and reply["refresh_token"] != refresh:
                save_token_file(self.token_file, reply["refresh_token"])
            return self.access


def rewrite_play_body(body: bytes) -> bytes:
    """ClawDot sends {"uri": ...}; Spotify wants context_uri for albums, artists and playlists, uris for tracks."""
    try:
        data = json.loads(body or b"{}")
        uri = data.get("uri")
    except (ValueError, AttributeError):
        return body
    if not isinstance(uri, str) or not uri.startswith("spotify:"):
        return body
    if uri.startswith("spotify:track:"):
        return json.dumps({"uris": [uri]}).encode()
    return json.dumps({"context_uri": uri}).encode()


def make_handler(tokens: Tokens, key: str) -> type[http.server.BaseHTTPRequestHandler]:
    class Handler(http.server.BaseHTTPRequestHandler):
        server_version = "clawdot-spotify-proxy"

        def log_message(self, fmt: str, *args) -> None:  # noqa: A002 - signature of the base class
            # Paths only: never headers, never tokens.
            sys.stderr.write(f"{self.command} {urllib.parse.urlsplit(self.path).path} -> {fmt % args}\n")

        def reply(self, status: int, body: bytes = b"", ctype: str = "application/json") -> None:
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def handle_any(self) -> None:
            auth = self.headers.get("Authorization", "")
            if not hmac.compare_digest(auth.encode(), f"Bearer {key}".encode()):
                return self.reply(401, b'{"error":"unauthorized"}')
            parts = urllib.parse.urlsplit(self.path)
            if (self.command, parts.path) not in ALLOWED:
                return self.reply(404, b'{"error":"that call is not allowed"}')
            length = int(self.headers.get("Content-Length") or 0)
            if length > MAX_BODY:
                return self.reply(413, b'{"error":"body too large"}')
            body = self.rfile.read(length) if length else b""
            if parts.path == "/v1/me/player/play":
                body = rewrite_play_body(body)
            try:
                token = tokens.get()
            except (OSError, ValueError, KeyError, urllib.error.URLError) as e:
                sys.stderr.write(f"token refresh failed: {type(e).__name__}\n")
                return self.reply(502, b'{"error":"could not refresh the Spotify token; run login again"}')
            url = f"{API_URL}{parts.path}" + (f"?{parts.query}" if parts.query else "")
            req = urllib.request.Request(  # noqa: S310
                url, data=body if self.command != "GET" else None, method=self.command
            )
            req.add_header("Authorization", f"Bearer {token}")
            if body:
                req.add_header("Content-Type", "application/json")
            try:
                with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:  # noqa: S310
                    self.reply(resp.status, resp.read(65536))
            except urllib.error.HTTPError as e:
                self.reply(e.code, e.read(65536))
            except (urllib.error.URLError, TimeoutError):
                self.reply(502, b'{"error":"Spotify did not answer"}')

        do_GET = do_PUT = do_POST = do_DELETE = handle_any  # noqa: N815

    return Handler


def serve(args: argparse.Namespace) -> int:
    key = os.environ.get("SPOTIFY_PROXY_KEY", "")
    if len(key) < 16:
        sys.stderr.write("Set SPOTIFY_PROXY_KEY to a random string of at least 16 characters.\n")
        return 2
    host, _, port = args.listen.rpartition(":")
    if host not in ("127.0.0.1", "::1", "localhost"):
        sys.stderr.write("Refusing to listen beyond the loopback address.\n")
        return 2
    tokens = Tokens(args.client_id, Path(args.token_file))
    httpd = http.server.ThreadingHTTPServer((host, int(port)), make_handler(tokens, key))
    sys.stderr.write(f"Listening on {args.listen}\n")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


def login(args: argparse.Namespace) -> int:
    """The PKCE authorisation-code flow with a loopback redirect (http://127.0.0.1:PORT/callback)."""
    verifier, challenge = pkce_pair()
    state = secrets.token_urlsafe(16)
    redirect = f"http://127.0.0.1:{args.port}/callback"
    result: dict[str, str] = {}

    class Callback(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            q = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(self.path).query))
            if q.get("state") == state and "code" in q:
                result["code"] = q["code"]
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"You can close this window and return to the terminal.")

        def log_message(self, fmt: str, *a) -> None:  # noqa: A002
            pass

    url = f"{ACCOUNTS_URL}/authorize?" + urllib.parse.urlencode(
        {
            "client_id": args.client_id,
            "response_type": "code",
            "redirect_uri": redirect,
            "scope": SCOPES,
            "state": state,
            "code_challenge_method": "S256",
            "code_challenge": challenge,
        }
    )
    httpd = http.server.HTTPServer(("127.0.0.1", args.port), Callback)
    print(f"Open this address, sign in and allow access:\n\n{url}\n")
    if not args.no_browser:
        webbrowser.open(url)
    while "code" not in result:
        httpd.handle_request()
    reply = token_request(
        {
            "grant_type": "authorization_code",
            "code": result["code"],
            "redirect_uri": redirect,
            "client_id": args.client_id,
            "code_verifier": verifier,
        }
    )
    save_token_file(Path(args.token_file), reply["refresh_token"])
    print(f"Saved the refresh token to {args.token_file} (readable by you only).")
    return 0


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("login", "serve"):
        s = sub.add_parser(name)
        s.add_argument("--client-id", required=True, help="Your Spotify app's client id (not a secret)")
        s.add_argument("--token-file", default=str(DEFAULT_TOKEN_FILE))
    sub.choices["login"].add_argument("--port", type=int, default=8888, help="Loopback port of the redirect URI")
    sub.choices["login"].add_argument("--no-browser", action="store_true")
    sub.choices["serve"].add_argument("--listen", default="127.0.0.1:8890")
    args = p.parse_args(argv)
    return login(args) if args.cmd == "login" else serve(args)


if __name__ == "__main__":
    sys.exit(main())
