"""ntfy bridge: subscribe to ntfy topics and have ClawDot say the messages.

    CLAWDOT_TOKEN=cdt_... NTFY_TOKEN=tk_... python3 -m clawdot_bridges.ntfy --config ntfy.toml

ntfy is a topic-based push service. This sidecar holds a long-lived ``GET <server>/<topics>/json`` stream
(one JSON message per line), turns each ``message`` event into a plain sentence and posts it to
``/api/announce``. It runs off the speaker, so the default build stays small (REQ-INB-029).
"""

from __future__ import annotations

import argparse
import json
import logging
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field

from .announce import AnnounceClient, Busy, BridgeError, Refused, require_env
from .filters import KeywordFilter, cap_priority, load_config, priority_name
from .limits import Dedupe, TokenBucket
from .sanitize import clean_name, clean_text

log = logging.getLogger("clawdot.ntfy")


@dataclass(frozen=True)
class Message:
    id: str
    topic: str
    title: str
    text: str
    priority: int


def parse_line(line: str) -> Message | None:
    """One line of the stream. Only ``message`` events count; everything else (``open``, ``keepalive``,
    deletions, bad lines) is ignored."""
    try:
        data = json.loads(line)
    except ValueError:
        return None
    if not isinstance(data, dict) or data.get("event") != "message":
        return None
    prio = data.get("priority", 3)
    return Message(
        id=str(data.get("id", ""))[:64],
        topic=str(data.get("topic", ""))[:64],
        title=str(data.get("title", "")),
        text=str(data.get("message", "")),
        priority=prio if isinstance(prio, int) and 1 <= prio <= 5 else 3,
    )


@dataclass
class Settings:
    server: str = "https://ntfy.sh"
    topics: list[str] = field(default_factory=list)
    clawdot_url: str = "http://127.0.0.1:3335"
    source: str = "ntfy"
    min_priority: int = 1
    urgent_from: int = 5
    max_priority: str = "normal"
    include: list[str] = field(default_factory=list)
    exclude: list[str] = field(default_factory=list)
    per_minute: float = 6.0
    burst: int = 3
    ttl_secs: int = 900
    max_chars: int = 300

    @classmethod
    def from_config(cls, cfg: dict) -> Settings:
        s = cls(
            server=str(cfg.get("server", cls.server)).rstrip("/"),
            topics=[str(t) for t in cfg.get("topics", [])],
            clawdot_url=str(cfg.get("clawdot_url", cls.clawdot_url)),
            source=clean_name(str(cfg.get("source", "ntfy"))) or "ntfy",
            min_priority=int(cfg.get("min_priority", 1)),
            urgent_from=int(cfg.get("urgent_from", 5)),
            max_priority=priority_name(cfg.get("max_priority"), "normal"),
            include=[str(w) for w in cfg.get("include_keywords", [])],
            exclude=[str(w) for w in cfg.get("exclude_keywords", [])],
            per_minute=float(cfg.get("per_minute", 6)),
            burst=int(cfg.get("burst", 3)),
            ttl_secs=int(cfg.get("ttl_secs", 900)),
            max_chars=int(cfg.get("max_chars", 300)),
        )
        if not s.topics:
            raise BridgeError("list at least one topic in `topics` (the bridge forwards nothing by default)")
        for t in s.topics:
            if not t or not all(c.isalnum() or c in "-_" for c in t):
                raise BridgeError(f"topic `{t}`: use letters, digits, - and _ only")
        if not s.server.startswith(("http://", "https://")):
            raise BridgeError("`server` must start with http:// or https://")
        return s


