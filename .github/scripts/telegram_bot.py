"""PacketYellow Telegram release bot.

One entry point for every Telegram job of the repository:

* publish a release automatically (``release`` event),
* update a post when a release is edited,
* remove a post when a release is deleted,
* resend / send a specific or the latest release manually,
* send the one-time introduction message,
* diagnose the bot configuration (``doctor``).

Dependency-free (Python 3.9+ stdlib only) so GitHub Actions needs no install
step; the rendering/chunking helpers live in :mod:`tg_text` and are unit
tested separately.

Design notes that fix the old workflows' failure modes:

* the release payload is read from the event file instead of being pasted
  into an environment variable (multi-line notes stayed intact),
* every message is length-checked and split into balanced chunks, so long
  release notes no longer make Telegram answer "message is too long",
* messages are sent with ``parse_mode=HTML`` and inline-keyboard buttons,
* each sent message id is persisted *before* moving on, so reruns never
  duplicate, and edits/deletes can find the original post,
* Telegram and GitHub errors are translated into actionable Persian hints,
* ``--dry-run`` renders everything without sending, for review before deploy.
"""

from __future__ import annotations

import argparse
import html
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, List, Optional, Sequence, Tuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

import tg_text as T  # noqa: E402

# --------------------------------------------------------------------------- #
# Project constants (override with environment variables if needed)
# --------------------------------------------------------------------------- #

DEFAULT_REPO = "Mehdi138iimm/PacketYellow"
DEFAULT_SITE = "https://mehdi138iimm.github.io/PacketYellow/"
DEFAULT_CHANNEL_URL = "https://t.me/PacketYellow"
DEFAULT_CHANNEL_HANDLE = "@PacketYellow"
DEFAULT_REPO_URL = "https://github.com/Mehdi138iimm/PacketYellow"

DIVIDER = "━━━━━━━━━━━━━━━━━━━━"
BRAND = "🟡 <b>PacketYellow</b>"
STATE_VERSION = 2
INTRO_STATE_VERSION = 2

ASSET_LABELS = {
    ".exe": "EXE",
    ".msi": "MSI",
    ".zip": "ZIP",
    ".dmg": "DMG",
    ".appimage": "AppImage",
    ".deb": "DEB",
    ".rpm": "RPM",
    ".apk": "APK",
    ".7z": "7Z",
}
ASSET_SKIP_SUFFIXES = (
    ".sig",
    ".sha256",
    ".sha512",
    ".blockmap",
    ".txt",
    ".json",
)

MODE_ALIASES = {
    "auto": "auto",
    "release": "auto",
    "latest": "latest",
    "tag": "tag",
    "publish": "latest",
    "send": "latest",
    "intro": "intro",
    "introduce": "intro",
    "doctor": "doctor",
    "check": "doctor",
    "diagnose": "doctor",
}


class BotError(RuntimeError):
    """Fatal, user-facing error (prints a clean message, exits non-zero)."""


# --------------------------------------------------------------------------- #
# Logging helpers
# --------------------------------------------------------------------------- #

def log(message: str) -> None:
    print(message, flush=True)


def warn(message: str) -> None:
    print(f"::warning::{message}", flush=True)


def write_step_summary(markdown: str) -> None:
    """Append markdown to the GitHub Actions job summary when available."""
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not path:
        return
    try:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(markdown.rstrip() + "\n")
    except OSError:
        pass


# --------------------------------------------------------------------------- #
# Configuration
# --------------------------------------------------------------------------- #

def env_bool(name: str, default: bool = False) -> bool:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on", "y"}


def env_str(name: str, default: str = "") -> str:
    value = os.environ.get(name)
    return default if value is None or value == "" else value


def parse_chat_list(raw: str) -> List[str]:
    """Split ``@channel,-100123`` into a de-duplicated list of chat ids."""
    chats: List[str] = []
    for piece in re.split(r"[,\n;]+", raw or ""):
        chat = piece.strip()
        if chat and chat not in chats:
            chats.append(chat)
    return chats


@dataclass
class Config:
    mode: str = "auto"
    tag: str = ""
    force: bool = False
    dry_run: bool = False
    silent: bool = False
    token: str = ""
    chats: List[str] = field(default_factory=list)
    thread_id: Optional[int] = None
    repo: str = DEFAULT_REPO
    github_token: str = ""
    github_api: str = "https://api.github.com"
    telegram_api: str = "https://api.telegram.org"
    site_url: str = DEFAULT_SITE
    channel_url: str = DEFAULT_CHANNEL_URL
    channel_handle: str = DEFAULT_CHANNEL_HANDLE
    repo_url: str = DEFAULT_REPO_URL
    state_file: Path = Path(".github/telegram-message-map.json")
    intro_state_file: Path = Path(".github/telegram-intro-state.json")
    event_path: Optional[Path] = None
    send_interval: float = 1.5
    budget: int = T.DEFAULT_BUDGET
    max_parts: int = 10

    @property
    def channel_username(self) -> str:
        return self.channel_handle.lstrip("@") or "PacketYellow"

    def validate(self) -> None:
        for chat in self.chats:
            if chat.startswith("http://") or chat.startswith("https://"):
                raise BotError(
                    f"مقدار TG_CHAT باید شناسه یا یوزرنیم باشد، نه لینک ({chat}). "
                    "مثلاً @PacketYellow یا -1001234567890."
                )
        if self.mode not in MODE_ALIASES.values():
            raise BotError(f"حالت ناشناخته: {self.mode}")
        for name, value in (
            ("SITE_URL", self.site_url),
            ("CHANNEL_URL", self.channel_url),
            ("REPO_URL", self.repo_url),
        ):
            if not T.safe_url(value):
                raise BotError(f"آدرس {name} معتبر نیست (باید http/https باشد): {value}")
        # Keep the working budget safely below Telegram's hard limit and the
        # part count inside what a chat will tolerate before it looks spammy.
        self.budget = max(1200, min(self.budget, T.TELEGRAM_LIMIT - 200))
        self.max_parts = max(1, min(self.max_parts, 20))
        # doctor exists precisely to report a missing token/chat, so it must run
        # even when the configuration is incomplete.
        if self.mode == "doctor":
            return
        if not self.dry_run and not self.token:
            raise BotError(
                "Secret «TG_TOKEN» تنظیم نشده است. توکن ربات را از @BotFather "
                "بگیرید و در Settings → Secrets and variables → Actions با نام "
                "TG_TOKEN ذخیره کنید."
            )
        if not self.dry_run and not self.chats:
            raise BotError(
                "Secret «TG_CHAT» تنظیم نشده است. شناسهٔ کانال/گروه مقصد را "
                "(مثلاً @PacketYellow یا -1001234567890) در GitHub Secrets با "
                "نام TG_CHAT ذخیره کنید."
            )


