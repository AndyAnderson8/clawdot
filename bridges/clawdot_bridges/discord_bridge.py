"""Discord bridge: have ClawDot say messages from chosen Discord channels.

    CLAWDOT_TOKEN=cdt_... DISCORD_TOKEN=... python3 -m clawdot_bridges.discord_bridge --config discord.toml

A Discord bot cannot be built from the standard library alone (the gateway needs WebSockets, heartbeats and
resumption), so this one program uses the well-known ``discord.py`` library, pinned in ``requirements-discord.txt``
and imported only when the bridge runs. Everything that decides what is said is plain Python in
:class:`Rules` and is tested without the library or a network (REQ-INB-029, REQ-INB-030).

Setup in the Discord developer portal: create a bot, turn on the privileged **Message Content** intent
(reading message text needs it), invite the bot to your server with permission to read the channels you list.
"""

from __future__ import annotations

import argparse
import logging
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass, field

from .announce import AnnounceClient, BridgeError, Busy, Refused, require_env
from .filters import KeywordFilter, cap_priority, load_config, priority_name
from .limits import Dedupe, TokenBucket
from .sanitize import clean_name, clean_text

log = logging.getLogger("clawdot.discord")


@dataclass(frozen=True)
class Incoming:
    """What the rules need to know about a message, with no Discord types."""

    channel_id: int
    channel_name: str
    author_id: int
    author_name: str
    author_is_bot: bool
    content: str
    mentions_bot: bool = False


@dataclass
class Settings:
    clawdot_url: str = "http://127.0.0.1:3335"
    source: str = "Discord"
    channels: list[int] = field(default_factory=list)
    users: list[int] = field(default_factory=list)
    ignore_bots: bool = True
    require_mention: bool = False
    include: list[str] = field(default_factory=list)
    exclude: list[str] = field(default_factory=list)
    urgent_keywords: list[str] = field(default_factory=list)
    max_priority: str = "normal"
    per_minute: float = 6.0
    burst: int = 3
    ttl_secs: int = 900
    max_chars: int = 300

    @classmethod
    def from_config(cls, cfg: dict) -> Settings:
        def ids(key: str) -> list[int]:
            out = []
            for v in cfg.get(key, []):
                if isinstance(v, bool) or not isinstance(v, int) or v <= 0:
                    raise BridgeError(f"`{key}` holds Discord ids (whole numbers), not {v!r}")
                out.append(v)
            return out

        s = cls(
            clawdot_url=str(cfg.get("clawdot_url", cls.clawdot_url)),
            source=clean_name(str(cfg.get("source", "Discord"))) or "Discord",
            channels=ids("channels"),
            users=ids("users"),
            ignore_bots=bool(cfg.get("ignore_bots", True)),
            require_mention=bool(cfg.get("require_mention", False)),
            include=[str(w) for w in cfg.get("include_keywords", [])],
            exclude=[str(w) for w in cfg.get("exclude_keywords", [])],
            urgent_keywords=[str(w) for w in cfg.get("urgent_keywords", [])],
            max_priority=priority_name(cfg.get("max_priority"), "normal"),
            per_minute=float(cfg.get("per_minute", 6)),
            burst=int(cfg.get("burst", 3)),
            ttl_secs=int(cfg.get("ttl_secs", 900)),
            max_chars=int(cfg.get("max_chars", 300)),
        )
        if not s.channels:
            raise BridgeError("list at least one channel id in `channels` (the bridge forwards nothing by default)")
        return s


@dataclass(frozen=True)
class Say:
    text: str
    title: str
    priority: str


