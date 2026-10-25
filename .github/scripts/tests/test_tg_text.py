"""Unit tests for the Markdown -> Telegram HTML renderer and message chunker."""

from __future__ import annotations

import json
import re
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

import tg_text as T  # noqa: E402

FIXTURES = Path(__file__).resolve().parent / "fixtures"
TESTS_DIR = Path(__file__).resolve().parent
REPO_ROOT = TESTS_DIR.parents[2]


def visible(html_text: str) -> str:
    """Plain text of a rendered fragment (for content comparisons)."""
    import html as html_module

    without_tags = re.sub(r"<[^>]+>", "", html_text)
    return re.sub(r"\s+", " ", html_module.unescape(without_tags)).strip()


class FormattingTests(unittest.TestCase):
    def test_persian_digits(self) -> None:
        self.assertEqual(T.fa_digits("2026"), "۲۰۲۶")

    def test_format_size(self) -> None:
        self.assertEqual(T.format_size(50179056), "۴۷٫۹ مگابایت")
        self.assertEqual(T.format_size(1024), "۱ کیلوبایت")
        self.assertEqual(T.format_size(512), "۵۱۲ بایت")

    def test_jalali_datetime_is_tehran_time(self) -> None:
        self.assertEqual(
            T.format_jalali_datetime("2026-10-24T20:53:20Z"),
            "۳ آبان ۱۴۰۵، ساعت ۰۰:۲۳",
        )

    def test_jalali_datetime_invalid(self) -> None:
        self.assertEqual(T.format_jalali_datetime(None), "—")
        self.assertEqual(T.format_jalali_datetime("not-a-date"), "—")

    def test_hashtag(self) -> None:
        self.assertEqual(T.hashtag_for("v1.7.0"), "#v1_7_0")
        self.assertEqual(T.hashtag_for(""), "")


