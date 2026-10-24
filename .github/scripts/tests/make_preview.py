"""Generate a standalone HTML preview of the bot messages.

Usage (from the repository root):

    python .github/scripts/tests/make_preview.py out.html

It renders the real message plans for a few published releases plus the intro
message, and also shows what the previous workflow would have produced, so the
formatting can be reviewed without sending anything to Telegram.
"""

from __future__ import annotations

import html
import json
import re
import sys
from pathlib import Path
from typing import Any, Dict, List

TESTS_DIR = Path(__file__).resolve().parent
SCRIPTS_DIR = TESTS_DIR.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import telegram_bot as bot  # noqa: E402
import tg_text as T  # noqa: E402

FIXTURES = TESTS_DIR / "fixtures"


def config() -> bot.Config:
    return bot.Config(
        mode="auto",
        token="preview",
        chats=["@PacketYellow"],
        dry_run=True,
        state_file=Path("state.json"),
        intro_state_file=Path("intro.json"),
    )


def load_release(tag: str) -> Dict[str, Any]:
    return json.loads((FIXTURES / f"release-{tag}.json").read_text(encoding="utf-8"))


def old_message(release: Dict[str, Any], *, truncate: bool = False) -> str:
    """Replica of the previous workflow's message, for comparison.

    ``truncate=False`` mirrors the automatic workflow (no split at all), while
    ``truncate=True`` mirrors the old manual workflow (hard cut at 4000).
    """
    body = release.get("body") or ""
    body = re.sub(r"<!--.*?-->", "", body, flags=re.S)
    body = re.sub(r"(?m)^\s{0,3}#{1,6}\s+", "", body)
    body = re.sub(r"!\[([^\]]*)\]\([^)]+\)", r"\1", body)
    body = re.sub(r"\[([^\]]+)\]\((https?://[^)]+)\)", r"\1", body)
    body = re.sub(r"(\*\*|__|~~|`)", "", body)
    body = re.sub(r"(?m)^\s*[-*+]\s+", "• ", body)
    body = re.sub(r"\n[ \t]*\n(?:[ \t]*\n)+", "\n\n", body).strip()
    body = body or "جزئیات این نسخه هنوز ثبت نشده است."
    tag = release.get("tag_name", "")
    url = release.get("html_url", "")
    message = (
        "🟡 PACKETYELLOW\n━━━━━━━━━━━━━━\n\n"
        f"🚀 نسخهٔ جدید {tag} منتشر شد!\n\n"
        f"📝 تغییرات نسخه:\n\n{body}\n\n"
        "━━━━━━━━━━━━━━\n"
        f"🔗 مشاهدهٔ نسخه و فایل‌ها:\n{url}\n\n"
        "🌐 وب‌سایت رسمی:\nhttps://mehdi138iimm.github.io/PacketYellow/\n\n"
        "💛 کانال رسمی: @PacketYellow"
    )
    if truncate and len(message) > 4000:
        message = message[:3900] + "\n\n…\n🔗 ادامه و فایل‌ها:\n" + url
    return message