class Rules:
    """Which messages are spoken, and what is said. No network, no Discord library."""

    def __init__(self, s: Settings, clock: Callable[[], float] = time.monotonic) -> None:
        self.s = s
        self.keywords = KeywordFilter(s.include, s.exclude)
        self.urgent = KeywordFilter(s.urgent_keywords)
        self.bucket = TokenBucket(s.per_minute, s.burst, clock)
        self.dedupe = Dedupe(clock=clock)

    def decide(self, m: Incoming) -> tuple[Say | None, str]:
        """The thing to say, or ``None`` and the reason."""
        if m.channel_id not in self.s.channels:
            return None, "channel not listed"
        if self.s.users and m.author_id not in self.s.users:
            return None, "author not listed"
        if self.s.ignore_bots and m.author_is_bot:
            return None, "from a bot"
        if self.s.require_mention and not m.mentions_bot:
            return None, "does not mention the bot"
        text = clean_text(m.content, self.s.max_chars)
        if not text:
            return None, "nothing left after cleaning"
        if not self.keywords.passes(text):
            return None, "keyword filter"
        if self.dedupe.is_repeat(str(m.channel_id), str(m.author_id), text):
            return None, "repeat"
        if not self.bucket.allow():
            return None, "rate limit"
        who = clean_name(m.author_name) or "someone"
        where = clean_name(m.channel_name) or "a channel"
        wanted = "urgent" if self.s.urgent_keywords and self.urgent.passes(text) else "normal"
        return Say(text=text, title=f"{who} in {where}", priority=cap_priority(wanted, self.s.max_priority)), "ok"


def deliver(rules: Rules, client: AnnounceClient, m: Incoming) -> str:
    say, why = rules.decide(m)
    if say is None:
        return f"ignored: {why}"
    try:
        out = client.say(
            say.text, source=rules.s.source, title=say.title, priority=say.priority, ttl_secs=rules.s.ttl_secs
        )
    except Busy:
        return "dropped: ClawDot is busy"
    return out.detail


def run(settings: Settings, client: AnnounceClient, discord_token: str) -> int:
    try:
        import discord  # the pinned library, only needed here
    except ImportError:
        print("clawdot-discord: install the pinned library: pip install -r requirements-discord.txt", file=sys.stderr)
        return 2

    rules = Rules(settings)
    intents = discord.Intents.default()
    intents.message_content = True
    bot = discord.Client(intents=intents)

    @bot.event
    async def on_ready() -> None:
        log.info("signed in; listening to %d channel(s)", len(settings.channels))

    @bot.event
    async def on_message(msg: discord.Message) -> None:
        if msg.author == bot.user or msg.channel.id not in settings.channels:
            return
        incoming = Incoming(
            channel_id=msg.channel.id,
            channel_name=getattr(msg.channel, "name", "") or "",
            author_id=msg.author.id,
            author_name=getattr(msg.author, "display_name", "") or msg.author.name,
            author_is_bot=msg.author.bot,
            content=msg.content or "",
            mentions_bot=bot.user is not None and bot.user in msg.mentions,
        )
        try:
            # urllib blocks, so keep it off the event loop that also answers Discord's heartbeats.
            import asyncio

            result = await asyncio.get_running_loop().run_in_executor(None, deliver, rules, client, incoming)
        except Refused as e:
            log.error("%s", e)
            await bot.close()
            return
        except BridgeError as e:
            log.warning("%s", e)
            return
        log.info("#%s: %s", incoming.channel_id, result)

    bot.run(discord_token, log_handler=None)
    return 0


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description="Forward chosen Discord channels to ClawDot's announce API.")
    p.add_argument("--config", required=True, help="TOML settings file (no secrets in it)")
    p.add_argument("-v", "--verbose", action="store_true")
    args = p.parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO, format="%(asctime)s %(levelname)s %(message)s"
    )
    try:
        settings = Settings.from_config(load_config(args.config))
        client = AnnounceClient(settings.clawdot_url, require_env("CLAWDOT_TOKEN"))
        discord_token = require_env("DISCORD_TOKEN")
    except BridgeError as e:
        print(f"clawdot-discord: {e}", file=sys.stderr)
        return 2
    return run(settings, client, discord_token)


if __name__ == "__main__":
    sys.exit(main())