class Bridge:
    """Decides what to say and says it. Pure apart from the announce client, so it is easy to test."""

    def __init__(self, settings: Settings, client: AnnounceClient, clock: Callable[[], float] = time.monotonic) -> None:
        self.s = settings
        self.client = client
        self.filter = KeywordFilter(settings.include, settings.exclude)
        self.bucket = TokenBucket(settings.per_minute, settings.burst, clock)
        self.dedupe = Dedupe(clock=clock)
        self.sent = 0

    def handle(self, m: Message) -> str:
        """Returns what happened, for the log: sent, or why not."""
        if m.topic not in self.s.topics:
            return "ignored: topic not listed"
        if m.priority < self.s.min_priority:
            return "ignored: below the minimum priority"
        text = clean_text(m.text, self.s.max_chars)
        title = clean_text(m.title, 120)
        if not (text or title):
            return "ignored: nothing left after cleaning"
        if not self.filter.passes(f"{title} {text}"):
            return "ignored: keyword filter"
        if self.dedupe.is_repeat(m.topic, title, text):
            return "ignored: repeat"
        if not self.bucket.allow():
            return "dropped: rate limit"
        wanted = "urgent" if m.priority >= self.s.urgent_from else "low" if m.priority <= 2 else "normal"
        priority = cap_priority(wanted, self.s.max_priority)
        try:
            outcome = self.client.say(
                text or title,
                source=self.s.source,
                title=title or None,
                priority=priority,
                ttl_secs=self.s.ttl_secs,
            )
        except Busy:
            return "dropped: ClawDot is busy"
        self.sent += 1
        return outcome.detail


def stream_lines(
    settings: Settings, auth_header: str | None, since: str | None, timeout: float = 90.0
) -> Iterator[str]:
    """Hold the stream open and yield its lines. ntfy sends a keepalive roughly every 45 s, so a quiet
    connection that goes ``timeout`` seconds without a line is treated as dead."""
    topics = ",".join(settings.topics)
    url = f"{settings.server}/{urllib.parse.quote(topics, safe=',')}/json"
    if since:
        url += "?" + urllib.parse.urlencode({"since": since})
    req = urllib.request.Request(url, headers={"Accept": "application/x-ndjson"})  # noqa: S310
    if auth_header:
        req.add_header("Authorization", auth_header)
    with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
        for raw in resp:
            yield raw.decode("utf-8", errors="replace").strip()


def auth_from_env(env: Callable[[str, str], str]) -> str | None:
    import base64

    token = env("NTFY_TOKEN", "")
    if token:
        return f"Bearer {token}"
    user, password = env("NTFY_USER", ""), env("NTFY_PASSWORD", "")
    if user and password:
        return "Basic " + base64.b64encode(f"{user}:{password}".encode()).decode()
    return None


def run(
    settings: Settings,
    client: AnnounceClient,
    auth: str | None,
    *,
    stop: Callable[[], bool] = lambda: False,
    sleep: Callable[[float], None] = time.sleep,
) -> int:
    bridge = Bridge(settings, client)
    since: str | None = None
    delay = 1.0
    while not stop():
        try:
            for line in stream_lines(settings, auth, since):
                delay = 1.0
                m = parse_line(line)
                if m is None:
                    continue
                since = m.id or since
                try:
                    log.info("%s %s: %s", m.topic, m.id, bridge.handle(m))
                except Refused as e:
                    log.error("%s", e)
                    return 1
                except BridgeError as e:
                    log.warning("%s", e)
                if stop():
                    return 0
        except urllib.error.HTTPError as e:
            if e.code in (401, 403):
                log.error("ntfy refused the credentials (%s); check NTFY_TOKEN or NTFY_USER and NTFY_PASSWORD", e.code)
                return 1
            log.warning("ntfy answered HTTP %s", e.code)
        except (urllib.error.URLError, TimeoutError, OSError):
            log.warning("lost the ntfy stream; retrying in %.0f s", delay)
        sleep(delay)
        delay = min(delay * 2, 60.0)
    return 0


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description="Forward ntfy messages to ClawDot's announce API.")
    p.add_argument("--config", required=True, help="TOML settings file (no secrets in it)")
    p.add_argument("-v", "--verbose", action="store_true")
    args = p.parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO, format="%(asctime)s %(levelname)s %(message)s"
    )
    try:
        import os

        settings = Settings.from_config(load_config(args.config))
        client = AnnounceClient(settings.clawdot_url, require_env("CLAWDOT_TOKEN"))
        auth = auth_from_env(lambda k, d: os.environ.get(k, d))
    except BridgeError as e:
        print(f"clawdot-ntfy: {e}", file=sys.stderr)
        return 2
    log.info("listening to %s on %s", ", ".join(settings.topics), settings.server)
    try:
        return run(settings, client, auth)
    except KeyboardInterrupt:
        return 0


if __name__ == "__main__":
    sys.exit(main())