CSS = """
:root {
  --bg: #0e1621; --panel: #17212b; --panel-2: #1c2733; --text: #e9eef5;
  --muted: #7f91a4; --accent: #ffcc00; --link: #6ab3f3; --ok: #4fce8a;
  --bad: #ff6b6b; --border: rgba(255,255,255,.07);
}
* { box-sizing: border-box; }
body {
  margin: 0; background: radial-gradient(1200px 600px at 20% -10%, #1a2a3d 0%, var(--bg) 55%);
  color: var(--text); font-family: "Vazirmatn", "Segoe UI", Tahoma, sans-serif;
  direction: rtl; padding: 32px 18px 80px;
}
.wrap { max-width: 860px; margin: 0 auto; }
h1 { font-size: 26px; margin: 0 0 6px; }
h1 .dot { color: var(--accent); }
.sub { color: var(--muted); margin: 0 0 28px; font-size: 14px; line-height: 1.9; }
h2 { font-size: 18px; margin: 38px 0 6px; padding-top: 10px; border-top: 1px solid var(--border); }
h2 span { color: var(--muted); font-size: 13px; font-weight: 400; }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; margin: 14px 0 6px; }
.note { background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 14px 16px; }
.note b { color: var(--accent); }
.note p { margin: 6px 0 0; color: var(--muted); font-size: 13px; line-height: 1.9; }
.bad { color: var(--bad); } .ok { color: var(--ok); }
table { width: 100%; border-collapse: collapse; margin: 12px 0; font-size: 13px; }
th, td { border-bottom: 1px solid var(--border); padding: 8px 10px; text-align: right; }
th { color: var(--muted); font-weight: 600; }
.chat { background: var(--bg); border: 1px solid var(--border); border-radius: 16px; padding: 16px; margin: 16px 0 8px; }
.post { background: var(--panel); border-radius: 14px; padding: 14px 16px 10px; margin: 0 0 12px; box-shadow: 0 8px 24px rgba(0,0,0,.25); }
.head { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; }
.avatar { width: 34px; height: 34px; border-radius: 50%; background: #2b3a4a; display: grid; place-items: center; font-size: 17px; }
.ch { font-size: 14px; font-weight: 700; }
.meta { color: var(--muted); font-size: 12px; }
.msgtext { white-space: pre-wrap; word-wrap: break-word; line-height: 1.95; font-size: 14px; }
.msgtext a { color: var(--link); text-decoration: none; }
.msgtext code { background: rgba(255,255,255,.07); border-radius: 4px; padding: 1px 4px; font-family: Consolas, monospace; font-size: 12.5px; direction: ltr; display: inline-block; }
.msgtext pre { background: rgba(0,0,0,.28); border: 1px solid var(--border); border-radius: 8px; padding: 10px 12px; overflow-x: auto; direction: ltr; text-align: left; }
.msgtext pre code { background: none; padding: 0; font-size: 12.5px; }
.msgtext b { color: #fff; }
.buttons { display: flex; flex-wrap: wrap; gap: 6px; margin: 12px 0 4px; }
.btn { background: rgba(106,179,243,.14); color: var(--link); border-radius: 9px; padding: 7px 12px; font-size: 12.5px; border: 1px solid rgba(106,179,243,.25); }
.foot { color: var(--muted); font-size: 11px; display: flex; gap: 14px; margin-top: 8px; }
.side { display: grid; grid-template-columns: 1fr 1fr; gap: 14px; }
@media (max-width: 760px) { .side { grid-template-columns: 1fr; } }
.old { border: 1px dashed rgba(255,107,107,.45); }
.old .msgtext { color: #cfd8e3; }
.tag { display: inline-block; border-radius: 999px; padding: 2px 9px; font-size: 11px; margin-inline-start: 6px; }
.tag.bad { background: rgba(255,107,107,.14); border: 1px solid rgba(255,107,107,.35); }
.tag.ok { background: rgba(79,206,138,.12); border: 1px solid rgba(79,206,138,.35); }
details { margin-top: 8px; }
summary { cursor: pointer; color: var(--muted); font-size: 12px; }
details pre { background: #0b1219; padding: 10px; border-radius: 8px; overflow-x: auto; direction: ltr; text-align: left; font-size: 11.5px; color: #9fb3c8; }
"""


def render_post(plan: bot.MessagePlan, part: str, index: int, total: int,
                old: bool = False) -> str:
    buttons = ""
    if (not old) and index == total - 1 and plan.buttons:
        rows = []
        for row in plan.buttons:
            pills = "".join(
                f'<span class="btn">{html.escape(button["text"])}</span>' for button in row
            )
            rows.append(f'<div class="buttons">{pills}</div>')
        buttons = "".join(rows)
    label = f"بخش {index + 1} از {total}" if total > 3 else f"{index + 1}/{total}"
    return f"""
    <div class="post{ ' old' if old else '' }">
      <div class="head">
        <div class="avatar">🟡</div>
        <div>
          <div class="ch">PacketYellow</div>
          <div class="meta">کانال · {label}</div>
        </div>
      </div>
      <div class="msgtext">{part}</div>
      {buttons}
      <div class="foot"><span>👁 ۱۲٫۴ هزار بازدید</span><span>{len(part):,} کاراکتر</span></div>
    </div>"""


