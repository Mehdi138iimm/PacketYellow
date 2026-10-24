"""Test support: an in-process HTTP server emulating the Telegram Bot API and
the GitHub REST API, used by the end-to-end bot tests.

The server validates the payloads the same way Telegram does for the checks we
care about (HTML entities, 4096 character limit, message ids, edit/delete
semantics) and records every call so tests can assert on them.
"""

from __future__ import annotations

import json
import re
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

import tg_text  # noqa: E402


class MockAPI:
    """Threaded fake of api.telegram.org + api.github.com."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.telegram_calls: List[Tuple[str, Dict[str, Any]]] = []
        self.messages: Dict[int, Dict[str, Any]] = {}
        self.next_message_id = 1000
        self.bad_token = False
        self.chat_missing = False
        self.chat_member: Dict[str, Any] = {
            "status": "administrator",
            "can_post_messages": True,
        }
        self.scripted_failures: Dict[str, List[Tuple[int, str, Dict[str, Any]]]] = {}
        self.fail_send_after: Optional[int] = None
        self.send_count = 0
        self.github_releases: List[Dict[str, Any]] = []
        self.github_fail = False
        self._server: Optional[ThreadingHTTPServer] = None
        self._thread: Optional[threading.Thread] = None

    # -- lifecycle ---------------------------------------------------------- #
    def start(self) -> "MockAPI":
        api = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: Any) -> None:  # silence
                pass

            def _read_json(self) -> Dict[str, Any]:
                length = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(length) if length else b"{}"
                try:
                    return json.loads(raw.decode("utf-8") or "{}")
                except ValueError:
                    return {}

            def _send(self, code: int, payload: Any) -> None:
                body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self) -> None:  # noqa: N802
                parsed = urllib.parse.urlparse(self.path)
                path = parsed.path
                if path.startswith("/repos/"):
                    api._handle_github(self, path)
                    return
                self._send(404, {"message": "Not Found"})

            def do_POST(self) -> None:  # noqa: N802
                parsed = urllib.parse.urlparse(self.path)
                head = re.match(r"^/bot([^/]+)/([A-Za-z]+)$", parsed.path)
                payload = self._read_json()
                if not head:
                    self._send(404, {"message": "Not Found"})
                    return
                api._handle_telegram(self, head.group(1), head.group(2), payload)

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None

    @property
    def url(self) -> str:
        assert self._server is not None
        return f"http://127.0.0.1:{self._server.server_address[1]}"

    # -- helpers ------------------------------------------------------------ #
    def script_failure(self, method: str, code: int, description: str,
                       parameters: Optional[Dict[str, Any]] = None) -> None:
        self.scripted_failures.setdefault(method, []).append(
            (code, description, parameters or {})
        )

    def sent_messages(self) -> List[Dict[str, Any]]:
        return [message for message in self.messages.values() if message.get("alive")]

    def calls(self, method: str) -> List[Dict[str, Any]]:
        return [payload for name, payload in self.telegram_calls if name == method]

    # -- Telegram ----------------------------------------------------------- #
    def _handle_telegram(self, handler: BaseHTTPRequestHandler, token: str,
                         method: str, payload: Dict[str, Any]) -> None:
        with self.lock:
            self.telegram_calls.append((method, payload))

            queue = self.scripted_failures.get(method) or []
            if queue:
                code, description, parameters = queue.pop(0)
                handler._send(code, {
                    "ok": False,
                    "error_code": code,
                    "description": description,
                    "parameters": parameters,
                })
                return

            if self.bad_token or token in {"", "BAD"}:
                handler._send(401, {
                    "ok": False,
                    "error_code": 401,
                    "description": "Unauthorized",
                })
                return

            if method == "getMe":
                handler._send(200, {"ok": True, "result": {
                    "id": 999,
                    "is_bot": True,
                    "first_name": "PacketYellow",
                    "username": "packetyellow_bot",
                }})
                return

            if method == "getChat":
                chat = str(payload.get("chat_id", ""))
                if self.chat_missing or not self._known_chat(chat):
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: chat not found",
                    })
                    return
                handler._send(200, {"ok": True, "result": {
                    "id": -1001234567890,
                    "type": "channel" if chat.startswith("@") else "supergroup",
                    "title": "PacketYellow",
                    "username": chat.lstrip("@") or "PacketYellow",
                }})
                return

            if method == "getChatMember":
                if self.chat_missing or not self._known_chat(str(payload.get("chat_id", ""))):
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: chat not found",
                    })
                    return
                handler._send(200, {"ok": True, "result": self.chat_member})
                return

            if method == "getChatMemberCount":
                handler._send(200, {"ok": True, "result": 42})
                return

            if method == "sendMessage":
                self.send_count += 1
                if self.fail_send_after is not None and self.send_count > self.fail_send_after:
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: simulated delivery failure",
                    })
                    return
                self._send_message(handler, payload)
                return

            if method == "editMessageText":
                message_id = int(payload.get("message_id") or 0)
                message = self.messages.get(message_id)
                if message is None or not message.get("alive"):
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: message to edit not found",
                    })
                    return
                text = str(payload.get("text") or "")
                if text == message["text"]:
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: message is not modified",
                    })
                    return
                problems = tg_text.html_problems(text)
                if problems:
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: can't parse entities: " + problems[0],
                    })
                    return
                message["text"] = text
                if "reply_markup" in payload:
                    # Telegram only replaces the keyboard when the field is sent.
                    message["markup"] = payload.get("reply_markup")
                handler._send(200, {"ok": True, "result": {"message_id": message_id}})
                return

            if method == "deleteMessage":
                message_id = int(payload.get("message_id") or 0)
                message = self.messages.get(message_id)
                if message is None or not message.get("alive"):
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: message to delete not found",
                    })
                    return
                message["alive"] = False
                handler._send(200, {"ok": True, "result": True})
                return

            handler._send(404, {"ok": False, "error_code": 404,
                                "description": "Not Found: method not found"})

    def _known_chat(self, chat: str) -> bool:
        return chat in {"@PacketYellow", "-1001234567890", "@packetyellow_test"}

    def _send_message(self, handler: BaseHTTPRequestHandler,
                      payload: Dict[str, Any]) -> None:
        chat = str(payload.get("chat_id") or "")
        text = str(payload.get("text") or "")
        if self.chat_missing or not self._known_chat(chat):
            handler._send(400, {
                "ok": False, "error_code": 400,
                "description": "Bad Request: chat not found",
            })
            return
        if self.chat_member.get("status") not in {"administrator", "creator", "member"}:
            handler._send(403, {
                "ok": False, "error_code": 403,
                "description": "Forbidden: bot is not a member of the channel chat",
            })
            return
        if self.chat_member.get("status") == "administrator" and self.chat_member.get(
            "can_post_messages"
        ) is False:
            handler._send(403, {
                "ok": False, "error_code": 403,
                "description": "Forbidden: not enough rights to send text messages",
            })
            return
        if payload.get("parse_mode") not in (None, "HTML"):
            handler._send(400, {
                "ok": False, "error_code": 400,
                "description": "Bad Request: parse_mode must be HTML or omitted",
            })
            return
        if tg_text.telegram_length(text) > tg_text.TELEGRAM_LIMIT:
            handler._send(400, {
                "ok": False, "error_code": 400,
                "description": f"Bad Request: message is too long "
                               f"({tg_text.telegram_length(text)} > 4096)",
            })
            return
        problems = [] if payload.get("parse_mode") is None else tg_text.html_problems(text)
        if problems:
            handler._send(400, {
                "ok": False, "error_code": 400,
                "description": "Bad Request: can't parse entities: " + problems[0],
            })
            return
        markup = payload.get("reply_markup") or {}
        for row in markup.get("inline_keyboard") or []:
            for button in row:
                if len(button.get("text", "").encode("utf-8")) > 64:
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: BUTTON_TEXT_INVALID",
                    })
                    return
                url = button.get("url", "")
                if not url.startswith(("http://", "https://", "tg://")):
                    handler._send(400, {
                        "ok": False, "error_code": 400,
                        "description": "Bad Request: BUTTON_URL_INVALID",
                    })
                    return

        self.next_message_id += 1
        message_id = self.next_message_id
        self.messages[message_id] = {
            "message_id": message_id,
            "chat": chat,
            "text": text,
            "alive": True,
            "markup": payload.get("reply_markup"),
            "preview": payload.get("link_preview_options"),
            "silent": payload.get("disable_notification"),
        }
        handler._send(200, {"ok": True, "result": {
            "message_id": message_id,
            "chat": {"id": -1001234567890, "type": "channel"},
            "date": 1761340000,
            "text": text,
        }})

    # -- GitHub ------------------------------------------------------------- #
    def _handle_github(self, handler: BaseHTTPRequestHandler, path: str) -> None:
        if self.github_fail:
            handler._send(500, {"message": "boom"})
            return
        releases = self.github_releases
        tag_match = re.match(r"^/repos/[^/]+/[^/]+/releases/tags/(.+)$", path)
        if tag_match:
            tag = urllib.parse.unquote(tag_match.group(1))
            for release in releases:
                if release.get("tag_name") == tag:
                    handler._send(200, release)
                    return
            handler._send(404, {"message": "Not Found"})
            return
        if path.endswith("/releases") or re.search(r"/releases\?", path):
            handler._send(200, releases)
            return
        handler._send(404, {"message": "Not Found"})
