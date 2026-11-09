"""Keyword filtering and configuration loading shared by the bridges."""

from __future__ import annotations

import tomllib
from collections.abc import Iterable
from pathlib import Path

from .announce import BridgeError

PRIORITIES = ("low", "normal", "urgent")


class KeywordFilter:
    """Case-insensitive substring rules: a message passes if it has an include word (or none are set) and no
    exclude word. Matching is on the cleaned text, so markup cannot hide or fake a keyword."""

    def __init__(self, include: Iterable[str] = (), exclude: Iterable[str] = ()) -> None:
        self.include = [w.casefold() for w in include if w.strip()]
        self.exclude = [w.casefold() for w in exclude if w.strip()]

    def passes(self, text: str) -> bool:
        folded = text.casefold()
        if any(w in folded for w in self.exclude):
            return False
        return not self.include or any(w in folded for w in self.include)


def load_config(path: str | Path) -> dict:
    """Read a TOML settings file. Secrets do not belong in it; they come from environment variables."""
    try:
        with open(path, "rb") as f:
            cfg = tomllib.load(f)
    except OSError as e:
        raise BridgeError(f"cannot read {path}: {e.strerror}") from None
    except tomllib.TOMLDecodeError as e:
        raise BridgeError(f"{path} is not valid TOML: {e}") from None
    for key in cfg:
        if "token" in key.lower() or "password" in key.lower() or "secret" in key.lower():
            raise BridgeError(f"{path}: `{key}` looks like a secret; put it in an environment variable instead")
    return cfg


def priority_name(value: object, default: str = "normal") -> str:
    if value is None:
        return default
    if value not in PRIORITIES:
        raise BridgeError(f"priority must be one of {', '.join(PRIORITIES)}")
    return str(value)


def cap_priority(wanted: str, highest: str) -> str:
    """Never ask for more urgency than the settings allow (``urgent`` is spoken in quiet hours)."""
    return PRIORITIES[min(PRIORITIES.index(wanted), PRIORITIES.index(highest))]
