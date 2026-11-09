"""Tests for spotify_token_proxy.py against fake Spotify servers on the loopback address. No network."""

from __future__ import annotations

import http.server
import json
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path

import spotify_token_proxy as proxy

KEY = "k" * 20


class Fake(http.server.BaseHTTPRequestHandler):
    """Plays both the token endpoint and the Web API, and records what it was sent."""

    seen: list[dict] = []
    refreshes = 0

    def log_message(self, fmt, *args):  # noqa: A002
        pass

    def handle_any(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length).decode() if length else ""
        Fake.seen.append(
            {"method": self.command, "path": self.path, "auth": self.headers.get("Authorization"), "body": body}
        )
        if self.path == "/api/token":
            Fake.refreshes += 1
            out = json.dumps({"access_token": f"access-{Fake.refreshes}", "expires_in": 3600}).encode()
            self.send_response(200)
        else:
            out = b'{"ok":true}'
            self.send_response(204 if "/next" in self.path else 200)
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        if self.command != "POST" or self.path == "/api/token":
            self.wfile.write(out)

    do_GET = do_PUT = do_POST = handle_any


def serve(handler) -> tuple[http.server.HTTPServer, str]:
    httpd = http.server.HTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd, f"http://127.0.0.1:{httpd.server_address[1]}"


class ProxyTest(unittest.TestCase):
    def setUp(self):
        Fake.seen = []
        Fake.refreshes = 0
        self.fake, base = serve(Fake)
        self.old = (proxy.ACCOUNTS_URL, proxy.API_URL)
        proxy.ACCOUNTS_URL = proxy.API_URL = base
        self.dir = tempfile.TemporaryDirectory()
        self.token_file = Path(self.dir.name) / "t.json"
        proxy.save_token_file(self.token_file, "refresh-1")
        self.now = [1000.0]
        tokens = proxy.Tokens("cid", self.token_file, clock=lambda: self.now[0])
        self.front, self.url = serve(proxy.make_handler(tokens, KEY))

    def tearDown(self):
        self.fake.shutdown()
        self.front.shutdown()
        proxy.ACCOUNTS_URL, proxy.API_URL = self.old
        self.dir.cleanup()

    def call(self, method, path, key=KEY, body=None):
        req = urllib.request.Request(self.url + path, data=body, method=method)
        if key is not None:
            req.add_header("Authorization", f"Bearer {key}")
        try:
            with urllib.request.urlopen(req, timeout=5) as r:
                return r.status, r.read()
        except urllib.error.HTTPError as e:
            body = e.read()
            e.close()
            return e.code, body

    def api_calls(self):
        return [s for s in Fake.seen if s["path"] != "/api/token"]

    def test_the_shared_key_is_required_and_nothing_reaches_spotify_without_it(self):
        self.assertEqual(self.call("GET", "/v1/me/player/devices", key=None)[0], 401)
        self.assertEqual(self.call("GET", "/v1/me/player/devices", key="wrong")[0], 401)
        self.assertEqual(Fake.seen, [])

    def test_only_the_listed_calls_are_forwarded(self):
        for method, path in [
            ("GET", "/v1/me"),
            ("DELETE", "/v1/me/player/play"),
            ("POST", "/v1/me/playlists"),
            ("PUT", "/v1/me/player"),
            ("GET", "/v1/me/player/queue"),
        ]:
            self.assertEqual(self.call(method, path)[0], 404, (method, path))
        self.assertEqual(Fake.seen, [])

    def test_the_access_token_is_refreshed_once_and_reused_until_it_nearly_expires(self):
        self.assertEqual(self.call("GET", "/v1/me/player/devices")[0], 200)
        self.assertEqual(self.call("GET", "/v1/me/player/currently-playing")[0], 200)
        self.assertEqual(Fake.refreshes, 1)
        self.assertEqual({s["auth"] for s in self.api_calls()}, {"Bearer access-1"})
        refresh = next(s for s in Fake.seen if s["path"] == "/api/token")
        self.assertIn("grant_type=refresh_token", refresh["body"])
        self.assertIn("refresh_token=refresh-1", refresh["body"])
        self.assertNotIn("client_secret", refresh["body"])
        self.now[0] += 3600 - 30  # inside the one-minute margin
        self.call("GET", "/v1/me/player/devices")
        self.assertEqual(Fake.refreshes, 2)
        self.assertEqual(self.api_calls()[-1]["auth"], "Bearer access-2")

    def test_the_query_string_is_passed_through_and_a_body_less_post_works(self):
        self.assertEqual(self.call("POST", "/v1/me/player/next?device_id=abc", body=b"")[0], 204)
        self.assertEqual(self.api_calls()[-1]["path"], "/v1/me/player/next?device_id=abc")

    def test_play_bodies_are_rewritten_to_what_spotify_expects(self):
        cases = {
            "spotify:album:1": {"context_uri": "spotify:album:1"},
            "spotify:artist:2": {"context_uri": "spotify:artist:2"},
            "spotify:playlist:3": {"context_uri": "spotify:playlist:3"},
            "spotify:track:4": {"uris": ["spotify:track:4"]},
        }
        for uri, want in cases.items():
            self.call("PUT", "/v1/me/player/play?device_id=d", body=json.dumps({"uri": uri}).encode())
            self.assertEqual(json.loads(self.api_calls()[-1]["body"]), want)
        # Anything that is not a spotify: address is passed on untouched for Spotify to refuse.
        self.call("PUT", "/v1/me/player/play", body=b'{"uri":"http://evil.example"}')
        self.assertEqual(json.loads(self.api_calls()[-1]["body"]), {"uri": "http://evil.example"})

    def test_a_missing_refresh_token_is_a_clear_502_not_a_crash(self):
        self.token_file.unlink()
        status, body = self.call("GET", "/v1/me/player/devices")
        self.assertEqual(status, 502)
        self.assertIn(b"login", body)

    def test_a_rotated_refresh_token_is_saved(self):
        tokens = proxy.Tokens("cid", self.token_file)
        original = proxy.token_request
        proxy.token_request = lambda p: {"access_token": "a", "expires_in": 60, "refresh_token": "refresh-2"}
        try:
            tokens.get()
        finally:
            proxy.token_request = original
        self.assertEqual(json.loads(self.token_file.read_text())["refresh_token"], "refresh-2")

    def test_the_token_file_is_private_and_pkce_is_s256(self):
        mode = self.token_file.stat().st_mode & 0o777
        self.assertEqual(mode & 0o077, 0, oct(mode))
        verifier, challenge = proxy.pkce_pair()
        self.assertGreaterEqual(len(verifier), 43)
        self.assertNotEqual(verifier, challenge)
        self.assertNotIn("=", challenge)

    def test_it_refuses_to_listen_beyond_loopback_or_without_a_real_key(self):
        class Args:
            client_id = "x"
            token_file = "t"
            listen = "0.0.0.0:8890"

        import os

        os.environ["SPOTIFY_PROXY_KEY"] = KEY
        self.assertEqual(proxy.serve(Args()), 2)
        os.environ["SPOTIFY_PROXY_KEY"] = "short"
        Args.listen = "127.0.0.1:8890"
        self.assertEqual(proxy.serve(Args()), 2)


if __name__ == "__main__":
    unittest.main()
