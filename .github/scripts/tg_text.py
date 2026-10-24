"""Rendering helpers for the PacketYellow Telegram release bot.

Pure-stdlib (no network, no file IO) so that everything here is unit-testable:

* GitHub-flavoured Markdown  ->  Telegram HTML (only tags Telegram accepts)
* Jalali (Persian) date/time and number/size formatting
* safe message chunking: messages are split on line boundaries, every chunk
  keeps balanced HTML tags, and oversized single blocks are split with open
  tags closed and re-opened so Telegram never answers "can't parse entities".

Telegram limits handled here:
* 4096 characters per message (we stay below it),
* 64 bytes per inline-keyboard button label.
"""

from __future__ import annotations

import html
import re
import urllib.parse
from datetime import datetime, timedelta, timezone
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

# --------------------------------------------------------------------------- #
# Constants
# --------------------------------------------------------------------------- #

#: Hard Telegram limit for a single message text.
TELEGRAM_LIMIT = 4096

#: Default working budget (markup included, so strictly below the hard limit).
DEFAULT_BUDGET = 3700

#: Tags we are allowed to emit; the test-suite validates this whitelist.
ALLOWED_TAGS = frozenset(
    {"b", "strong", "i", "em", "u", "ins", "s", "strike", "del",
     "code", "pre", "a", "blockquote", "br"}
)

#: Languages Telegram knows how to syntax-highlight in <pre><code>.
_TELEGRAM_LANGUAGES = frozenset(
    {"python", "javascript", "js", "bash", "shell", "sh", "json", "html", "css",
     "rust", "c", "cpp", "csharp", "c++", "java", "go", "sql", "yaml", "yml",
     "ini", "xml", "php", "ruby", "typescript", "ts", "diff", "dockerfile",
     "kotlin", "swift", "dart", "objectivec", "r", "matlab", "perl", "lua"}
)

_PERSIAN_DIGITS = str.maketrans("0123456789", "۰۱۲۳۴۵۶۷۸۹")

_JALALI_MONTHS = (
    "فروردین", "اردیبهشت", "خرداد", "تیر", "مرداد", "شهریور",
    "مهر", "آبان", "آذر", "دی", "بهمن", "اسفند",
)

# Iran has no DST since 2022.
_TEHRAN = timezone(timedelta(hours=3, minutes=30))