class MarkdownTests(unittest.TestCase):
    def test_heading_becomes_bold(self) -> None:
        self.assertEqual(T.render_markdown("## جدید"), ["<b>جدید</b>"])

    def test_bullets_and_nesting(self) -> None:
        lines = T.render_markdown("- یک\n  - زیرشاخه\n    - عمیق")
        self.assertEqual(lines, ["• یک", "  ◦ زیرشاخه", "    ▪ عمیق"])

    def test_task_list(self) -> None:
        self.assertEqual(T.render_markdown("- [x] انجام شد"), ["☑ انجام شد"])
        self.assertEqual(T.render_markdown("- [ ] باقی مانده"), ["☐ باقی مانده"])

    def test_ordered_list(self) -> None:
        self.assertEqual(T.render_markdown("1. اول"), ["1. اول"])

    def test_rule_not_confused_with_bullet(self) -> None:
        self.assertEqual(T.render_markdown("---"), ["──────────────"])
        self.assertEqual(T.render_markdown("======"), ["──────────────"])
        self.assertEqual(T.render_markdown("- یک"), ["• یک"])

    def test_blockquote(self) -> None:
        self.assertEqual(T.render_markdown("> یادداشت"), ["│ یادداشت"])

    def test_table_rows(self) -> None:
        lines = T.render_markdown("| الف | ب |\n|---|---|\n| ۱ | ۲ |")
        self.assertEqual(lines, ["• الف · ب", "• ۱ · ۲"])

    def test_inline_code_and_link(self) -> None:
        lines = T.render_markdown("از `cargo test` استفاده کن و [سایت](https://x.com/a?b=1&c=2) را ببین")
        self.assertEqual(
            lines,
            ['از <code>cargo test</code> استفاده کن و '
             '<a href="https://x.com/a?b=1&amp;c=2">سایت</a> را ببین'],
        )

    def test_fenced_code_with_language(self) -> None:
        lines = T.render_markdown("```python\nprint('hi')\n```")
        self.assertEqual(len(lines), 1)
        self.assertEqual(lines[0], '<pre><code class="language-python">print(\'hi\')</code></pre>')

    def test_fenced_code_without_language(self) -> None:
        lines = T.render_markdown("```\nraw <tag> & stuff\n```")
        self.assertEqual(lines, ["<pre>raw &lt;tag&gt; &amp; stuff</pre>"])

    def test_fenced_code_unknown_language_has_no_class(self) -> None:
        lines = T.render_markdown("```brainfuck\n+++\n```")
        self.assertEqual(lines, ["<pre>+++</pre>"])

    def test_emphasis(self) -> None:
        self.assertEqual(T.render_markdown("**پررنگ**"), ["<b>پررنگ</b>"])
        self.assertEqual(T.render_markdown("~~حذف~~"), ["<s>حذف</s>"])
        self.assertEqual(T.render_markdown("*کج*"), ["<i>کج</i>"])

    def test_underscore_emphasis_kept_literal(self) -> None:
        # Technical identifiers must never be turned into emphasis.
        self.assertEqual(T.render_markdown("__init__ متد"), ["__init__ متد"])
        self.assertEqual(T.render_markdown("__پررنگ__"), ["__پررنگ__"])
        self.assertEqual(T.render_markdown("_کج_"), ["_کج_"])

    def test_snake_case_is_not_italic(self) -> None:
        self.assertEqual(
            T.render_markdown("مسیر route_exclude_address فعال شد"),
            ["مسیر route_exclude_address فعال شد"],
        )
        self.assertEqual(T.render_markdown("__init__ متد"), ["__init__ متد"])

    def test_image_dropped(self) -> None:
        self.assertEqual(T.render_markdown("![banner](https://x/y.png)"), [])

    def test_raw_html_stripped_but_autolink_kept(self) -> None:
        lines = T.render_markdown('<div>متن <b>مهم</b></div>\n<https://x.com/a>')
        self.assertEqual(lines, ['متن مهم', '<a href="https://x.com/a">https://x.com/a</a>'])

    def test_html_comment_removed(self) -> None:
        self.assertEqual(T.render_markdown("متن<!-- پنهان -->"), ["متن"])

    def test_crlf_normalised(self) -> None:
        self.assertEqual(T.render_markdown("الف\r\n\r\nب"), ["الف", "", "ب"])

    def test_ampersand_escaped(self) -> None:
        self.assertEqual(T.render_markdown("A & B < 5"), ["A &amp; B &lt; 5"])

    def test_script_tag_cannot_leak(self) -> None:
        rendered = T.render_markdown("<script>alert(1)</script>")
        self.assertEqual(T.html_problems(rendered[0]), [])

    def test_drop_duplicate_title(self) -> None:
        body = "## PacketYellow v1.7.0\n\nمهم\n- یک"
        self.assertEqual(
            T.drop_duplicate_title(body, "v1.7.0", "PacketYellow v1.7.0"),
            "مهم\n- یک",
        )

    def test_drop_duplicate_title_keeps_real_content(self) -> None:
        body = "## خلاصه\n\nمتن"
        self.assertEqual(T.drop_duplicate_title(body, "v1.7.0"), body)

    def test_empty_document(self) -> None:
        self.assertEqual(T.render_markdown(""), [])
        self.assertEqual(T.render_markdown("   \n\n  "), [])


