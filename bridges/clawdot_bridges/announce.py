"""A client of ClawDot's ``POST /api/announce`` (REQ-INB-021) and the settings every bridge shares.

The endpoint token is read from an environment variable and sent only as an ``Authorization`` header. It is
never put in a URL, a log line or an error message.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass


class BridgeError(Exception):
    """A failure with a message that is safe to log (it never contains a token)."""


class Refused(BridgeError):
    """ClawDot refused the credential; retrying will not help until the token is fixed."""


class Busy(BridgeError):
    """ClawDot asked the bridge to slow down (HTTP 429)."""


@dataclass(frozen=True)
class Outcome:
    queued: bool
    detail: str


def require_env(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        raise BridgeError(f"set the environment variable {name}")
    return value


class AnnounceClient:
    def __init__(self, base_url: str, token: str, timeout: float = 10.0) -> None:
        if not base_url.startswith(("http://", "https://")):
            raise BridgeError("the ClawDot URL must start with http:// or https://")
        self.url = base_url.rstrip("/") + "/api/announce"
        self._token = token
        self.timeout = timeout

    def __repr__(self) -> str:  # keep the token out of any traceback or debug print
        return f"AnnounceClient(url={self.url!r})"

    def say(
        self,
        text: str,
        source: str,
        title: str | None = None,
        priority: str = "normal",
        ttl_secs: int | None = 900,
    ) -> Outcome:
        body: dict[str, object] = {"text": text, "source": source, "priority": priority}
        if title:
            body["title"] = title
        if ttl_secs:
            body["ttl_secs"] = ttl_secs
        req = urllib.request.Request(  # noqa: S310 - the URL scheme is checked in __init__
            self.url,
            data=json.dumps(body).encode(),
            method="POST",
            headers={"Content-Type": "application/json", "Authorization": f"Bearer {self._token}"},
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:  # noqa: S310
                answer = json.load(resp)
        except urllib.error.HTTPError as e:
            e.close()
            if e.code == 401:
                raise Refused("ClawDot refused the token (401): is it current and the endpoint on?") from None
            if e.code == 429:
                raise Busy("ClawDot says too many requests (429)") from None
            raise BridgeError(f"ClawDot answered HTTP {e.code}") from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise BridgeError("could not reach ClawDot") from None
        except ValueError:
            raise BridgeError("ClawDot sent an answer that is not JSON") from None
        if answer.get("queued"):
            return Outcome(True, "queued")
        return Outcome(False, f"filtered by the endpoint ({answer.get('filtered', 'unknown')})")