_HTML_COMMENT_RE = re.compile(r"<!--.*?-->", re.S)
_FENCE_RE = re.compile(
    r"(?ms)^[ \t]*```[ \t]*([A-Za-z0-9_+#.-]*)[ \t]*\n(.*?)^[ \t]*```[ \t]*$"
)
_AUTOLINK_RE = re.compile(r"<((?:https?://|tg://|mailto:)[^>\s]+)>")
_INLINE_CODE_RE = re.compile(r"``([^`\n]+)``|`([^`\n]+)`")
_IMAGE_RE = re.compile(r"!\[([^\]]*)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
_LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")

_HEADING_RE = re.compile(r"^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$")
_RULE_RE = re.compile(r"^\s{0,3}(?:(?:-\s*){3,}|(?:\*\s*){3,}|(?:_\s*){3,}|={3,})\s*$")
_QUOTE_RE = re.compile(r"^\s{0,3}>\s?(.*)$")
_BULLET_RE = re.compile(r"^(\s*)[-*+]\s+(.*)$")
_ORDERED_RE = re.compile(r"^(\s*)(\d{1,3})[.)]\s+(.*)$")
_TABLE_SEP_RE = re.compile(r"^\s*\|?[\s:|-]*-[\s:|-]*\|[\s:|-]*$")
_TASK_RE = re.compile(r"^\[([ xX])\]\s*(.*)$")

_TOKEN_RE = re.compile(r"\x00(\d+)\x00")
_FENCE_TOKEN_RE = re.compile(r"\x01(\d+)\x01")
_TAG_SPLIT_RE = re.compile(r"(<[^>]+>)")
_TAG_NAME_RE = re.compile(r"</?\s*([A-Za-z0-9]+)")
_VOID_TAGS = frozenset({"br", "hr", "img", "input", "meta", "link"})

_BREAK_CHARS = set(" \t،؛:!?؟…»”.,)]")


# --------------------------------------------------------------------------- #
# Small formatting helpers
# --------------------------------------------------------------------------- #

def escape_html(value: str) -> str:
    """Escape text for Telegram's HTML parse mode (text nodes)."""
    return html.escape(value or "", quote=False)


def escape_attr(value: str) -> str:
    """Escape a value for use inside an HTML attribute (href)."""
    return html.escape(value or "", quote=True)


def telegram_length(text: str) -> int:
    """Telegram counts characters in UTF-16 code units, not code points."""
    if not text:
        return 0
    return len(text.encode("utf-16-le")) // 2


_SAFE_URL_SCHEMES = frozenset({"http", "https", "tg", "mailto"})


def safe_url(url: str) -> bool:
    """Only http(s)/tg/mailto links are allowed inside Telegram messages."""
    try:
        parsed = urllib.parse.urlsplit((url or "").strip())
    except ValueError:
        return False
    return parsed.scheme.lower() in _SAFE_URL_SCHEMES and bool(parsed.netloc or parsed.path)


def fa_digits(value: str) -> str:
    """Convert ASCII digits to Persian digits (used for dates/sizes only)."""
    return str(value).translate(_PERSIAN_DIGITS)


def fa_number(value: float, decimals: int = 1) -> str:
    """Format a number with Persian digits and the Persian decimal separator."""
    if decimals <= 0:
        text = str(int(round(value)))
    else:
        text = f"{value:.{decimals}f}".rstrip("0").rstrip(".")
    return fa_digits(text).replace(".", "٫")


def format_size(num_bytes: Optional[int]) -> str:
    """Human readable file size, Persian digits, e.g. ``۴۷٫۹ مگابایت``."""
    if num_bytes is None:
        return "—"
    size = float(num_bytes)
    if size < 1024:
        return f"{fa_digits(int(size))} بایت"
    for factor, label in (
        (1024 ** 4, "ترابایت"),
        (1024 ** 3, "گیگابایت"),
        (1024 ** 2, "مگابایت"),
        (1024, "کیلوبایت"),
    ):
        if size >= factor:
            return f"{fa_number(size / factor)} {label}"
    return f"{fa_digits(int(size))} بایت"


def format_size_short(num_bytes: Optional[int]) -> str:
    """ASCII size for inline-keyboard labels (32 byte button limit)."""
    if num_bytes is None:
        return "?"
    size = float(num_bytes)
    for factor, suffix in (
        (1024 ** 3, "GB"),
        (1024 ** 2, "MB"),
        (1024, "KB"),
    ):
        if size >= factor:
            return f"{size / factor:.1f} {suffix}"
    return f"{int(size)} B"


def normalize_newlines(text: str) -> str:
    """CRLF / lone CR -> LF, strip BOM and NUL/control placeholders."""
    if not text:
        return ""
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    return text.lstrip("\ufeff")


def _gregorian_to_jalali(gy: int, gm: int, gd: int) -> Tuple[int, int, int]:
    """Gregorian -> Jalali conversion (Arash Partow's algorithm)."""
    g_days_in_month = (0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334)
    gy2 = gy + 1 if gm > 2 else gy
    days = (
        355666
        + 365 * gy
        + (gy2 + 3) // 4
        - (gy2 + 99) // 100
        + (gy2 + 399) // 400
        + gd
        + g_days_in_month[gm - 1]
    )
    jy = -1595 + 33 * (days // 12053)
    days %= 12053
    jy += 4 * (days // 1461)
    days %= 1461
    if days > 365:
        jy += (days - 1) // 365
        days = (days - 1) % 365
    if days < 186:
        jm = 1 + days // 31
        jd = 1 + days % 31
    else:
        jm = 7 + (days - 186) // 30
        jd = 1 + (days - 186) % 30
    return jy, jm, jd


def format_jalali_datetime(iso_value: Optional[str]) -> str:
    """ISO-8601 UTC timestamp -> ``۳ آبان ۱۴۰۵، ساعت ۰۰:۲۳`` (Tehran time)."""
    if not iso_value:
        return "—"
    try:
        raw = iso_value.strip().replace("Z", "+00:00")
        moment = datetime.fromisoformat(raw)
    except ValueError:
        return "—"
    if moment.tzinfo is None:
        moment = moment.replace(tzinfo=timezone.utc)
    local = moment.astimezone(_TEHRAN)
    jy, jm, jd = _gregorian_to_jalali(local.year, local.month, local.day)
    clock = f"{local.hour:02d}:{local.minute:02d}"
    return f"{fa_digits(jd)} {_JALALI_MONTHS[jm - 1]} {fa_digits(jy)}، ساعت {fa_digits(clock)}"


def iso_date(iso_value: Optional[str]) -> str:
    """``2026-10-24T20:53:20Z`` -> ``2026-10-24`` (empty string when unknown)."""
    if not iso_value:
        return ""
    return iso_value.strip().split("T", 1)[0]


def hashtag_for(version: str) -> str:
    """``v1.7.0`` -> ``#v1_7_0``."""
    cleaned = re.sub(r"[^0-9A-Za-z_]+", "_", version or "").strip("_")
    return f"#{cleaned}" if cleaned else ""


# --------------------------------------------------------------------------- #
# HTML / Markdown conversion
# --------------------------------------------------------------------------- #

def _convert_html_tags(text: str) -> str:
    """Best effort conversion of the few raw HTML tags release notes may hold."""
    text = re.sub(r"(?i)<br\s*/?>", "\n", text)
    text = re.sub(r"(?i)<hr\s*/?>", "\n──────────────\n", text)
    text = re.sub(r"(?i)<img\b[^>]*?/?>", "", text)
    text = re.sub(r"(?i)</?details[^>]*>", "", text)
    text = re.sub(r"(?is)<summary[^>]*>(.*?)</summary>", r"**\1**", text)

    def anchor(match: "re.Match[str]") -> str:
        return f"[{match.group(2).strip()}]({match.group(1)})"

    text = re.sub(
        r"""(?is)<a\b[^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>""", anchor, text
    )
    # Drop any remaining tag except bare autolinks such as <https://…>.
    text = re.sub(r"</?(?!(?:https?|tg|mailto):)[A-Za-z][^>]*>", "", text)
    return text


_EMPHASIS_MARKER_RE = re.compile(r"(\*\*\*|\*\*|~~|\*)")
_MARKERS = frozenset({"***", "**", "~~", "*"})


def _next_visible_char(tokens: Sequence[str], index: int) -> str:
    """First non-marker character after ``tokens[index]`` (``""`` if none)."""
    for position in range(index + 1, len(tokens)):
        token = tokens[position]
        if token in _MARKERS:
            continue
        if token:
            return token[0]
    return ""


def _emphasis(marked: str) -> str:
    """Apply Telegram emphasis markup to already escaped text.

    Implemented as a small stack-based parser rather than chained regex
    replacements: chained passes can produce overlapping tags such as
    ``<b><i>x</b></i>`` which Telegram rejects with "can't parse entities".
    Markers that cannot be paired (``2 * 3``) are kept literally; unbalanced
    opening markers are closed at the end of the line so the markup stays valid.

    Underscore emphasis (``_x_`` / ``__x__``) is intentionally not supported:
    technical release notes are full of identifiers such as ``__init__`` or
    ``route_exclude_address`` and those must stay literal.
    """
    if not marked:
        return marked

    tokens = _EMPHASIS_MARKER_RE.split(marked)
    out: List[str] = []
    stack: List[str] = []
    previous_char = ""

    def close(tag: str) -> None:
        # Close inner tags first so the emitted HTML always nests correctly.
        while stack:
            open_tag = stack.pop()
            out.append(f"</{open_tag}>")
            if open_tag == tag:
                break

    def can_open() -> bool:
        return bool(next_char) and not next_char.isspace()

    for index, token in enumerate(tokens):
        if token in _MARKERS:
            next_char = _next_visible_char(tokens, index)

            if token == "***":
                combined = stack[-2:] == ["b", "i"]
                if combined and previous_char and not previous_char.isspace():
                    close("i")
                    close("b")
                elif can_open():
                    stack.extend(["b", "i"])
                    out.append("<b><i>")
                else:
                    out.append(token)
                    previous_char = "*"
                continue

            tag = "b" if token == "**" else "s" if token == "~~" else "i"
            if tag in stack and previous_char and not previous_char.isspace():
                close(tag)
            elif can_open():
                stack.append(tag)
                out.append(f"<{tag}>")
            else:
                out.append(token)
                previous_char = token[-1]
            continue

        out.append(token)
        if token:
            previous_char = token[-1]

    while stack:
        out.append(f"</{stack.pop()}>")

    result = "".join(out)
    for _ in range(3):
        # Stray/unbalanced markers can leave an empty pair behind.
        result = re.sub(r"<(b|i|s)></\1>", "", result)
    return result


def render_inline(text: str) -> str:
    """Render one line of Markdown inline markup into Telegram HTML."""
    if not text:
        return ""
    store: List[str] = []

    def hold(value: str) -> str:
        store.append(value)
        return f"\x00{len(store) - 1}\x00"

    # Autolinks first, so escaping never eats <https://…>.
    def autolink(match: "re.Match[str]") -> str:
        url = match.group(1)
        if not safe_url(url):
            return escape_html(url)
        return hold(f'<a href="{escape_attr(url)}">{escape_html(url)}</a>')

    text = _AUTOLINK_RE.sub(autolink, text)

    def code(match: "re.Match[str]") -> str:
        payload = match.group(1) if match.group(1) is not None else match.group(2)
        return hold(f"<code>{escape_html(payload)}</code>")

    text = _INLINE_CODE_RE.sub(code, text)

    # Images (badges/screenshots) carry no value inside a Telegram post.
    text = _IMAGE_RE.sub("", text)

    def link(match: "re.Match[str]") -> str:
        url = match.group(2).strip()
        label = escape_html(match.group(1).strip())
        if not safe_url(url):
            return label or escape_html(url)
        label = _emphasis(label) or escape_html(url)
        return hold(f'<a href="{escape_attr(url)}">{label}</a>')

    text = _LINK_RE.sub(link, text)

    # Escape whatever is left, then apply emphasis on the escaped text.
    text = escape_html(text)
    text = _emphasis(text)

    # Restore held fragments; labels can contain earlier tokens, so repeat
    # until nothing changes (bounded to avoid a pathological loop).
    previous = None
    for _ in range(8):
        if previous == text:
            break
        previous = text
        text = _TOKEN_RE.sub(lambda match: store[int(match.group(1))], text)
    return text



def render_markdown(markdown: str) -> List[str]:
    """Convert a Markdown document into a list of rendered HTML lines.

    Empty strings represent paragraph breaks.  Each returned element is a
    self-contained, balanced HTML fragment (multi-line fenced code blocks are
    a single element and therefore never split by line packing).
    """
    if not markdown:
        return []

    text = normalize_newlines(markdown)
    text = _HTML_COMMENT_RE.sub("", text)

    fences: Dict[str, str] = {}

    def fence(match: "re.Match[str]") -> str:
        language = (match.group(1) or "").strip().lower()
        body = match.group(2).rstrip("\n")
        escaped = escape_html(body)
        if language in _TELEGRAM_LANGUAGES:
            fragment = f'<pre><code class="language-{language}">{escaped}</code></pre>'
        else:
            fragment = f"<pre>{escaped}</pre>"
        key = f"\x01{len(fences)}\x01"
        fences[key] = fragment
        return f"\n{key}\n"

    # Fenced blocks are protected *before* raw HTML conversion so that code
    # samples keep their angle brackets and ampersands.
    text = _FENCE_RE.sub(fence, text)
    text = _convert_html_tags(text)

    rendered: List[str] = []
    for raw_line in text.split("\n"):
        stripped = raw_line.strip()
        if not stripped:
            rendered.append("")
            continue

        fence_key = _FENCE_TOKEN_RE.fullmatch(stripped)
        if fence_key:
            rendered.append(fences[stripped])
            continue

        if _RULE_RE.match(stripped) and not _BULLET_RE.match(stripped):
            rendered.append("──────────────")
            continue

        heading = _HEADING_RE.match(raw_line)
        if heading:
            body = render_inline(heading.group(2).strip())
            if body:
                rendered.append(f"<b>{body}</b>")
            continue

        if _TABLE_SEP_RE.match(stripped):
            continue

        if stripped.startswith("|"):
            cells = [cell.strip() for cell in stripped.strip("|").split("|")]
            cells = [cell for cell in cells if cell]
            if cells:
                rendered.append("• " + " · ".join(render_inline(cell) for cell in cells))
            continue

        quote = _QUOTE_RE.match(raw_line)
        if quote:
            body = render_inline(quote.group(1).strip())
            if body:
                rendered.append("│ " + body)
            continue

        bullet = _BULLET_RE.match(raw_line)
        if bullet and not _RULE_RE.match(stripped):
            depth = min(len(bullet.group(1).expandtabs(2)) // 2, 2)
            marker = ("•", "◦", "▪")[depth]
            body = bullet.group(2).strip()
            task = _TASK_RE.match(body)
            if task:
                marker = "☑" if task.group(1).lower() == "x" else "☐"
                body = task.group(2)
            body_html = render_inline(body)
            if body_html:
                rendered.append("  " * depth + f"{marker} {body_html}")
            continue

        ordered = _ORDERED_RE.match(raw_line)
        if ordered:
            depth = min(len(ordered.group(1).expandtabs(2)) // 2, 2)
            body_html = render_inline(ordered.group(3).strip())
            if body_html:
                rendered.append("  " * depth + f"{ordered.group(2)}. {body_html}")
            continue

        inline_html = render_inline(stripped)
        if inline_html:
            rendered.append(inline_html)

    while rendered and not rendered[0]:
        rendered.pop(0)
    while rendered and not rendered[-1]:
        rendered.pop()
    return rendered


def drop_duplicate_title(markdown: str, version: str, name: str = "") -> str:
    """Remove a leading heading that just repeats the release title.

    Release bodies usually start with ``## PacketYellow v1.7.0``; the bot
    already prints that in its own header card, so it must not be duplicated.
    """
    lines = normalize_newlines(markdown).split("\n")
    index = 0
    while index < len(lines) and not lines[index].strip():
        index += 1
    if index >= len(lines):
        return markdown

    first = lines[index].strip()
    drop = False

    heading = _HEADING_RE.match(first)
    if heading:
        title = heading.group(2).strip()
        lowered = title.lower()
        bare_version = (version or "").lower().lstrip("v")
        drop = "packetyellow" in lowered or (
            bool(bare_version) and bare_version in lowered
        )
    elif name and first.lower() == name.lower():
        drop = True

    if not drop:
        return markdown

    del lines[index]
    while index < len(lines) and not lines[index].strip():
        del lines[index]
    return "\n".join(lines)


# --------------------------------------------------------------------------- #
# Chunking (the part that keeps Telegram's 4096 limit from breaking sends)
# --------------------------------------------------------------------------- #

def _tag_name(token: str) -> str:
    match = _TAG_NAME_RE.match(token)
    return match.group(1).lower() if match else ""


def _is_opening(token: str) -> bool:
    if token.startswith("</") or token.endswith("/>"):
        return False
    return _tag_name(token) not in _VOID_TAGS


def _is_closing(token: str) -> bool:
    return token.startswith("</")


def _best_break_index(text: str, room: int) -> int:
    """Largest index <= room that ends on a natural break character."""
    if room >= len(text):
        return len(text)
    limit = max(room, 1)
    for index in range(limit, 0, -1):
        if text[index - 1] in _BREAK_CHARS:
            return index
    return limit


def _has_visible_content(fragment: str) -> bool:
    """True when the fragment has text besides HTML tags."""
    return bool(re.sub(r"<[^>]+>", "", fragment).strip())


def split_html(text: str, limit: int) -> List[str]:
    """Split a balanced HTML fragment into balanced chunks of <= limit chars."""
    if limit <= 0:
        raise ValueError("limit must be positive")
    if len(text) <= limit:
        return [text]

    pieces: List[str] = []
    buffer: List[str] = []
    open_tags: List[Tuple[str, str]] = []

    def closers() -> str:
        return "".join(f"</{name}>" for name, _ in reversed(open_tags))

    def reopen() -> List[str]:
        return [tag for _, tag in open_tags]

    def current_len() -> int:
        return sum(len(item) for item in buffer) + len(closers())

    def flush() -> None:
        nonlocal buffer
        chunk = ("".join(buffer) + closers()).strip()
        if chunk and _has_visible_content(chunk):
            pieces.append(chunk)
        buffer = reopen()

    for token in _TAG_SPLIT_RE.split(text):
        if not token:
            continue
        if token.startswith("<") and token.endswith(">"):
            # Opening a tag also adds a closing tag, so both must fit.
            extra = len(f"</{_tag_name(token)}>") if _is_opening(token) else 0
            # Check for overflow *before* touching open_tags, otherwise the
            # token would be emitted twice (once by the reopen, once here).
            if buffer and current_len() + len(token) + extra > limit:
                flush()
            if _is_closing(token):
                name = _tag_name(token)
                if open_tags and open_tags[-1][0] == name:
                    open_tags.pop()
                elif any(tag == name for tag, _ in open_tags):
                    while open_tags and open_tags[-1][0] != name:
                        open_tags.pop()
                    if open_tags:
                        open_tags.pop()
            elif _is_opening(token):
                open_tags.append((_tag_name(token), token))
            buffer.append(token)
            continue

        remaining = token
        while remaining:
            room = limit - current_len()
            if room <= 0:
                # Try closing the current chunk first; only force progress when
                # even an empty chunk would not fit (degenerate tiny limits).
                flush()
                if limit - current_len() <= 0:
                    buffer.append(remaining[0])
                    remaining = remaining[1:]
                    flush()
                continue
            if len(remaining) <= room:
                buffer.append(remaining)
                remaining = ""
                continue
            cut = _best_break_index(remaining, room)
            buffer.append(remaining[:cut])
            flush()
            remaining = remaining[cut:]

    tail = ("".join(buffer) + closers()).strip()
    if tail and _has_visible_content(tail):
        pieces.append(tail)
    return pieces


def normalize_chunk(chunk: str) -> str:
    """Collapse redundant blank lines and trailing spaces inside one message."""
    lines = [line.rstrip() for line in chunk.split("\n")]
    result: List[str] = []
    blanks = 0
    for line in lines:
        if line == "":
            blanks += 1
            if blanks <= 1 and result:
                result.append("")
        else:
            blanks = 0
            result.append(line)
    while result and result[-1] == "":
        result.pop()
    while result and result[0] == "":
        result.pop(0)
    return "\n".join(result)


def pack_lines(lines: Sequence[str], limit: int) -> List[str]:
    """Greedily pack rendered lines into message-sized balanced chunks."""
    chunks: List[str] = []
    buffer: List[str] = []
    length = 0

    def push_buffer() -> None:
        nonlocal buffer, length
        if buffer:
            chunks.append("\n".join(buffer))
            buffer = []
            length = 0

    for line in lines:
        if len(line) > limit:
            push_buffer()
            pieces = split_html(line, limit)
            for piece in pieces[:-1]:
                chunks.append(piece)
            buffer = [pieces[-1]]
            length = len(pieces[-1])
            continue

        needed = len(line) + (1 if buffer else 0)
        if buffer and length + needed > limit:
            push_buffer()
            needed = len(line)
        buffer.append(line)
        length += needed

    push_buffer()
    packed = [normalize_chunk(chunk) for chunk in chunks]
    return [chunk for chunk in packed if chunk]


# --------------------------------------------------------------------------- #
# HTML validation helpers (used by the tests and by --dry-run)
# --------------------------------------------------------------------------- #

_STAT_TAG_RE = re.compile(r"<\s*(/?)\s*([A-Za-z0-9]+)([^>]*)>")
_ENTITY_RE = re.compile(r"&(?:amp|lt|gt|quot|apos|#\d{1,7}|#x[0-9a-fA-F]{1,6});")
_ATTR_RE = re.compile(r"""([A-Za-z0-9_:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))""")


def _has_bad_ampersand(text: str) -> bool:
    """True when text contains a bare ``&`` that Telegram would reject."""
    index = 0
    while True:
        index = text.find("&", index)
        if index == -1:
            return False
        match = _ENTITY_RE.match(text, index)
        if not match:
            return True
        index = match.end()


def html_problems(text: str) -> List[str]:
    """Return a list of reasons why ``text`` is not valid Telegram HTML."""
    problems: List[str] = []
    stack: List[str] = []
    position = 0

    for match in _STAT_TAG_RE.finditer(text):
        if match.start() != position and _has_bad_ampersand(text[position:match.start()]):
            problems.append(f"un-escaped '&' near offset {position}")
        position = match.end()

        closing = match.group(1) == "/"
        name = match.group(2).lower()
        attrs = match.group(3)

        if name not in ALLOWED_TAGS:
            problems.append(f"tag <{name}> is not supported by Telegram")
            continue
        if closing:
            if not stack or stack[-1] != name:
                problems.append(f"unexpected closing tag </{name}>")
            else:
                stack.pop()
            continue
        if name in _VOID_TAGS:
            continue

        allowed = {"href"} if name == "a" else {"class"} if name == "code" else set()
        for attr in _ATTR_RE.finditer(attrs):
            attr_name = attr.group(1).lower()
            value = attr.group(2) if attr.group(2) is not None else (
                attr.group(3) if attr.group(3) is not None else attr.group(4)
            )
            if attr_name not in allowed:
                problems.append(f"attribute '{attr_name}' not allowed on <{name}>")
            elif attr_name == "href":
                if not safe_url(value or ""):
                    problems.append(f"unsupported link scheme in href={value!r}")
                if _has_bad_ampersand(value or ""):
                    problems.append(f"un-escaped '&' in href={value!r}")
        # Unquoted leftovers or stray '=' inside the tag (already covered by
        # _ATTR_RE consuming quoted values, so nothing else to check).
        stack.append(name)

    if stack:
        problems.append(f"unclosed tag(s): {', '.join(stack)}")
    tail = text[position:]
    if _has_bad_ampersand(tail):
        problems.append("un-escaped '&' in message tail")
    return problems