class ChunkingTests(unittest.TestCase):
    def test_split_keeps_balance(self) -> None:
        text = "<b>" + ("سلام دنیا " * 80).strip() + "</b>"
        pieces = T.split_html(text, 200)
        self.assertGreater(len(pieces), 1)
        for piece in pieces:
            self.assertLessEqual(len(piece), 200)
            self.assertEqual(T.html_problems(piece), [])
        joined = visible(" ".join(pieces))
        self.assertEqual(joined, visible(text))

    def test_split_long_link(self) -> None:
        text = '<a href="https://example.com/x">' + ("متن پیوند " * 60).strip() + "</a>"
        pieces = T.split_html(text, 150)
        for piece in pieces:
            self.assertLessEqual(len(piece), 150)
            self.assertEqual(T.html_problems(piece), [])
            self.assertIn('<a href="https://example.com/x">', piece)
            self.assertIn("</a>", piece)

    def test_split_pre_block(self) -> None:
        code = "<pre>" + "\n".join(f"line-{i}" for i in range(200)) + "</pre>"
        pieces = T.split_html(code, 300)
        self.assertGreater(len(pieces), 1)
        for piece in pieces:
            self.assertLessEqual(len(piece), 300)
            self.assertEqual(T.html_problems(piece), [])
            self.assertTrue(piece.startswith("<pre>"))
            self.assertTrue(piece.endswith("</pre>"))

    def test_pack_lines_respects_limit(self) -> None:
        lines = [f"خط شمارهٔ {i} با متن کوتاه" for i in range(300)]
        chunks = T.pack_lines(lines, 500)
        self.assertGreater(len(chunks), 1)
        for chunk in chunks:
            self.assertLessEqual(len(chunk), 500)
            self.assertEqual(T.html_problems(chunk), [])

    def test_pack_long_single_line(self) -> None:
        line = "<b>" + ("x" * 5000) + "</b>"
        chunks = T.pack_lines([line], 1000)
        self.assertGreaterEqual(len(chunks), 5)
        for chunk in chunks:
            self.assertLessEqual(len(chunk), 1000)
            self.assertEqual(T.html_problems(chunk), [])
        self.assertEqual(visible("".join(chunks)), "x" * 5000)

    def test_pack_collapses_blank_lines(self) -> None:
        chunks = T.pack_lines(["a", "", "", "", "b"], 100)
        self.assertEqual(chunks, ["a\n\nb"])

    def test_html_problems_detects_unclosed(self) -> None:
        self.assertNotEqual(T.html_problems("<b>text"), [])
        self.assertNotEqual(T.html_problems("<unknown>x</unknown>"), [])
        self.assertNotEqual(T.html_problems("a & b"), [])
        self.assertEqual(T.html_problems("a &amp; b"), [])

    def test_html_problems_detects_bad_attribute(self) -> None:
        self.assertNotEqual(T.html_problems('<a onclick="x">y</a>'), [])


class EmphasisSafetyTests(unittest.TestCase):
    """Regression tests: overlapping emphasis used to emit crossed tags."""

    def assert_valid(self, markdown_text: str) -> str:
        lines = T.render_markdown(markdown_text)
        for line in lines:
            self.assertEqual(T.html_problems(line), [], line)
            self.assertNotIn("\x00", line)
        return " ".join(lines)

    def test_triple_star_is_bold_italic(self) -> None:
        self.assertEqual(T.render_markdown("***Breaking***"), ["<b><i>Breaking</i></b>"])

    def test_crossing_markers_stay_balanced(self) -> None:
        for markdown_text in (
            "**a *b** c*",
            "*a **b* c**",
            "**bold *italic* bold**",
            "*italic **bold** italic*",
            "~~strike **bold**~~",
        ):
            with self.subTest(text=markdown_text):
                self.assert_valid(markdown_text)

        for markdown_text in (
            "**bold *italic* bold**",
            "*italic **bold** italic*",
            "~~strike **bold**~~",
        ):
            self.assertIn("<b>", self.assert_valid(markdown_text))

    def test_unpaired_marker_keeps_markup_valid(self) -> None:
        self.assertEqual(T.render_markdown("2 * 3 = 6"), ["2 * 3 = 6"])
        # An unbalanced opener is closed at the end of the line (still valid)
        rendered = self.assert_valid("**ناتمام")
        self.assertIn("ناتمام", rendered)
        self.assertEqual(T.html_problems(rendered), [])

    def test_link_label_with_inline_code(self) -> None:
        self.assertEqual(
            T.render_markdown("[`docs`](https://x.com/d)"),
            ['<a href="https://x.com/d"><code>docs</code></a>'],
        )

    def test_link_label_with_autolink(self) -> None:
        rendered = self.assert_valid("[<https://x.com>](https://y.com)")
        self.assertIn('href="https://y.com"', rendered)

    def test_url_is_attribute_escaped(self) -> None:
        rendered = self.assert_valid('[x](https://x.com/?a=1&b=2)')
        self.assertIn('href="https://x.com/?a=1&amp;b=2"', rendered)
        quoted = self.assert_valid('[x](https://x.com/a%22b)')
        self.assertIn("href=", quoted)

    def test_unsafe_scheme_is_not_linked(self) -> None:
        rendered = self.assert_valid("[x](javascript:void)")
        self.assertNotIn("<a", rendered)
        self.assertIn("x", rendered)

    def test_attribute_validation(self) -> None:
        self.assertEqual(T.html_problems('<a href="https://x/?a=1&amp;b=2">y</a>'), [])
        self.assertNotEqual(T.html_problems('<a href="javascript:x">y</a>'), [])
        self.assertNotEqual(T.html_problems('<a onclick="x">y</a>'), [])
        self.assertNotEqual(T.html_problems('<code href="x">y</code>'), [])

    def test_telegram_length_counts_utf16_units(self) -> None:
        self.assertEqual(T.telegram_length("abc"), 3)
        self.assertEqual(T.telegram_length("🎉"), 2)
        self.assertEqual(T.telegram_length("۳"), 1)