def main() -> None:
    out_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("telegram-preview.html")
    cfg = config()

    sections: List[str] = []
    rows: List[str] = []

    tags = ["v1.7.0", "v0.6.0", "v0.5.3"]
    for tag in tags:
        release = load_release(tag)
        plan = bot.build_release_plan(cfg, release)
        auto_len = len(old_message(release))
        manual_len = len(old_message(release, truncate=True))
        parts = len(plan.parts)
        status = (
            '<span class="tag bad">۴۰۰ می‌گرفت</span>'
            if auto_len > T.TELEGRAM_LIMIT
            else '<span class="tag ok">قبول</span>'
        )
        rows.append(
            f"<tr><td>{html.escape(tag)}</td><td>{len(release.get('body') or ''):,}</td>"
            f"<td>{auto_len:,} {status}</td><td>{manual_len:,}</td><td>{parts}</td></tr>"
        )
        posts = "".join(
            render_post(plan, part, index, parts) for index, part in enumerate(plan.parts)
        )
        sections.append(
            f'<h2>{html.escape(tag)} <span>— {parts} پیام · {len(plan.parts[0]):,} کاراکتر در بخش اول</span></h2>'
            f'<div class="chat">{posts}</div>'
        )

    intro_plan = bot.build_intro_plan(cfg)
    intro_posts = "".join(
        render_post(intro_plan, part, index, len(intro_plan.parts))
        for index, part in enumerate(intro_plan.parts)
    )

    release_short = load_release("v1.7.0")
    old_text = old_message(release_short)
    before_after = f"""
    <div class="side">
      <div class="chat old-wrap">
        <div class="post old">
          <div class="head"><div class="avatar">🟡</div><div><div class="ch">PacketYellow</div><div class="meta">قبل · پیام قدیمی (متن ساده)</div></div></div>
          <div class="msgtext">{html.escape(old_text)}</div>
          <div class="foot"><span>{len(old_text):,} کاراکتر</span><span>بدون دکمه · بدون استایل</span></div>
        </div>
      </div>
      <div class="chat">
        {render_post(bot.build_release_plan(cfg, release_short), bot.build_release_plan(cfg, release_short).parts[0], 0, len(bot.build_release_plan(cfg, release_short).parts))}
      </div>
    </div>"""

    page = f"""<!doctype html>
<html lang="fa" dir="rtl">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>PacketYellow · پیش‌نمایش پیام‌های ربات تلگرام</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<style>{CSS}</style>
</head>
<body>
<div class="wrap">
  <h1><span class="dot">🟡</span> پیش‌نمایش پیام‌های ربات تلگرام PacketYellow</h1>
  <p class="sub">
    این فایل خروجی واقعی <code>telegram_bot.py</code> برای نسخه‌های منتشرشدهٔ پروژه است — همان چیزی که
    در کانال <b>@PacketYellow</b> دیده می‌شود. برای دیدن متن خام HTML هر پیام، روی «HTML خام» کلیک کنید.
  </p>

  <h2>۱) چرا نسخهٔ قبلی کار نمی‌کرد</h2>
  <div class="grid">
    <div class="note">
      <b>۱. سقف ۴۰۹۶ کاراکتری تلگرام</b>
      <p>ورک‌فلوی قبلی یادداشت انتشار را بدون تقسیم می‌فرستاد. سه نسخهٔ آخر از این سقف عبور می‌کردند
      و تلگرام درخواست را با <span class="bad">400 message is too long</span> رد می‌کرد.</p>
    </div>
    <div class="note">
      <b>۲. رد شدن ارسال توسط تلگرام</b>
      <p>در تاریخ ۲۴ اکتبر هر دو اجرای ربات (رویداد انتشار و اجرای دستی) شکست خوردند؛ در کمتر از
      سه ثانیه و قبل از ذخیرهٔ وضعیت. یعنی خود <b>ارسال</b> رد شده بود: توکن، شناسهٔ چت یا
      ادمین‌نبودن ربات. حالا حالت <code>doctor</code> و پیام خطای فارسی دقیقاً همین را نشان می‌دهد.</p>
    </div>
    <div class="note">
      <b>۳. ضدتکرار و ویرایش/حذف</b>
      <p>فایل <code>telegram-message-map.json</code> خالی می‌ماند، پس حذف و ویرایش نسخه کار نمی‌کرد و
      هر اجرای دستی پیام تکراری می‌فرستاد. حالا شناسهٔ هر پیام بلافاصله ذخیره می‌شود.</p>
    </div>
  </div>

  <table>
    <tr><th>نسخه</th><th>کاراکتر یادداشت</th><th>پیام خودکار قبلی</th><th>پیام دستی قبلی (بریده)</th><th>پیام‌های جدید</th></tr>
    {''.join(rows)}
  </table>

  <h2>۲) قبل / بعد</h2>
  {before_after}

  <h2>۳) پیام‌های نسخه‌های واقعی</h2>
  {''.join(sections)}

  <h2>۴) پیام معرفی ربات</h2>
  <div class="chat">{intro_posts}</div>

  <p class="sub" style="margin-top:34px">
    تست‌ها: <code>python -m unittest discover -s .github/scripts/tests -v</code> — همهٔ پیام‌ها از نظر
    HTML مجاز تلگرام، سقف طول و سالم‌ماندن تگ‌ها در تقسیم بررسی می‌شوند.
  </p>
</div>
</body>
</html>
"""
    out_path.write_text(page, encoding="utf-8")
    print(f"preview written to {out_path} ({len(page):,} bytes)")


if __name__ == "__main__":
    main()
