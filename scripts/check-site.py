#!/usr/bin/env python3
"""Checks the landing page in site/ follows its rules (see the landing-page spec).

Run: python3 scripts/check-site.py
"""
import re
import unittest
from html.parser import HTMLParser
from pathlib import Path

SITE = Path(__file__).resolve().parent.parent / "site"
DMG = "https://github.com/chengboonrong/agentpulse/releases/latest/download/AgentPulse.dmg"
EXPECTED = ["cats", "answers", "menubar", "private", "agents", "download"]
REDUCED = "(prefers-reduced-motion: reduce)"


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.refs = []        # (tag, url) of resources the page loads or links to locally
        self.loads = []       # urls the browser fetches on page load
        self.anchors = []     # (href, classes)
        self.section_ids = []
        self.sprites = []     # (img src, [reduced-motion source srcsets] of its picture)
        self.text = []
        self._picture = None
        self._skip = 0

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag in ("script", "style", "title"):
            self._skip += 1
        if tag == "script" and a.get("src"):
            self.loads.append(a["src"])
        if tag == "picture":
            self._picture = []
        if tag == "img" and a.get("src"):
            self.refs.append(a["src"])
            self.loads.append(a["src"])
            if self._picture is not None:
                self.sprites.append((a["src"], self._picture))
            else:
                self.sprites.append((a["src"], []))
        if tag == "source" and a.get("srcset"):
            urls = [part.strip().split()[0] for part in a["srcset"].split(",") if part.strip()]
            self.refs += urls
            self.loads += urls
            if self._picture is not None and a.get("media", "").strip() == REDUCED:
                self._picture += urls
        if tag == "link" and a.get("href"):
            self.refs.append(a["href"])
            rels = set((a.get("rel") or "").split())
            if rels & {"stylesheet", "icon", "apple-touch-icon", "preload"}:
                self.loads.append(a["href"])
        if tag == "a" and a.get("href"):
            self.anchors.append((a["href"], (a.get("class") or "").split()))
            self.refs.append(a["href"])
        if tag == "section" and a.get("id"):
            self.section_ids.append(a["id"])

    def handle_endtag(self, tag):
        if tag in ("script", "style", "title"):
            self._skip -= 1
        if tag == "picture":
            self._picture = None

    def handle_data(self, data):
        if not self._skip:
            self.text.append(data)


def is_local(url):
    return not re.match(r"^(https?:|mailto:|#|//)", url)


def is_apng(path):
    data = path.read_bytes()
    return data[:8] == b"\x89PNG\r\n\x1a\n" and b"acTL" in data[: data.find(b"IDAT")]


def luminance(hex_colour):
    h = hex_colour.lstrip("#")
    if len(h) == 3:
        h = "".join(c * 2 for c in h)
    def chan(v):
        v /= 255
        return v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4
    r, g, b = (chan(int(h[i:i + 2], 16)) for i in (0, 2, 4))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    la, lb = sorted((luminance(a), luminance(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def token_blocks(css):
    """The light :root block and the :root block inside the dark-scheme media query."""
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    light = re.search(r"(?<![\w-]):root\s*\{([^}]*)\}", css)
    dark_media = re.search(r"@media\s*\(prefers-color-scheme:\s*dark\)\s*\{\s*:root\s*\{([^}]*)\}", css)
    blocks = {}
    for name, m in (("light", light), ("dark", dark_media)):
        if m:
            blocks[name] = dict(re.findall(r"--([\w-]+)\s*:\s*(#[0-9A-Fa-f]{3,6})\b", m.group(1)))
    return blocks


class SiteRules(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.html = (SITE / "index.html").read_text()
        cls.page = Page()
        cls.page.feed(cls.html)
        cls.visible_text = re.sub(r"\s+", " ", " ".join(cls.page.text))
        cls.css = (SITE / "style.css").read_text()

    def test_local_references_exist(self):
        missing = sorted({u for u in self.page.refs if is_local(u)
                          and not (SITE / u.split("#")[0].split("?")[0]).exists()})
        self.assertEqual(missing, [])

    def test_no_third_party_requests(self):
        external = [u for u in self.page.loads if not is_local(u)]
        self.assertEqual(external, [])
        self.assertNotIn("<script", self.html)

    def test_download_buttons_are_one_click(self):
        hrefs = [h for h, classes in self.page.anchors if "download" in classes]
        self.assertGreaterEqual(len(hrefs), 2)
        self.assertTrue(all(h == DMG for h in hrefs), hrefs)

    def test_no_version_number(self):
        self.assertIsNone(re.search(r"\bv?\d+\.\d+\.\d+\b", self.visible_text))

    def test_every_animated_sprite_has_a_still(self):
        unguarded = []
        for src, stills in self.page.sprites:
            path = SITE / src
            if path.exists() and is_apng(path):
                if not any((SITE / s).exists() and not is_apng(SITE / s) for s in stills):
                    unguarded.append(src)
        self.assertEqual(unguarded, [])

    def test_sections_in_order(self):
        self.assertEqual([i for i in self.page.section_ids if i in EXPECTED], EXPECTED)

    def test_cpu_claim(self):
        self.assertIn("13 MB", self.visible_text)
        self.assertNotIn("1.5%", self.visible_text)

    def test_text_tokens_meet_contrast(self):
        blocks = token_blocks(self.css)
        self.assertEqual(sorted(blocks), ["dark", "light"])
        failures = []
        for name, t in blocks.items():
            for fg, bg in [("ink", "ground"), ("ink", "card"), ("muted", "ground"),
                           ("muted", "card"), ("link", "ground"), ("link", "card")]:
                if fg not in t or bg not in t:
                    failures.append(f"{name}: --{fg} or --{bg} missing")
                    continue
                ratio = contrast(t[fg], t[bg])
                if ratio < 4.5:
                    failures.append(f"{name}: --{fg} on --{bg} is {ratio:.2f}:1")
        self.assertEqual(failures, [])


if __name__ == "__main__":
    unittest.main()
