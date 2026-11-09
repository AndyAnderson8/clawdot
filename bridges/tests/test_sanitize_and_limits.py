"""Cleaning and limiting: REQ-INB-029, REQ-INB-030. No network."""

from __future__ import annotations

import unittest

from clawdot_bridges.filters import KeywordFilter, cap_priority
from clawdot_bridges.limits import Dedupe, TokenBucket
from clawdot_bridges.sanitize import clean_name, clean_text


class SanitizeTest(unittest.TestCase):
    def test_markup_links_and_mentions_become_plain_words(self):
        cases = {
            "**bold** and __under__ and ~~gone~~ and ||spoiler||": "bold and under and gone and spoiler",
            "see [the docs](https://evil.example/x?y=1) now": "see the docs now",
            "![alt text](https://evil.example/i.png)": "alt text",
            "go to https://evil.example/path?q=1 please": "go to (link) please",
            "<https://evil.example/a> and www.evil.example/b": "(link) and (link)",
            "hi <@123456789012345678> and <@&123456789012345678> in <#123456789012345678>": "hi and in",
            "@everyone @here wake up": "wake up",
            "<:pepe:123456789012345678> <a:dance:123456789012345678> ok": "ok",
            "<t:1700000000:R> then": "then",
            "<b>bold</b><script>alert(1)</script>": "bold alert(1)",
            "> quoted\n# heading\n- item": "quoted heading - item",
            "javascript://x and data://y": "(link) and (link)",
        }
        for raw, want in cases.items():
            self.assertEqual(clean_text(raw), want, raw)

    def test_code_is_not_read_out(self):
        self.assertEqual(clean_text("run ```rm -rf /\nnow``` ok"), "run (code omitted) ok")
        self.assertEqual(clean_text("run ```never closed"), "run (code omitted)")
        self.assertEqual(clean_text("use `ls` here"), "use ls here")

    def test_control_and_direction_characters_are_removed(self):
        raw = "a‮b​c\u0007d⁦e﻿ f\x00g"
        self.assertEqual(clean_text(raw), "abcde fg")

    def test_look_alike_forms_are_folded(self):
        self.assertEqual(clean_text("Ｈｉ ①"), "Hi 1")

    def test_length_is_bounded_and_the_cut_is_marked(self):
        out = clean_text("word " * 200, 40)
        self.assertLessEqual(len(out), 40)
        self.assertTrue(out.endswith("…"))
        self.assertEqual(clean_text("x" * 100, 10), "x" * 9 + "…")

    def test_odd_input_does_not_crash(self):
        self.assertEqual(clean_text(None), "")  # type: ignore[arg-type]
        self.assertEqual(clean_text(""), "")
        self.assertEqual(clean_text("   \n\t  "), "")
        self.assertEqual(clean_text("[" * 500 + "](" + "x" * 5000), clean_text("[" * 500 + "](" + "x" * 5000))

    def test_names_are_short_plain_phrases(self):
        self.assertEqual(clean_name("**Ali** <@123456789012345678> https://x.example"), "Ali")
        self.assertEqual(len(clean_name("n" * 200)), 40)


class LimitsTest(unittest.TestCase):
    def test_a_burst_then_a_steady_rate(self):
        now = [0.0]
        b = TokenBucket(per_minute=6, burst=3, clock=lambda: now[0])
        self.assertEqual([b.allow() for _ in range(5)], [True, True, True, False, False])
        self.assertEqual(b.dropped, 2)
        now[0] += 10  # one token every ten seconds
        self.assertTrue(b.allow())
        self.assertFalse(b.allow())
        now[0] += 3600  # never more than the burst
        self.assertEqual([b.allow() for _ in range(5)], [True, True, True, False, False])

    def test_bad_limits_are_refused(self):
        for args in [(0, 1), (-1, 1), (5, 0)]:
            with self.assertRaises(ValueError):
                TokenBucket(*args)

    def test_repeats_are_dropped_only_within_the_window(self):
        now = [0.0]
        d = Dedupe(window=60, clock=lambda: now[0])
        self.assertFalse(d.is_repeat("a", "b"))
        self.assertTrue(d.is_repeat("a", "b"))
        self.assertFalse(d.is_repeat("a", "c"))
        self.assertFalse(d.is_repeat("ab", ""), "parts are not glued together")
        now[0] += 61
        self.assertFalse(d.is_repeat("a", "b"))

    def test_dedupe_memory_is_bounded(self):
        d = Dedupe(window=1e9, capacity=10)
        for i in range(1000):
            d.is_repeat(str(i))
        self.assertLessEqual(len(d.seen), 11)


class FilterTest(unittest.TestCase):
    def test_keywords(self):
        f = KeywordFilter(include=["Deploy", "backup"], exclude=["test"])
        self.assertTrue(f.passes("The DEPLOY finished"))
        self.assertFalse(f.passes("deploy test run"))
        self.assertFalse(f.passes("lunch"))
        self.assertTrue(KeywordFilter().passes("anything"))
        self.assertTrue(KeywordFilter(include=[" "]).passes("blank words are ignored"))

    def test_priority_is_capped(self):
        self.assertEqual(cap_priority("urgent", "normal"), "normal")
        self.assertEqual(cap_priority("low", "normal"), "low")
        self.assertEqual(cap_priority("urgent", "urgent"), "urgent")


if __name__ == "__main__":
    unittest.main()