def load_config(argv: Optional[Sequence[str]] = None) -> Config:
    parser = argparse.ArgumentParser(
        description="PacketYellow Telegram release bot",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument("--mode", help="auto | latest | tag | intro | doctor")
    parser.add_argument("--tag", help="release tag for --mode tag")
    parser.add_argument("--event-path", help="GitHub event JSON (defaults to GITHUB_EVENT_PATH)")
    parser.add_argument("--state-file")
    parser.add_argument("--intro-state-file")
    parser.add_argument("--repo", help="owner/name")
    parser.add_argument("--chat", help="comma separated chat ids (overrides TG_CHAT)")
    parser.add_argument("--thread-id", type=int)
    parser.add_argument("--github-api")
    parser.add_argument("--telegram-api")
    parser.add_argument("--send-interval", type=float)
    parser.add_argument("--budget", type=int)
    parser.add_argument("--max-parts", type=int)
    parser.add_argument("--force", action="store_true", default=None)
    parser.add_argument("--no-force", dest="force", action="store_false")
    parser.add_argument("--dry-run", action="store_true", default=None)
    parser.add_argument("--silent", action="store_true", default=None)
    args = parser.parse_args(argv)

    raw_mode = (args.mode or env_str("MODE", "auto")).strip().lower()
    mode = MODE_ALIASES.get(raw_mode, raw_mode)

    force = args.force if args.force is not None else env_bool("FORCE")
    dry_run = args.dry_run if args.dry_run is not None else env_bool("DRY_RUN")
    silent = args.silent if args.silent is not None else env_bool("SILENT")

    chat_raw = args.chat or env_str("CHAT_OVERRIDE") or env_str("TG_CHAT")
    thread_raw = args.thread_id if args.thread_id is not None else env_str("THREAD_ID")

    def _int(raw: Any, name: str) -> Optional[int]:
        if raw is None or str(raw).strip() == "":
            return None
        try:
            return int(str(raw).strip())
        except ValueError as exc:
            raise BotError(f"مقدار {name} باید عدد باشد (مقدار فعلی: {raw}).") from exc

    def _float(raw: Any, default: float) -> float:
        if raw is None or str(raw).strip() == "":
            return default
        try:
            return float(str(raw).strip())
        except ValueError:
            return default

    config = Config(
        mode=mode,
        tag=(args.tag or env_str("TAG")).strip(),
        force=bool(force),
        dry_run=bool(dry_run),
        silent=bool(silent),
        token=env_str("TG_TOKEN"),
        chats=parse_chat_list(chat_raw),
        thread_id=_int(thread_raw, "THREAD_ID"),
        repo=args.repo or env_str("REPOSITORY", DEFAULT_REPO),
        github_token=env_str("GH_TOKEN"),
        github_api=args.github_api or env_str("GITHUB_API_BASE", "https://api.github.com"),
        telegram_api=args.telegram_api or env_str("TELEGRAM_API_BASE", "https://api.telegram.org"),
        site_url=env_str("SITE_URL", DEFAULT_SITE),
        channel_url=env_str("CHANNEL_URL", DEFAULT_CHANNEL_URL),
        channel_handle=env_str("CHANNEL_HANDLE", DEFAULT_CHANNEL_HANDLE),
        repo_url=env_str("REPO_URL", DEFAULT_REPO_URL),
        state_file=Path(args.state_file or env_str("STATE_FILE", ".github/telegram-message-map.json")),
        intro_state_file=Path(
            args.intro_state_file or env_str("INTRO_STATE_FILE", ".github/telegram-intro-state.json")
        ),
        event_path=Path(args.event_path or env_str("GITHUB_EVENT_PATH", "")) if (
            args.event_path or env_str("GITHUB_EVENT_PATH")
        ) else None,
        send_interval=(
            args.send_interval
            if args.send_interval is not None
            else _float(env_str("SEND_INTERVAL", "1.5"), 1.5)
        ),
        budget=(
            args.budget
            if args.budget is not None
            else _int(env_str("MESSAGE_BUDGET", str(T.DEFAULT_BUDGET)), "MESSAGE_BUDGET")
        ),
        max_parts=(
            args.max_parts
            if args.max_parts is not None
            else _int(env_str("MAX_PARTS", "10"), "MAX_PARTS")
        ),
    )
    if config.repo_url == DEFAULT_REPO_URL and config.repo != DEFAULT_REPO:
        config.repo_url = f"https://github.com/{config.repo}"
    config.validate()
    return config


# --------------------------------------------------------------------------- #
# Telegram Bot API client
# --------------------------------------------------------------------------- #

class TelegramError(BotError):
    def __init__(self, method: str, code: int, description: str) -> None:
        self.method = method
        self.code = code
        self.description = description
        super().__init__(
            f"Telegram API «{method}» failed ({code}): {description}\n"
            f"راهنما: {telegram_hint(code, description)}"
        )


def telegram_hint(code: int, description: str) -> str:
    text = (description or "").lower()
    if code == 401 or "unauthorized" in text:
        return (
            "توکن ربات نامعتبر است. در @BotFather توکن را دوباره بگیرید و مقدار "
            "Secret با نام TG_TOKEN را به‌روز کنید."
        )
    if "chat not found" in text:
        return (
            "چت مقصد پیدا نشد. مقدار TG_CHAT را بررسی کنید (مثلاً @PacketYellow "
            "یا -1001234567890) و مطمئن شوید ربات حداقل یک‌بار عضو/ادمین کانال شده است."
        )
    if "not a member" in text or "kicked" in text or "chat_write_forbidden" in text:
        return (
            "ربات اجازهٔ ارسال در مقصد را ندارد: ربات را به‌عنوان ادمین کانال با "
            "دسترسی «Post messages» اضافه کنید."
        )
    if "not enough rights" in text or "forbidden" in text:
        return (
            "دسترسی ربات کافی نیست. ربات باید ادمین کانال با اجازهٔ ارسال پیام باشد."
        )
    if "message is too long" in text:
        return "متن پیام از ۴۰۹۶ کاراکتر بیشتر شده است؛ این خطا نباید رخ دهد، لطفاً Issue باز کنید."
    if "parse entities" in text or "unsupported start tag" in text:
        return (
            "HTML پیام نامعتبر است. با حالت dry-run خروجی را بررسی کنید؛ اگر تکرار شد Issue باز کنید."
        )
    if "message to delete not found" in text or "message can't be deleted" in text:
        return "پیام قبلی از قبل حذف شده است؛ نیازی به اقدام نیست."
    if "message is not modified" in text:
        return "محتوای پیام تغییری نکرده است؛ نیازی به اقدام نیست."
    if "thread not found" in text or "message_thread_id" in text:
        return "شناسهٔ Topic درست نیست؛ THREAD_ID را بررسی کنید یا خالی بگذارید."
    if code == 429:
        return "محدودیت نرخ تلگرام؛ اسکریپت خودکار چند ثانیه صبر و دوباره تلاش می‌کند."
    return "پاسخ تلگرام را در لاگ همین مرحله ببینید."


class TelegramClient:
    def __init__(
        self,
        token: str,
        base_url: str = "https://api.telegram.org",
        *,
        timeout: int = 30,
        max_attempts: int = 4,
    ) -> None:
        self.token = token
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.max_attempts = max_attempts
        self.bot_id: Optional[int] = None

    # -- low level ---------------------------------------------------------- #
    #: Methods that create/change a message.  Retrying them after a timeout or
    #: a 5xx could post the same content twice (the Bot API has no idempotency
    #: key), so only rate-limit answers are retried for these.
    MUTATING_METHODS = frozenset({"sendMessage", "editMessageText"})

    def call(self, method: str, payload: Dict[str, Any]) -> Any:
        url = f"{self.base_url}/bot{self.token}/{method}"
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        request = urllib.request.Request(
            url,
            data=body,
            method="POST",
            headers={"Content-Type": "application/json", "Accept": "application/json"},
        )
        mutating = method in self.MUTATING_METHODS

        last_error: Optional[Exception] = None
        for attempt in range(1, self.max_attempts + 1):
            try:
                with urllib.request.urlopen(request, timeout=self.timeout) as response:
                    raw = response.read().decode("utf-8", "replace")
                data = json.loads(raw or "{}")
                if not data.get("ok"):
                    raise TelegramError(method, int(data.get("error_code", 400)), str(data.get("description", raw)))
                return data.get("result")
            except urllib.error.HTTPError as exc:
                raw = exc.read().decode("utf-8", "replace")
                try:
                    data = json.loads(raw or "{}")
                except ValueError:
                    data = {}
                code = int(data.get("error_code") or exc.code or 0)
                description = str(data.get("description") or raw[:400] or "HTTP error")
                retry_after = int(((data.get("parameters") or {}).get("retry_after") or 0))
                last_error = TelegramError(method, code, description)
                if code == 429 and attempt < self.max_attempts:
                    wait = retry_after or 3 * attempt
                    log(f"⏳ محدودیت نرخ تلگرام؛ {wait} ثانیه صبر و تلاش دوباره…")
                    time.sleep(wait)
                    continue
                if 500 <= code < 600 and not mutating and attempt < self.max_attempts:
                    time.sleep(2 * attempt)
                    continue
                raise last_error from None
            except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as exc:
                last_error = exc
                if not mutating and attempt < self.max_attempts:
                    time.sleep(2 * attempt)
                    continue
                raise TelegramError(method, 0, str(exc)) from exc
        raise TelegramError(method, 0, str(last_error))

    # -- high level --------------------------------------------------------- #
    def get_me(self) -> Dict[str, Any]:
        me = self.call("getMe", {})
        self.bot_id = me.get("id")
        return me

    def get_chat(self, chat_id: str) -> Dict[str, Any]:
        return self.call("getChat", {"chat_id": chat_id})

    def get_chat_member(self, chat_id: str, user_id: int) -> Dict[str, Any]:
        return self.call("getChatMember", {"chat_id": chat_id, "user_id": user_id})

    def get_chat_member_count(self, chat_id: str) -> int:
        return int(self.call("getChatMemberCount", {"chat_id": chat_id}))

    def send_message(
        self,
        chat_id: str,
        text: str,
        *,
        silent: bool = False,
        reply_markup: Optional[Dict[str, Any]] = None,
        preview_url: Optional[str] = None,
        thread_id: Optional[int] = None,
        parse_mode: Optional[str] = "HTML",
    ) -> Dict[str, Any]:
        payload: Dict[str, Any] = {
            "chat_id": chat_id,
            "text": text,
            "disable_notification": bool(silent),
        }
        if parse_mode:
            payload["parse_mode"] = parse_mode
        if thread_id:
            payload["message_thread_id"] = thread_id
        if reply_markup:
            payload["reply_markup"] = reply_markup
        if preview_url:
            payload["link_preview_options"] = {
                "url": preview_url,
                "prefer_large_media": True,
                "show_above_text": False,
            }
        else:
            payload["link_preview_options"] = {"is_disabled": True}
        return self.call("sendMessage", payload)

    def edit_message_text(
        self,
        chat_id: str,
        message_id: int,
        text: str,
        *,
        reply_markup: Optional[Dict[str, Any]] = None,
        preview_url: Optional[str] = None,
        parse_mode: Optional[str] = "HTML",
    ) -> Dict[str, Any]:
        payload: Dict[str, Any] = {
            "chat_id": chat_id,
            "message_id": message_id,
            "text": text,
        }
        if parse_mode:
            payload["parse_mode"] = parse_mode
        # ``None`` means "leave the current keyboard untouched"; an explicit
        # empty keyboard removes a stale one after an edit.
        if reply_markup is not None:
            payload["reply_markup"] = reply_markup
        payload["link_preview_options"] = (
            {"url": preview_url, "prefer_large_media": True, "show_above_text": False}
            if preview_url
            else {"is_disabled": True}
        )
        try:
            return self.call("editMessageText", payload)
        except TelegramError as exc:
            if "message is not modified" in str(exc.description).lower():
                return {}
            raise

    def delete_message(self, chat_id: str, message_id: int) -> bool:
        try:
            self.call("deleteMessage", {"chat_id": chat_id, "message_id": message_id})
            return True
        except TelegramError as exc:
            text = str(exc.description).lower()
            if "not found" in text or "can't be deleted" in text or "message_id_invalid" in text:
                log(f"ℹ️ پیام {message_id} از قبل حذف شده بود.")
                return False
            raise


# --------------------------------------------------------------------------- #
# GitHub REST client
# --------------------------------------------------------------------------- #

class GitHubError(BotError):
    pass


class GitHubClient:
    def __init__(
        self,
        repo: str,
        token: str = "",
        api_url: str = "https://api.github.com",
        *,
        timeout: int = 30,
    ) -> None:
        self.repo = repo
        self.token = token
        self.api_url = api_url.rstrip("/")
        self.timeout = timeout

    def get(self, path: str) -> Any:
        url = f"{self.api_url}{path}"
        headers = {
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "PacketYellow-Telegram-Bot",
        }
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        request = urllib.request.Request(url, headers=headers)

        for attempt in range(1, 4):
            try:
                with urllib.request.urlopen(request, timeout=self.timeout) as response:
                    return json.loads(response.read().decode("utf-8"))
            except urllib.error.HTTPError as exc:
                body = exc.read().decode("utf-8", "replace")
                if exc.code == 404:
                    raise GitHubError(
                        f"ریلیز/ریپازیتوری در GitHub پیدا نشد ({path}). "
                        "نام ریپازیتوری یا برچسب نسخه را بررسی کنید."
                    ) from exc
                if exc.code in (403, 429):
                    remaining = exc.headers.get("X-RateLimit-Remaining", "?")
                    raise GitHubError(
                        f"GitHub API دسترسی را محدود کرده است (HTTP {exc.code}, "
                        f"remaining={remaining}). چند دقیقه بعد دوباره تلاش کنید یا "
                        "GH_TOKEN را تنظیم کنید."
                    ) from exc
                if 500 <= exc.code < 600 and attempt < 3:
                    time.sleep(2 * attempt)
                    continue
                raise GitHubError(f"GitHub API error (HTTP {exc.code}): {body[:300]}") from exc
            except (urllib.error.URLError, TimeoutError, OSError) as exc:
                if attempt < 3:
                    time.sleep(2 * attempt)
                    continue
                raise GitHubError(f"ارتباط با GitHub برقرار نشد: {exc}") from exc
        raise GitHubError("GitHub API request failed")

    def get_release_by_tag(self, tag: str) -> Dict[str, Any]:
        quoted = urllib.parse.quote(tag, safe="")
        return self.get(f"/repos/{self.repo}/releases/tags/{quoted}")

    def list_releases(self, per_page: int = 15) -> List[Dict[str, Any]]:
        releases = self.get(f"/repos/{self.repo}/releases?per_page={per_page}")
        return releases if isinstance(releases, list) else []

    def get_latest_release(self, include_prerelease: bool = False) -> Dict[str, Any]:
        releases = [release for release in self.list_releases() if not release.get("draft")]
        if not releases:
            raise GitHubError("هیچ ریلیز منتشرشده‌ای در این ریپازیتوری پیدا نشد.")

        def sort_key(item: Dict[str, Any]) -> str:
            return item.get("published_at") or item.get("created_at") or ""

        releases.sort(key=sort_key, reverse=True)
        if not include_prerelease:
            stable = [release for release in releases if not release.get("prerelease")]
            if stable:
                return stable[0]
        return releases[0]


# --------------------------------------------------------------------------- #
# Persistent state (message ids, so edits/deletes and reruns stay safe)
# --------------------------------------------------------------------------- #

class StateStore:
    """Tracks which Telegram messages belong to which release."""

    def __init__(self, path: Path) -> None:
        self.path = Path(path)
        self.data: Dict[str, Any] = {}
        self.changed = False
        self.load()

    def load(self) -> None:
        raw: Any = {}
        if self.path.exists():
            try:
                raw = json.loads(self.path.read_text(encoding="utf-8"))
            except (OSError, ValueError) as exc:
                warn(f"فایل وضعیت قابل خواندن نیست ({exc})؛ از وضعیت خالی شروع می‌شود.")
                raw = {}
        self.data = self._migrate(raw if isinstance(raw, dict) else {})

    @staticmethod
    def _migrate(raw: Dict[str, Any]) -> Dict[str, Any]:
        if raw.get("version") == STATE_VERSION and isinstance(raw.get("releases"), dict):
            return raw
        releases: Dict[str, Any] = {}
        for key, value in raw.items():
            if not isinstance(value, dict) or "message_id" not in value:
                continue
            chat = str(value.get("chat_id") or "")
            releases[str(key)] = {
                "tag": value.get("tag") or "",
                "title": value.get("title") or "",
                "chats": {chat: {"message_ids": [int(value["message_id"])]}},
            }
        return {"version": STATE_VERSION, "updated_at": "", "releases": releases}

    # -- persistence -------------------------------------------------------- #
    def save(self) -> None:
        self.data["version"] = STATE_VERSION
        self.data["updated_at"] = datetime.now(timezone.utc).isoformat(timespec="seconds")
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temp = self.path.with_suffix(self.path.suffix + ".tmp")
        temp.write_text(
            json.dumps(self.data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        os.replace(temp, self.path)
        self.changed = True

    # -- release helpers ---------------------------------------------------- #
    def release_entry(self, release_id: str) -> Optional[Dict[str, Any]]:
        return (self.data.get("releases") or {}).get(str(release_id))

    def messages_for(self, release_id: str, chat: str) -> List[int]:
        entry = self.release_entry(release_id) or {}
        info = (entry.get("chats") or {}).get(str(chat)) or {}
        return [int(item) for item in info.get("message_ids") or []]

    def record(
        self,
        release_id: str,
        *,
        tag: str,
        title: str,
        chat: str,
        message_ids: Sequence[int],
    ) -> None:
        releases = self.data.setdefault("releases", {})
        entry = releases.setdefault(str(release_id), {"chats": {}})
        entry["tag"] = tag
        entry["title"] = title
        entry.setdefault("chats", {})[str(chat)] = {
            "message_ids": [int(item) for item in message_ids],
            "updated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        }
        entry.setdefault("sent_at", datetime.now(timezone.utc).isoformat(timespec="seconds"))
        self.prune()

    def remove_release(self, release_id: str) -> None:
        releases = self.data.get("releases") or {}
        if str(release_id) in releases:
            del releases[str(release_id)]

    def all_chats(self) -> List[str]:
        chats: List[str] = []
        for entry in (self.data.get("releases") or {}).values():
            for chat in (entry.get("chats") or {}):
                if chat and chat not in chats:
                    chats.append(chat)
        return chats

    def prune(self, keep: int = 60) -> None:
        releases = self.data.get("releases") or {}
        if len(releases) <= keep:
            return

        def sort_key(item: Tuple[str, Dict[str, Any]]) -> str:
            return item[1].get("sent_at") or ""

        for key, _ in sorted(releases.items(), key=sort_key)[: len(releases) - keep]:
            del releases[key]


class IntroState:
    """Tracks whether the one-time introduction message was already sent."""

    def __init__(self, path: Path) -> None:
        self.path = Path(path)
        self.data: Dict[str, Any] = self._load()

    def _load(self) -> Dict[str, Any]:
        if self.path.exists():
            try:
                raw = json.loads(self.path.read_text(encoding="utf-8"))
                if isinstance(raw, dict):
                    raw.setdefault("version", INTRO_STATE_VERSION)
                    raw.setdefault("chats", {})
                    return raw
            except (OSError, ValueError):
                pass
        return {"version": INTRO_STATE_VERSION, "intro_sent": False, "chats": {}}

    @property
    def sent(self) -> bool:
        return bool(self.data.get("intro_sent"))

    def mark(self, chat: str, message_ids: Sequence[int]) -> None:
        self.data["version"] = INTRO_STATE_VERSION
        self.data["intro_sent"] = True
        self.data["sent_at"] = datetime.now(timezone.utc).isoformat(timespec="seconds")
        self.data.setdefault("chats", {})[str(chat)] = {
            "message_ids": [int(item) for item in message_ids]
        }

    def save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temp = self.path.with_suffix(self.path.suffix + ".tmp")
        temp.write_text(
            json.dumps(self.data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        os.replace(temp, self.path)


# --------------------------------------------------------------------------- #
# Message composition
# --------------------------------------------------------------------------- #

@dataclass
class MessagePlan:
    parts: List[str]
    buttons: List[List[Dict[str, str]]] = field(default_factory=list)
    preview_url: Optional[str] = None
    version: str = ""
    assets: List[Dict[str, Any]] = field(default_factory=list)


def _release_version(release: Dict[str, Any]) -> str:
    tag = (release.get("tag_name") or "").strip()
    if tag:
        return tag
    name = (release.get("name") or "").strip()
    return name or "بدون شمارهٔ نسخه"


def _release_assets(release: Dict[str, Any]) -> List[Dict[str, Any]]:
    assets = [asset for asset in release.get("assets") or [] if isinstance(asset, dict)]
    return assets


def _primary_assets(assets: Sequence[Dict[str, Any]]) -> List[Dict[str, Any]]:
    usable = [
        asset
        for asset in assets
        if not str(asset.get("name", "")).lower().endswith(ASSET_SKIP_SUFFIXES)
    ]
    if not usable:
        usable = list(assets)
    primary: List[Dict[str, Any]] = []
    for suffix in (".exe", ".msi"):
        for asset in usable:
            if str(asset.get("name", "")).lower().endswith(suffix) and asset not in primary:
                primary.append(asset)
                break
    for asset in usable:
        if len(primary) >= 2:
            break
        if asset not in primary:
            primary.append(asset)
    return primary[:2]


def _asset_button_text(asset: Dict[str, Any]) -> str:
    name = str(asset.get("name") or "download")
    suffix = Path(name).suffix.lower()
    label = ASSET_LABELS.get(suffix, (suffix.lstrip(".") or "FILE").upper())
    size = T.format_size_short(asset.get("size"))
    text = f"⬇️ {label} · {size}"
    # Telegram allows 64 bytes per button label.
    if len(text.encode("utf-8")) > 64:
        text = f"⬇️ {label}"
    return text


def build_buttons(config: Config, release: Optional[Dict[str, Any]]) -> List[List[Dict[str, str]]]:
    rows: List[List[Dict[str, str]]] = []
    if release:
        assets = _release_assets(release)
        primary = _primary_assets(assets)
        asset_row: List[Dict[str, str]] = []
        for asset in primary[:2]:
            url = asset.get("browser_download_url")
            if url:
                asset_row.append({"text": _asset_button_text(asset), "url": str(url)})
        if asset_row:
            rows.append(asset_row)

        release_url = release.get("html_url")
        first_row: List[Dict[str, str]] = []
        if release_url:
            first_row.append({"text": "📄 صفحهٔ انتشار", "url": str(release_url)})
        first_row.append({"text": "🌐 وب‌سایت", "url": config.site_url})
        if first_row:
            rows.append(first_row)
        rows.append(
            [
                {"text": "💻 GitHub", "url": config.repo_url},
                {"text": "📢 کانال", "url": config.channel_url},
            ]
        )
    else:
        rows.append(
            [
                {"text": "⬇️ آخرین نسخه", "url": f"{config.repo_url}/releases/latest"},
                {"text": "🌐 وب‌سایت", "url": config.site_url},
            ]
        )
        rows.append(
            [
                {"text": "💻 GitHub", "url": config.repo_url},
                {"text": "📢 کانال", "url": config.channel_url},
            ]
        )
    return rows


def _continuation_header(version: str, index: int, total: int) -> str:
    return (
        f"{BRAND} · <b>{T.escape_html(version)}</b>"
        f" — ادامهٔ تغییرات ({T.fa_digits(index)}/{T.fa_digits(total)})"
    )


def _essential_footer(footer_lines: Sequence[str]) -> List[str]:
    """Keep the links/tags part of a footer when the message must shrink."""
    lines = list(footer_lines)
    for index, line in enumerate(lines):
        if line.startswith("🔗") or line.startswith("📄"):
            return lines[index:]
    for index, line in enumerate(lines):
        if "#PacketYellow" in line:
            return lines[index:]
    return lines[-3:] if lines else []


def _footer_links(more_url: str) -> str:
    return f'🔗 <a href="{T.escape_attr(more_url)}">صفحهٔ انتشار و همهٔ فایل‌ها</a>'


def assemble_parts(
    config: Config,
    header: Sequence[str],
    notes: Sequence[str],
    footer: Sequence[str],
    version: str,
    more_url: str,
) -> List[str]:
    """Pack header + notes + footer into balanced Telegram-sized messages."""
    base_notes = list(notes)
    keep = len(base_notes)
    footer_lines = list(footer)
    budget = max(1200, config.budget)

    for _attempt in range(60):
        if keep < len(base_notes):
            notes = base_notes[:keep] + [
                "",
                f'🔗 <a href="{T.escape_attr(more_url)}">… ادامهٔ یادداشت انتشار در صفحهٔ انتشار</a>',
            ]
        else:
            notes = list(base_notes)

        continuation = _continuation_header(version, 99, 99)
        reserve = T.telegram_length(continuation) + 2
        chunks = T.pack_lines(list(header) + notes, max(700, budget - reserve))
        footer_text = T.normalize_chunk("\n".join(footer_lines)).strip()

        if chunks and footer_text and (
            T.telegram_length(chunks[-1]) + 2 + T.telegram_length(footer_text) + reserve <= budget
        ):
            merged = T.normalize_chunk(chunks[-1] + "\n\n" + footer_text)
            if T.telegram_length(merged) <= T.TELEGRAM_LIMIT:
                chunks[-1] = merged
            else:
                chunks.append(footer_text)
        elif footer_text:
            chunks.append(footer_text)

        total = len(chunks)
        decorated: List[str] = []
        for index, chunk in enumerate(chunks):
            if index == 0:
                decorated.append(chunk)
            else:
                decorated.append(
                    _continuation_header(version, index + 1, total) + "\n\n" + chunk
                )

        if any(T.telegram_length(part) > T.TELEGRAM_LIMIT for part in decorated):
            budget = max(1200, budget - 150)
            continue

        if total > config.max_parts:
            if keep > 0:
                keep = max(0, int(keep * 0.75) - 2)
                continue
            essential = _essential_footer(footer_lines)
            if len(footer_lines) > len(essential):
                footer_lines = essential
                continue
            # Last resort: never send more messages than the configured cap.
            decorated = decorated[: config.max_parts]
            if decorated:
                last = decorated[-1]
                trimmed = T.normalize_chunk(last + "\n\n" + T.DIVIDER + "\n\n" + _footer_links(more_url))
                if T.telegram_length(trimmed) <= T.TELEGRAM_LIMIT:
                    decorated[-1] = trimmed
            return decorated

        return decorated

    raise BotError("ساخت پیام ناموفق بود: تقسیم متن به پیام‌های مجاز ممکن نشد.")


def build_release_plan(config: Config, release: Dict[str, Any]) -> MessagePlan:
    version = _release_version(release)
    title = (release.get("name") or "").strip()
    url = str(release.get("html_url") or config.repo_url)
    published_at = release.get("published_at") or release.get("created_at")
    assets = _release_assets(release)
    prerelease = bool(release.get("prerelease"))
    draft = bool(release.get("draft"))

    if draft:
        raise BotError(
            f"نسخهٔ {version} پیش‌نویس (draft) است و قابل انتشار نیست. "
            "ابتدا آن را در GitHub منتشر کنید."
        )

    total_size = sum(int(asset.get("size") or 0) for asset in assets)
    if assets:
        files_line = (
            f"📦 {T.fa_digits(len(assets))} فایل · {T.format_size(total_size)}"
        )
    else:
        files_line = "📦 فایلی برای این نسخه بارگذاری نشده است"

    badge = "⚠️ نسخهٔ پیش‌انتشار (Beta)" if prerelease else "✅ انتشار پایدار"
    platform = _platform_hint(assets)

    header: List[str] = [
        BRAND,
        DIVIDER,
        "",
        f"🚀 <b>نسخهٔ {T.escape_html(version)} منتشر شد</b>",
        "",
        badge,
        f"🗓 {T.format_jalali_datetime(published_at)}",
        files_line,
    ]
    if platform:
        header.append(platform)
    if title and title.lower() not in {version.lower(), f"packetyellow {version}".lower()}:
        header.append(f"🏷 {T.escape_html(title)}")
    header.extend(["", DIVIDER, "", "📝 <b>تغییرات این نسخه</b>", ""])

    body = release.get("body") or ""
    body = T.drop_duplicate_title(body, version, title)
    notes = T.render_markdown(body)
    while notes and not notes[-1].strip():
        notes.pop()
    if not any(line.strip() for line in notes):
        notes = [
            "یادداشت انتشار برای این نسخه ثبت نشده است.",
            "",
            f'فهرست کامل فایل‌ها و توضیحات را در <a href="{T.escape_attr(url)}">صفحهٔ انتشار</a> ببینید.',
        ]

    footer: List[str] = ["", DIVIDER]
    if assets:
        footer.extend(["", "⬇️ <b>دانلود</b>", ""])
        listed = 0
        for asset in assets:
            name = str(asset.get("name") or "")
            if name.lower().endswith(ASSET_SKIP_SUFFIXES):
                continue
            download_url = asset.get("browser_download_url")
            if not download_url or listed >= 6:
                continue
            footer.append(
                f'• <a href="{T.escape_attr(str(download_url))}">{T.escape_html(name)}</a>'
                f" · {T.format_size(asset.get('size'))}"
            )
            listed += 1
        if len(assets) > listed:
            footer.append(f"• و {T.fa_digits(len(assets) - listed)} فایل دیگر در صفحهٔ انتشار")

    footer.extend(
        [
            "",
            f'🔗 <a href="{T.escape_attr(url)}">صفحهٔ انتشار و همهٔ فایل‌ها</a>',
            "",
            f'🌐 <a href="{T.escape_attr(config.site_url)}">وب‌سایت</a>'
            f' · 💻 <a href="{T.escape_attr(config.repo_url)}">GitHub</a>'
            f' · 📢 <a href="{T.escape_attr(config.channel_url)}">'
            f"{T.escape_html(config.channel_handle)}</a>",
        ]
    )
    tags = " ".join(item for item in ("#PacketYellow", "#Release", T.hashtag_for(version)) if item)
    footer.extend(["", tags])

    parts = assemble_parts(config, header, notes, footer, version, url)
    return MessagePlan(
        parts=parts,
        buttons=build_buttons(config, release),
        preview_url=url,
        version=version,
        assets=assets,
    )


def _platform_hint(assets: Sequence[Dict[str, Any]]) -> str:
    names = " ".join(str(asset.get("name", "")) for asset in assets).lower()
    if not names:
        return ""
    platforms: List[str] = []
    if ".exe" in names or ".msi" in names:
        platforms.append("ویندوز")
    if ".dmg" in names or ".app" in names:
        platforms.append("macOS")
    if ".appimage" in names or ".deb" in names or ".rpm" in names:
        platforms.append("لینوکس")
    if ".apk" in names:
        platforms.append("اندروید")
    if not platforms:
        return ""
    arch = " · ۶۴ بیتی" if ("x64" in names or "amd64" in names) else ""
    return f"🖥 {' / '.join(platforms)}{arch}"


def build_intro_plan(config: Config) -> MessagePlan:
    intro_markdown = f"""\
### ربات رسمی اطلاع‌رسانی PacketYellow

از این پس هر نسخهٔ جدید PacketYellow به‌صورت خودکار همین‌جا منتشر می‌شود؛ با یادداشت تغییرات، فهرست فایل‌ها و لینک دانلود مستقیم.

**این ربات چه می‌کند؟**

- انتشار خودکار نسخه‌های جدید در لحظهٔ انتشار در GitHub
- خلاصه‌ای خوانا از تغییرات هر نسخه
- فهرست فایل‌های نصب به‌همراه حجم و لینک دانلود
- به‌روزرسانی پیام در صورت ویرایش نسخه و حذف پیام در صورت حذف نسخه

**دسترسی سریع**

- وب‌سایت پروژه: [mehdi138iimm.github.io/PacketYellow]({config.site_url})
- مخزن GitHub: [Mehdi138iimm/PacketYellow]({config.repo_url})
- آخرین نسخه: [releases/latest]({config.repo_url}/releases/latest)
- کانال رسمی: [{config.channel_handle}]({config.channel_url})

**نکتهٔ نصب**

فایل `.exe` برای نصب معمولی و فایل `.msi` برای نصب سیستمی/سازمانی مناسب است. توضیحات کامل نصب و راه‌اندازی در صفحهٔ هر نسخه آمده است.

ساخته‌شده با 💛 توسط [Mehdi Jafari](https://github.com/Mehdi138iimm)
"""

    notes = T.render_markdown(intro_markdown)
    header = [
        BRAND,
        DIVIDER,
        "",
        "👋 <b>به کانال رسمی PacketYellow خوش آمدید</b>",
        "",
        "🤖 <b>ربات انتشار نسخه‌ها</b>",
        "",
    ]
    footer = [
        "",
        DIVIDER,
        "",
        f"🌐 <a href=\"{T.escape_attr(config.site_url)}\">وب‌سایت</a>"
        f" · 💻 <a href=\"{T.escape_attr(config.repo_url)}\">GitHub</a>"
        f" · 📢 <a href=\"{T.escape_attr(config.channel_url)}\">"
        f"{T.escape_html(config.channel_handle)}</a>",
        "",
        "#PacketYellow #Release",
    ]
    parts = assemble_parts(config, header, notes, footer, "PacketYellow", config.site_url)
    return MessagePlan(
        parts=parts,
        buttons=build_buttons(config, None),
        preview_url=config.site_url,
        version="intro",
    )


# --------------------------------------------------------------------------- #
# Delivery
# --------------------------------------------------------------------------- #

def _log_plan(plan: MessagePlan) -> None:
    log(f"🧩 پیام در {T.fa_digits(len(plan.parts))} بخش آماده شد "
        f"(طول هر بخش: {', '.join(str(T.telegram_length(part)) for part in plan.parts)} واحد)")


def _prepare_part(text: str) -> Tuple[str, Optional[str]]:
    """Return ``(text, parse_mode)`` that is guaranteed to be accepted.

    A single malformed note must never abort a whole release notification, so a
    part with broken HTML is downgraded to plain text (links stay visible as
    URLs because Telegram auto-links them).
    """
    problems = T.html_problems(text)
    if not problems and T.telegram_length(text) <= T.TELEGRAM_LIMIT:
        return text, "HTML"

    if problems:
        warn(f"HTML پیام نامعتبر بود ({problems[0]}); این بخش به متن ساده تبدیل شد.")
    else:
        warn("طول پیام از سقف تلگرام بیشتر بود؛ این بخش کوتاه‌تر ارسال می‌شود.")

    plain = html.unescape(re.sub(r"<[^>]+>", "", text))
    plain = re.sub(r"\n{3,}", "\n\n", plain).strip()
    if T.telegram_length(plain) > T.TELEGRAM_LIMIT:
        plain = plain[: T.TELEGRAM_LIMIT - 100].rstrip() + "\n…"
    return plain, None


def _send_plan(
    config: Config,
    telegram: TelegramClient,
    plan: MessagePlan,
    *,
    chat: str,
    on_progress: Optional[Callable[[List[int]], None]] = None,
    already_sent: int = 0,
) -> List[int]:
    """Send a plan; ``already_sent`` completes a previously interrupted send."""
    message_ids: List[int] = []
    for index, part in enumerate(plan.parts):
        if index < already_sent:
            continue
        is_last = index == len(plan.parts) - 1
        markup = {"inline_keyboard": plan.buttons} if (is_last and plan.buttons) else None
        text, parse_mode = _prepare_part(part)
        result = telegram.send_message(
            chat,
            text,
            silent=config.silent,
            reply_markup=markup,
            preview_url=plan.preview_url if is_last else None,
            thread_id=config.thread_id,
            parse_mode=parse_mode,
        )
        message_id = int((result or {}).get("message_id") or 0)
        message_ids.append(message_id)
        log(f"  ✅ بخش {T.fa_digits(index + 1)}/{T.fa_digits(len(plan.parts))} ارسال شد "
            f"(message_id={message_id})")
        # Persist after every single message so a crash never loses ids that
        # Telegram already accepted (that used to cause duplicate posts).
        if on_progress:
            on_progress(message_ids)
        if not is_last:
            time.sleep(config.send_interval)
    return message_ids


def _edit_plan(
    config: Config,
    telegram: TelegramClient,
    plan: MessagePlan,
    *,
    chat: str,
    existing: Sequence[int],
    on_progress: Optional[Callable[[List[int]], None]] = None,
) -> List[int]:
    message_ids = list(existing)

    def send_part(index: int, part: str, is_last: bool) -> None:
        markup = {"inline_keyboard": plan.buttons} if (is_last and plan.buttons) else None
        text, parse_mode = _prepare_part(part)
        result = telegram.send_message(
            chat,
            text,
            silent=config.silent,
            reply_markup=markup,
            preview_url=plan.preview_url if is_last else None,
            thread_id=config.thread_id,
            parse_mode=parse_mode,
        )
        new_id = int((result or {}).get("message_id") or 0)
        if index < len(message_ids):
            message_ids[index] = new_id
        else:
            message_ids.append(new_id)
        action = "به‌عنوان ادامه ارسال شد" if index >= len(existing) else "از نو ارسال شد"
        log(f"  ➕ بخش {T.fa_digits(index + 1)} {action} (message_id={new_id})")

    for index, part in enumerate(plan.parts):
        is_last = index == len(plan.parts) - 1
        markup = {"inline_keyboard": plan.buttons} if (is_last and plan.buttons) else None

        if index < len(existing):
            # Telegram keeps the old inline keyboard unless we send an explicit
            # empty one, so a part that lost its buttons must be cleared.
            edit_markup = markup if is_last else {"inline_keyboard": []}
            text, parse_mode = _prepare_part(part)
            try:
                telegram.edit_message_text(
                    chat,
                    int(existing[index]),
                    text,
                    reply_markup=edit_markup,
                    preview_url=plan.preview_url if is_last else None,
                    parse_mode=parse_mode,
                )
                log(f"  ✏️ بخش {T.fa_digits(index + 1)} ویرایش شد (message_id={existing[index]})")
            except TelegramError as exc:
                description = str(exc.description).lower()
                if "not found" not in description and "message_id_invalid" not in description:
                    raise
                # The old post was deleted manually; fall back to a fresh message.
                log(f"  ♻️ پیام قبلی پیدا نشد؛ بخش {T.fa_digits(index + 1)} از نو ارسال می‌شود.")
                send_part(index, part, is_last)
        else:
            send_part(index, part, is_last)

        if on_progress:
            on_progress(message_ids)
        if not is_last:
            time.sleep(config.send_interval)

    for stale_id in existing[len(plan.parts):]:
        telegram.delete_message(chat, int(stale_id))
        log(f"  🗑 بخش اضافی حذف شد (message_id={stale_id})")
        if stale_id in message_ids:
            message_ids.remove(stale_id)
    return message_ids


def _dry_run_plan(plan: MessagePlan, label: str) -> None:
    log(f"🧪 حالت آزمایشی ({label}): هیچ پیامی ارسال نمی‌شود.")
    _log_plan(plan)
    for index, part in enumerate(plan.parts):
        problems = T.html_problems(part)
        if problems:
            warn(f"HTML بخش {T.fa_digits(index + 1)} نامعتبر است؛ در ارسال واقعی به "
                 f"متن ساده تبدیل می‌شود ({problems[0]}).")
        if T.telegram_length(part) > T.TELEGRAM_LIMIT:
            warn(f"بخش {T.fa_digits(index + 1)} از سقف تلگرام بلندتر است.")
        log(f"\n───── پیام {index + 1}/{len(plan.parts)} "
            f"({T.telegram_length(part)} واحد) ─────")
        log(part)
    log("\n───── دکمه‌ها ─────")
    for row in plan.buttons:
        log(" | ".join(f"[{button['text']}]({button['url']})" for button in row))


def _describe_release(release: Dict[str, Any]) -> str:
    version = _release_version(release)
    state = "پیش‌نویس" if release.get("draft") else (
        "پیش‌انتشار" if release.get("prerelease") else "پایدار"
    )
    return f"{version} ({state})"


def publish_release(
    config: Config,
    telegram: TelegramClient,
    state: StateStore,
    release: Dict[str, Any],
    *,
    edit: bool,
) -> int:
    release_id = str(release.get("id") or release.get("tag_name") or "")
    version = _release_version(release)
    title = (release.get("name") or "").strip()
    log(f"📦 ریلیز هدف: {_describe_release(release)} — id={release_id}")

    plan = build_release_plan(config, release)
    _log_plan(plan)

    if config.dry_run:
        _dry_run_plan(plan, "ریلیز")
        return 0

    for chat in config.chats:
        existing = state.messages_for(release_id, chat)
        log(f"💬 مقصد: {chat} — پیام ذخیره‌شده: {existing or 'ندارد'}")

        if existing and not edit and not config.force:
            if len(existing) >= len(plan.parts):
                log("⏭ این نسخه قبلاً در این مقصد ارسال شده است (برای ارسال دوباره: FORCE=true).")
                continue
            # A previous run was interrupted mid-way: complete the missing parts
            # instead of skipping the whole release.
            log(f"↩️ ارسال قبلی ناقص بود ({T.fa_digits(len(existing))} از "
                f"{T.fa_digits(len(plan.parts))} بخش)؛ ادامهٔ ارسال انجام می‌شود.")

            def progress_partial(new_ids: List[int], _chat: str = chat,
                                 prior: List[int] = existing) -> None:
                state.record(
                    release_id, tag=version, title=title, chat=_chat,
                    message_ids=list(prior) + list(new_ids),
                )
                state.save()

            new_ids = _send_plan(
                config, telegram, plan, chat=chat,
                on_progress=progress_partial, already_sent=len(existing),
            )
            message_ids = list(existing) + list(new_ids)
            state.record(release_id, tag=version, title=title, chat=chat,
                         message_ids=message_ids)
            state.save()
            log(f"💾 وضعیت ذخیره شد: {release_id} → {chat} → {message_ids}")
            continue

        if existing and config.force and not edit:
            for message_id in existing:
                telegram.delete_message(chat, message_id)
            existing = []
            state.record(release_id, tag=version, title=title, chat=chat, message_ids=[])
            state.save()

        def progress(message_ids: List[int], _chat: str = chat) -> None:
            state.record(
                release_id, tag=version, title=title, chat=_chat, message_ids=message_ids
            )
            state.save()

        if edit and existing:
            message_ids = _edit_plan(
                config, telegram, plan, chat=chat, existing=existing, on_progress=progress
            )
        else:
            message_ids = _send_plan(config, telegram, plan, chat=chat, on_progress=progress)

        state.record(release_id, tag=version, title=title, chat=chat, message_ids=message_ids)
        state.save()
        log(f"💾 وضعیت ذخیره شد: {release_id} → {chat} → {message_ids}")

    log(f"✅ انتشار «{version}» در تلگرام کامل شد.")
    _write_publish_summary(config, release, state)
    return 0


def _write_publish_summary(config: Config, release: Dict[str, Any], state: StateStore) -> None:
    release_id = str(release.get("id") or release.get("tag_name") or "")
    entry = state.release_entry(release_id) or {}
    rows = ["## 📢 PacketYellow → Telegram", "", "| نسخه | مقصد | پیام‌ها |", "|---|---|---|"]
    for chat, info in (entry.get("chats") or {}).items():
        ids = ", ".join(str(item) for item in info.get("message_ids") or []) or "—"
        rows.append(f"| `{_release_version(release)}` | `{chat}` | {ids} |")
    rows.extend(
        [
            "",
            f"- پیام‌ها: {T.fa_digits(len(entry.get('chats') or {}))} مقصد",
            f"- کانال: {config.channel_handle}",
            f"- صفحهٔ انتشار: {release.get('html_url') or config.repo_url}",
        ]
    )
    write_step_summary("\n".join(rows))


def delete_release(
    config: Config,
    telegram: TelegramClient,
    state: StateStore,
    release: Dict[str, Any],
) -> int:
    release_id = str(release.get("id") or release.get("tag_name") or "")
    log(f"🗑 حذف پیام‌های ریلیز id={release_id}")

    entry = state.release_entry(release_id)
    if not entry:
        log("ℹ️ پیامی برای این ریلیز ذخیره نشده است؛ کاری لازم نیست.")
        return 0

    chats = dict(entry.get("chats") or {})
    if config.dry_run:
        log(f"🧪 حالت آزمایشی: این پیام‌ها حذف می‌شدند: {chats}")
        return 0

    for chat, info in chats.items():
        for message_id in info.get("message_ids") or []:
            telegram.delete_message(chat, int(message_id))
            log(f"  🗑 پیام {message_id} در {chat} حذف شد")

    state.remove_release(release_id)
    state.save()
    log("✅ پیام‌های ریلیز حذف شدند.")
    return 0


def publish_intro(
    config: Config,
    telegram: TelegramClient,
    intro_state: IntroState,
) -> int:
    if intro_state.sent and not config.force:
        log("ℹ️ پیام معرفی قبلاً ارسال شده است (برای ارسال دوباره: FORCE=true).")
        return 0

    plan = build_intro_plan(config)
    _log_plan(plan)
    if config.dry_run:
        _dry_run_plan(plan, "معرفی")
        return 0

    if intro_state.sent and config.force:
        # Remove the previous introduction so a forced resend does not leave two.
        for chat, info in (intro_state.data.get("chats") or {}).items():
            for message_id in (info or {}).get("message_ids") or []:
                telegram.delete_message(chat, int(message_id))
                log(f"  🗑 معرفی قبلی حذف شد ({chat} → {message_id})")

    for chat in config.chats:
        def progress(message_ids: List[int], _chat: str = chat) -> None:
            intro_state.mark(_chat, message_ids)
            intro_state.save()

        message_ids = _send_plan(config, telegram, plan, chat=chat, on_progress=progress)
        intro_state.mark(chat, message_ids)
        intro_state.save()
        log(f"💾 وضعیت معرفی ذخیره شد: {chat} → {message_ids}")
    log("✅ پیام معرفی ارسال شد.")
    return 0


# --------------------------------------------------------------------------- #
# Doctor: explain the current configuration in plain Persian
# --------------------------------------------------------------------------- #

def run_doctor(config: Config) -> int:
    lines: List[str] = ["## 🩺 وضعیت ربات تلگرام PacketYellow", ""]
    failures = 0
    warnings = 0

    def annotate(level: str, title: str, detail: str) -> None:
        """Surface a check result as a GitHub Actions annotation (no logs needed)."""
        if os.environ.get("GITHUB_ACTIONS") != "true":
            return
        summary = " ".join(str(detail).split())
        print(f"::{level} title={title}::{summary}", flush=True)

    def check(ok: bool, title: str, detail: str, *, critical: bool = True) -> None:
        nonlocal failures, warnings
        if ok:
            icon = "✅"
        elif critical:
            icon = "❌"
            failures += 1
            annotate("error", title, detail)
        else:
            icon = "⚠️"
            warnings += 1
            annotate("warning", title, detail)
        lines.append(f"- {icon} **{title}** — {detail}")
        log(f"{icon} {title}: {detail}")

    check(bool(config.token), "توکن ربات (TG_TOKEN)", "تنظیم شده" if config.token else "تنظیم نشده")
    check(bool(config.chats), "مقصد (TG_CHAT)", ", ".join(config.chats) if config.chats else "تنظیم نشده")
    check(
        bool(config.github_token),
        "توکن GitHub (GH_TOKEN)",
        "تنظیم شده" if config.github_token else "تنظیم نشده (اختیاری؛ فقط سهمیهٔ API بیشتر می‌شود)",
        critical=False,
    )

    if config.token:
        try:
            telegram = TelegramClient(config.token, config.telegram_api)
            me = telegram.get_me()
            check(True, "اتصال به Telegram", f"ربات @{me.get('username')} (id={me.get('id')})")
            lines.append(f"- 🤖 ربات: @{me.get('username')} — {me.get('first_name', '')}")
            for chat in config.chats:
                try:
                    info = telegram.get_chat(chat)
                    chat_type = info.get("type", "?")
                    title = info.get("title") or info.get("username") or chat
                    members = "?"
                    try:
                        members = str(telegram.get_chat_member_count(chat))
                    except TelegramError:
                        pass
                    check(True, f"چت {chat}", f"{title} · نوع: {chat_type} · اعضا: {members}")
                    if chat_type == "channel":
                        try:
                            member = telegram.get_chat_member(chat, int(me["id"]))
                            status = member.get("status")
                            can_post = member.get("can_post_messages")
                            ok = status == "administrator" and can_post is not False
                            check(
                                ok,
                                f"دسترسی ربات در {chat}",
                                f"وضعیت: {status}"
                                + ("" if ok else " — ربات را ادمین کانال با اجازهٔ ارسال پیام کنید"),
                            )
                        except TelegramError as exc:
                            check(False, f"دسترسی ربات در {chat}", str(exc.description))
                    elif chat_type in {"group", "supergroup"}:
                        try:
                            member = telegram.get_chat_member(chat, int(me["id"]))
                            status = member.get("status")
                            ok = status in {"administrator", "creator", "member"}
                            check(ok, f"عضویت ربات در {chat}", f"وضعیت: {status}")
                        except TelegramError as exc:
                            check(False, f"عضویت ربات در {chat}", str(exc.description))
                except TelegramError as exc:
                    check(False, f"چت {chat}", str(exc.description))
        except (TelegramError, BotError) as exc:
            check(False, "اتصال به Telegram", str(exc))

    try:
        github = GitHubClient(config.repo, config.github_token, config.github_api)
        latest = github.get_latest_release(include_prerelease=True)
        check(True, "GitHub API", f"ریپازیتوری {config.repo} · آخرین نسخه: {_release_version(latest)}")
    except BotError as exc:
        check(False, "GitHub API", str(exc))

    if not failures:
        lines.extend(["", "🎉 همهٔ بررسی‌ها موفق بود؛ ربات آمادهٔ ارسال است."])
    else:
        lines.extend(["", "⚠️ موارد علامت‌خورده را رفع کنید و دوباره doctor را اجرا کنید."])
    if warnings:
        lines.append(f"ℹ️ {T.fa_digits(warnings)} مورد هشدار (اختیاری) وجود دارد.")

    write_step_summary("\n".join(lines))
    log("\n" + "\n".join(line for line in lines if line.startswith("- ")))
    return 0 if failures == 0 else 1


# --------------------------------------------------------------------------- #
# Release resolution (event payload / GitHub API)
# --------------------------------------------------------------------------- #

def read_event(config: Config) -> Tuple[str, Optional[Dict[str, Any]]]:
    path = config.event_path
    if not path or not Path(path).exists():
        return "", None
    try:
        payload = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        warn(f"خواندن فایل رویداد ناموفق بود: {exc}")
        return "", None
    action = str(payload.get("action") or "")
    release = payload.get("release")
    return action, (release if isinstance(release, dict) else None)


def resolve_release(
    config: Config,
    action: str,
    event_release: Optional[Dict[str, Any]],
) -> Tuple[str, Optional[Dict[str, Any]]]:
    """Return ``(action, release)`` where action is publish/edit/delete/skip."""
    github: Optional[GitHubClient] = None

    def fetch() -> GitHubClient:
        nonlocal github
        if github is None:
            github = GitHubClient(config.repo, config.github_token, config.github_api)
        return github

    if config.mode == "auto":
        if event_release is not None and action:
            if event_release.get("draft") and action != "deleted":
                # GitHub fires "edited" for draft releases too; there is nothing
                # published to announce yet.
                log("ℹ️ این نسخه پیش‌نویس (draft) است؛ پیامی ارسال نمی‌شود.")
                return "skip", None
            if action in {"published", "prereleased", "released"}:
                return "publish", event_release
            if action == "edited":
                # The payload can be older than the queued run (concurrency
                # collapses bursts), so re-read the release when possible.
                tag = str(event_release.get("tag_name") or "")
                if tag:
                    try:
                        fresh = fetch().get_release_by_tag(tag)
                        if isinstance(fresh, dict) and not fresh.get("draft"):
                            return "edit", fresh
                    except BotError as exc:
                        log(f"ℹ️ خواندن نسخهٔ به‌روز از GitHub ممکن نشد ({exc}); از دادهٔ رویداد استفاده می‌شود.")
                return "edit", event_release
            if action == "deleted":
                return "delete", event_release
            log(f"ℹ️ رویداد «{action}» نیازی به ارسال پیام ندارد.")
            return "skip", None
        log("ℹ️ رویداد ریلیز در دسترس نیست؛ آخرین نسخه ارسال می‌شود.")
        return "publish", fetch().get_latest_release(include_prerelease=True)

    if config.mode == "latest":
        return "publish", fetch().get_latest_release(include_prerelease=True)

    if config.mode == "tag":
        if not config.tag:
            raise BotError("برای حالت tag باید مقدار TAG (مثلاً v1.7.0) را وارد کنید.")
        return "publish", fetch().get_release_by_tag(config.tag)

    raise BotError(f"حالت «{config.mode}» در این مسیر پشتیبانی نمی‌شود.")


# --------------------------------------------------------------------------- #
# Entry point
# --------------------------------------------------------------------------- #

def run(argv: Optional[Sequence[str]] = None) -> int:
    config = load_config(argv)
    log(f"🤖 PacketYellow Telegram Bot — mode={config.mode} dry_run={config.dry_run} "
        f"repo={config.repo} chats={config.chats or '—'}")

    if config.mode == "doctor":
        return run_doctor(config)

    telegram = TelegramClient(config.token, config.telegram_api) if config.token else None
    state = StateStore(config.state_file)
    intro_state = IntroState(config.intro_state_file)

    if config.mode == "intro":
        if telegram is None and not config.dry_run:
            raise BotError("توکن ربات برای ارسال معرفی لازم است (TG_TOKEN).")
        return publish_intro(config, telegram, intro_state)  # type: ignore[arg-type]

    action, event_release = read_event(config)
    action, release = resolve_release(config, action, event_release)
    if action == "skip" or release is None:
        return 0

    if config.dry_run and telegram is None:
        telegram = None
    elif telegram is None:
        raise BotError("توکن ربات برای ارسال لازم است (TG_TOKEN).")

    if action == "delete":
        return delete_release(config, telegram, state, release)  # type: ignore[arg-type]
    return publish_release(
        config, telegram, state, release, edit=(action == "edit")  # type: ignore[arg-type]
    )


def main() -> None:
    try:
        sys.exit(run())
    except BotError as exc:
        print(f"❌ {exc}", flush=True)
        if os.environ.get("GITHUB_ACTIONS") == "true":
            print(f"::error::{str(exc).splitlines()[0]}")
        sys.exit(2)
    except KeyboardInterrupt:
        print("متوقف شد.", flush=True)
        sys.exit(130)
    except Exception as exc:  # noqa: BLE001 - last-resort friendly error
        import traceback

        traceback.print_exc()
        print(f"❌ خطای غیرمنتظره: {exc}", flush=True)
        if os.environ.get("GITHUB_ACTIONS") == "true":
            print(f"::error::خطای غیرمنتظره در ربات تلگرام: {type(exc).__name__}")
        sys.exit(1)


if __name__ == "__main__":
    main()
