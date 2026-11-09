"""Make outside text safe to hand to ClawDot (REQ-INB-030).

ClawDot treats announcement text as untrusted data and cleans it again, but a bridge is the first place
where chat markup, links and look-alike characters can be removed, so it never forwards raw markup. The
result is plain words: no HTML, no Markdown, no links, no mentions, no control or direction-changing
characters, bounded in length.
"""

from __future__ import annotations

import re
import unicodedata

_FENCE = re.compile(r"```.*?(```|$)", re.DOTALL)
_INLINE_CODE = re.compile(r"`+([^`]*)`+")
_CUSTOM_EMOJI = re.compile(r"<a?:[A-Za-z0-9_]{1,32}:\d{5,25}>")
_MENTION = re.compile(r"<(@[!&]?|#)\d{5,25}>|@(everyone|here)\b")
_TIMESTAMP = re.compile(r"<t:-?\d{1,15}(:[tTdDfFR])?>")
_MD_IMAGE_OR_LINK = re.compile(r"!?\[([^\]]{0,200})\]\(([^)]{0,2000})\)")
_ANGLE_URL = re.compile(r"<(?:https?|ftp)://[^>\s]{0,2000}>", re.IGNORECASE)
_URL = re.compile(r"\b(?:https?|ftp|file|data|javascript)://\S+|\bwww\.\S+", re.IGNORECASE)
_HTML_TAG = re.compile(r"</?[A-Za-z][^>]{0,500}>")
_MD_MARKS = re.compile(r"(\*\*|__|~~|\|\||[*_~>#])")
_SPACE = re.compile(r"\s+")

# Format and direction controls that can reorder or hide text (bidi overrides, zero-width characters, BOM).
_INVISIBLE = {
    *range(0x200B, 0x2010),
    *range(0x202A, 0x202F),
    *range(0x2060, 0x2070),
    0xFEFF,
    0x00AD,
}


def _drop_controls(text: str) -> str:
    out = []
    for ch in text:
        cp = ord(ch)
        if cp in _INVISIBLE:
            continue
        cat = unicodedata.category(ch)
        if cat in ("Cc", "Cs", "Co", "Cn") and ch not in ("\n", "\t"):
            continue
        out.append(ch)
    return "".join(out)


def clean_text(text: str, max_chars: int = 400) -> str:
    """Plain, single-spaced words, at most ``max_chars`` characters (the cut is marked with an ellipsis)."""
    if not isinstance(text, str):
        return ""
    s = unicodedata.normalize("NFKC", text)
    s = _FENCE.sub(" (code omitted) ", s)
    s = _INLINE_CODE.sub(r"\1", s)
    s = _CUSTOM_EMOJI.sub(" ", s)
    s = _MENTION.sub(" ", s)
    s = _TIMESTAMP.sub(" ", s)
    s = _MD_IMAGE_OR_LINK.sub(lambda m: f" {m.group(1)} " if m.group(1).strip() else " ", s)
    s = _ANGLE_URL.sub(" (link) ", s)
    s = _URL.sub(" (link) ", s)
    s = _HTML_TAG.sub(" ", s)
    s = _drop_controls(s)
    s = _MD_MARKS.sub(" ", s)
    s = _SPACE.sub(" ", s).strip()
    if len(s) > max_chars:
        cut = s[: max_chars - 1].rsplit(" ", 1)[0] or s[: max_chars - 1]
        s = cut.rstrip(" ,;:-") + "…"
    return s


def clean_name(name: str, max_chars: int = 40) -> str:
    """A name (a channel, a person, a topic) as one short plain phrase."""
    s = clean_text(name, max_chars)
    return s.replace("(link)", "").replace("(code omitted)", "").strip()