class SplitBoundaryTests(unittest.TestCase):
    """Regression tests: split points used to duplicate/drop open tags."""

    def _visible(self, pieces) -> str:
        return visible("".join(pieces))

    def test_opening_tag_at_boundary(self) -> None:
        text = "a" * 10 + "<b>" + "b" * 10
        pieces = T.split_html(text, 12)
        for piece in pieces:
            self.assertLessEqual(len(piece), 12)
            self.assertEqual(T.html_problems(piece), [])
        self.assertEqual(self._visible(pieces), "a" * 10 + "b" * 10)
        self.assertEqual("".join(pieces).count("<b>"), "".join(pieces).count("</b>"))

    def test_closing_tag_at_boundary(self) -> None:
        text = "x" * 10 + "<b>bold</b>" + "y" * 10
        pieces = T.split_html(text, 14)
        for piece in pieces:
            self.assertLessEqual(len(piece), 14)
            self.assertEqual(T.html_problems(piece), [])
        self.assertEqual(self._visible(pieces), "x" * 10 + "bold" + "y" * 10)

    def test_nested_tags_across_boundary(self) -> None:
        text = "<b>" + ("پررنگ " * 40) + "</b>" + "<i>" + ("کج " * 40) + "</i>"
        pieces = T.split_html(text, 120)
        for piece in pieces:
            self.assertLessEqual(len(piece), 120)
            self.assertEqual(T.html_problems(piece), [])
        self.assertEqual(self._visible(pieces), visible(text))


class RealReleaseTests(unittest.TestCase):
    def test_every_real_release_renders_cleanly(self) -> None:
        fixtures = sorted(FIXTURES.glob("release-*.json"))
        self.assertGreaterEqual(len(fixtures), 5)
        for path in fixtures:
            with self.subTest(fixture=path.name):
                release = json.loads(path.read_text(encoding="utf-8"))
                body = T.drop_duplicate_title(
                    release.get("body") or "", release.get("tag_name", ""),
                    release.get("name") or "",
                )
                lines = T.render_markdown(body)
                chunks = T.pack_lines(lines, T.DEFAULT_BUDGET)
                self.assertGreaterEqual(len(chunks), 1)
                for chunk in chunks:
                    self.assertLessEqual(len(chunk), T.TELEGRAM_LIMIT)
                    self.assertEqual(T.html_problems(chunk), [])

    def test_long_release_needs_multiple_parts(self) -> None:
        release = json.loads(
            (FIXTURES / "release-v0.6.0.json").read_text(encoding="utf-8")
        )
        lines = T.render_markdown(release["body"])
        chunks = T.pack_lines(lines, T.DEFAULT_BUDGET)
        self.assertGreater(len(chunks), 1)


if __name__ == "__main__":
    unittest.main()
