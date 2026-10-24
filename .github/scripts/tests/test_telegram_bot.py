"""End-to-end and unit tests for the PacketYellow Telegram bot.

The end-to-end tests start :class:`support.MockAPI` (a fake Telegram + GitHub),
run ``telegram_bot.py`` as a subprocess exactly like GitHub Actions does, and
assert on the HTTP payloads, the persisted state file and the exit code.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any, Dict, List

TESTS_DIR = Path(__file__).resolve().parent
SCRIPTS_DIR = TESTS_DIR.parent
REPO_ROOT = TESTS_DIR.parents[2]
FIXTURES = TESTS_DIR / "fixtures"
BOT_SCRIPT = SCRIPTS_DIR / "telegram_bot.py"

sys.path.insert(0, str(SCRIPTS_DIR))
sys.path.insert(0, str(TESTS_DIR))

import support  # noqa: E402
import telegram_bot as bot  # noqa: E402
import tg_text as T  # noqa: E402

ENV_KEYS_TO_CLEAR = (
    "TG_TOKEN", "TG_CHAT", "CHAT_OVERRIDE", "MODE", "TAG", "FORCE", "DRY_RUN",
    "SILENT", "THREAD_ID", "GH_TOKEN", "REPOSITORY", "TELEGRAM_API_BASE",
    "GITHUB_API_BASE", "GITHUB_EVENT_PATH", "GITHUB_STEP_SUMMARY", "SITE_URL",
    "CHANNEL_URL", "CHANNEL_HANDLE", "REPO_URL", "STATE_FILE", "INTRO_STATE_FILE",
    "SEND_INTERVAL", "MESSAGE_BUDGET", "MAX_PARTS", "GITHUB_ACTIONS",
)


def load_release(tag: str) -> Dict[str, Any]:
    return json.loads((FIXTURES / f"release-{tag}.json").read_text(encoding="utf-8"))


class BotTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self.api = support.MockAPI().start()
        self.addCleanup(self.api.stop)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.tmp_path = Path(self.tmp.name)
        self.state_file = self.tmp_path / "telegram-message-map.json"
        self.intro_state_file = self.tmp_path / "telegram-intro-state.json"
        self.event_file = self.tmp_path / "event.json"
        self.summary_file = self.tmp_path / "summary.md"

        self.base_env = {key: value for key, value in os.environ.items() if key not in ENV_KEYS_TO_CLEAR}
        self.base_env.update(
            {
                "TG_TOKEN": "12345:TEST-TOKEN",
                "TG_CHAT": "@PacketYellow",
                "TELEGRAM_API_BASE": self.api.url,
                "GITHUB_API_BASE": self.api.url,
                "GITHUB_STEP_SUMMARY": str(self.summary_file),
                "SEND_INTERVAL": "0",
                "PYTHONIOENCODING": "utf-8",
                "PYTHONUTF8": "1",
            }
        )

    # -- helpers ------------------------------------------------------------ #
    def write_event(self, action: str, release: Dict[str, Any]) -> str:
        self.event_file.write_text(
            json.dumps({"action": action, "release": release}, ensure_ascii=False),
            encoding="utf-8",
        )
        return str(self.event_file)

    def run_bot(self, *args: str, env: Dict[str, str] | None = None,
                event_path: str | None = None) -> subprocess.CompletedProcess:
        environment = dict(self.base_env)
        environment.pop("GITHUB_EVENT_PATH", None)
        if event_path:
            environment["GITHUB_EVENT_PATH"] = event_path
        environment.update(env or {})
        command = [
            sys.executable,
            str(BOT_SCRIPT),
            "--state-file", str(self.state_file),
            "--intro-state-file", str(self.intro_state_file),
            *args,
        ]
        return subprocess.run(
            command,
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=environment,
            timeout=120,
        )

    def assert_ok(self, proc: subprocess.CompletedProcess) -> None:
        self.assertEqual(
            proc.returncode, 0,
            f"exit={proc.returncode}\nstdout:\n{proc.stdout}\nstderr:\n{proc.stderr}",
        )

    def state(self) -> Dict[str, Any]:
        if not self.state_file.exists():
            return {}
        return json.loads(self.state_file.read_text(encoding="utf-8"))

    def assert_html_valid(self, text: str) -> None:
        self.assertEqual(T.html_problems(text), [], text)
        self.assertLessEqual(len(text), T.TELEGRAM_LIMIT)


class PublishTests(BotTestCase):
    def test_publish_short_release(self) -> None:
        release = load_release("v1.7.0")
        self.api.github_releases = [release]
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assert_ok(proc)

        messages = self.api.sent_messages()
        self.assertEqual(len(messages), 1)
        message = messages[0]
        text = message["text"]
        self.assert_html_valid(text)
        for expected in (
            "PacketYellow", "v1.7.0", "انتشار پایدار", "تغییرات این نسخه",
            "۳ آبان ۱۴۰۵", "صفحهٔ انتشار", "#v1_7_0",
            "PacketYellow_1.7.0_x64-setup.exe", "mehdi138iimm.github.io",
        ):
            self.assertIn(expected, text)
        self.assertTrue(message["markup"])
        rows = message["markup"]["inline_keyboard"]
        self.assertTrue(rows)
        for row in rows:
            self.assertTrue(1 <= len(row) <= 8)
            for button in row:
                self.assertLessEqual(len(button["text"].encode("utf-8")), 64)
                self.assertTrue(button["url"].startswith("http"))
        self.assertEqual(message["preview"]["url"], release["html_url"])

        state = self.state()
        self.assertEqual(state["version"], 2)
        entry = state["releases"][str(release["id"])]
        self.assertEqual(entry["chats"]["@PacketYellow"]["message_ids"], [message["message_id"]])

    def test_rerun_does_not_duplicate(self) -> None:
        release = load_release("v1.7.0")
        event_path = self.write_event("published", release)
        self.assert_ok(self.run_bot(event_path=event_path))
        self.assert_ok(self.run_bot(event_path=event_path))
        self.assertEqual(len(self.api.sent_messages()), 1)
        self.assertEqual(self.api.calls("sendMessage"), [self.api.calls("sendMessage")[0]])

    def test_force_resends(self) -> None:
        release = load_release("v1.7.0")
        event_path = self.write_event("published", release)
        self.assert_ok(self.run_bot(event_path=event_path))
        first_id = self.api.sent_messages()[0]["message_id"]
        self.assert_ok(self.run_bot(event_path=event_path, env={"FORCE": "true"}))
        alive = self.api.sent_messages()
        self.assertEqual(len(alive), 1)
        self.assertNotEqual(alive[0]["message_id"], first_id)
        self.assertFalse(self.api.messages[first_id]["alive"])

    def test_long_release_is_split_into_valid_parts(self) -> None:
        release = load_release("v0.6.0")
        self.api.github_releases = [release]
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assert_ok(proc)

        messages = self.api.sent_messages()
        self.assertGreaterEqual(len(messages), 2)
        for index, message in enumerate(messages):
            self.assert_html_valid(message["text"])
            if index > 0:
                self.assertIn("ادامهٔ تغییرات", message["text"])
            if index < len(messages) - 1:
                self.assertNotIn("صفحهٔ انتشار و همهٔ فایل‌ها", message["text"])
        self.assertIn("صفحهٔ انتشار و همهٔ فایل‌ها", messages[-1]["text"])
        self.assertTrue(messages[-1]["markup"])
        self.assertFalse(messages[0]["markup"])
        state = self.state()
        ids = state["releases"][str(release["id"])]["chats"]["@PacketYellow"]["message_ids"]
        self.assertEqual(ids, [message["message_id"] for message in messages])

    def test_empty_release_notes_get_placeholder(self) -> None:
        release = dict(load_release("v1.7.0"))
        release["id"] = 999001
        release["body"] = ""
        self.api.github_releases = [release]
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assert_ok(proc)
        text = self.api.sent_messages()[0]["text"]
        self.assertIn("یادداشت انتشار برای این نسخه ثبت نشده است.", text)
        self.assertNotIn("این یک پیام آزمایشی", text)

    def test_prerelease_badge(self) -> None:
        release = dict(load_release("v1.7.0"))
        release["id"] = 999002
        release["prerelease"] = True
        self.api.github_releases = [release]
        proc = self.run_bot(event_path=self.write_event("prereleased", release))
        self.assert_ok(proc)
        self.assertIn("پیش‌انتشار", self.api.sent_messages()[0]["text"])

    def test_unknown_event_action_is_ignored(self) -> None:
        release = load_release("v1.7.0")
        proc = self.run_bot(event_path=self.write_event("created", release))
        self.assert_ok(proc)
        self.assertEqual(self.api.calls("sendMessage"), [])

    def test_two_chats_get_two_messages(self) -> None:
        release = load_release("v1.7.0")
        self.api.github_releases = [release]
        proc = self.run_bot(
            event_path=self.write_event("published", release),
            env={"TG_CHAT": "@PacketYellow,-1001234567890"},
        )
        self.assert_ok(proc)
        self.assertEqual(len(self.api.sent_messages()), 2)

    def test_release_event_payload_without_github_api_still_works(self) -> None:
        # The event carries the full release payload; the bot must not depend on
        # the GitHub API being reachable during a release event.
        self.api.github_fail = True
        release = load_release("v1.7.0")
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assert_ok(proc)
        self.assertEqual(len(self.api.sent_messages()), 1)


class EditAndDeleteTests(BotTestCase):
    def _publish_long(self) -> Dict[str, Any]:
        release = load_release("v0.6.0")
        self.api.github_releases = [release]
        self.assert_ok(self.run_bot(event_path=self.write_event("published", release)))
        self.assertGreaterEqual(len(self.api.sent_messages()), 2)
        return release

    def test_interrupted_send_is_completed_on_rerun(self) -> None:
        release = load_release("v0.6.0")
        self.api.github_releases = [release]
        event_path = self.write_event("published", release)

        self.api.fail_send_after = 1  # first part succeeds, second one fails
        first = self.run_bot(event_path=event_path)
        self.assertEqual(first.returncode, 2)
        state = self.state()
        ids = state["releases"][str(release["id"])]["chats"]["@PacketYellow"]["message_ids"]
        self.assertEqual(len(ids), 1)

        self.api.fail_send_after = None
        second = self.run_bot(event_path=event_path)
        self.assert_ok(second)
        self.assertEqual(len(self.api.sent_messages()), 3)
        state = self.state()
        ids = state["releases"][str(release["id"])]["chats"]["@PacketYellow"]["message_ids"]
        self.assertEqual(len(ids), 3)

    def test_edit_recreates_message_deleted_by_hand(self) -> None:
        release = load_release("v1.7.0")
        self.api.github_releases = [release]
        self.assert_ok(self.run_bot(event_path=self.write_event("published", release)))
        original = self.api.sent_messages()[0]
        self.api.messages[original["message_id"]]["alive"] = False  # deleted in Telegram

        edited = dict(release)
        edited["body"] = "## PacketYellow v1.7.0\n\n- متن تازه برای ویرایش"
        self.api.github_releases = [edited]  # the edit event re-reads GitHub
        proc = self.run_bot(event_path=self.write_event("edited", edited))
        self.assert_ok(proc)

        alive = self.api.sent_messages()
        self.assertEqual(len(alive), 1)
        self.assertNotEqual(alive[0]["message_id"], original["message_id"])
        self.assertIn("متن تازه برای ویرایش", alive[0]["text"])
        ids = self.state()["releases"][str(release["id"])]["chats"]["@PacketYellow"]["message_ids"]
        self.assertEqual(ids, [alive[0]["message_id"]])

    def test_draft_edit_is_skipped(self) -> None:
        release = dict(load_release("v1.7.0"))
        release["draft"] = True
        proc = self.run_bot(event_path=self.write_event("edited", release))
        self.assert_ok(proc)
        self.assertEqual(self.api.telegram_calls, [])
        self.assertIn("پیش‌نویس", proc.stdout)

    def test_server_error_on_send_is_not_retried(self) -> None:
        # Retrying sendMessage after a 5xx could post twice; the bot must fail
        # loudly instead of duplicating the release announcement.
        self.api.script_failure("sendMessage", 500, "Internal Server Error")
        release = load_release("v1.7.0")
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assertEqual(proc.returncode, 2)
        self.assertEqual(len(self.api.calls("sendMessage")), 1)

    def test_invalid_site_url_is_rejected(self) -> None:
        release = load_release("v1.7.0")
        proc = self.run_bot(
            event_path=self.write_event("published", release),
            env={"SITE_URL": "ftp://example.com/"},
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIn("SITE_URL", proc.stdout)

    def test_edit_that_grows_parts_clears_old_keyboard(self) -> None:
        short = load_release("v1.7.0")
        self.api.github_releases = [short]
        self.assert_ok(self.run_bot(event_path=self.write_event("published", short)))
        first = self.api.sent_messages()[0]
        self.assertTrue(first["markup"], "first post should carry buttons")

        grown = dict(short)
        grown["body"] = load_release("v0.6.0")["body"]
        self.api.github_releases = [grown]

        proc = self.run_bot(event_path=self.write_event("edited", short))
        self.assert_ok(proc)
        alive = self.api.sent_messages()
        self.assertGreaterEqual(len(alive), 2)
        edited_first = self.api.messages[first["message_id"]]
        self.assertEqual(edited_first["markup"], {"inline_keyboard": []})
        self.assertTrue(alive[-1]["markup"])

    def test_release_edit_updates_and_trims_parts(self) -> None:
        release = self._publish_long()
        old_messages = self.api.sent_messages()
        edited = dict(release)
        edited["body"] = "## PacketYellow v0.6.0\n\n- فقط یک تغییر کوچک"
        self.api.github_releases = [edited]  # the edit event re-reads GitHub
        proc = self.run_bot(event_path=self.write_event("edited", edited))
        self.assert_ok(proc)

        alive = self.api.sent_messages()
        self.assertEqual(len(alive), 1)
        self.assertEqual(alive[0]["message_id"], old_messages[0]["message_id"])
        self.assertIn("فقط یک تغییر کوچک", alive[0]["text"])
        self.assertNotIn("بازطراحی کامل رابط کاربری", alive[0]["text"])
        state = self.state()
        ids = state["releases"][str(release["id"])]["chats"]["@PacketYellow"]["message_ids"]
        self.assertEqual(ids, [old_messages[0]["message_id"]])

    def test_release_delete_removes_messages(self) -> None:
        release = load_release("v1.7.0")
        self.api.github_releases = [release]
        self.assert_ok(self.run_bot(event_path=self.write_event("published", release)))
        self.assert_ok(self.run_bot(event_path=self.write_event("deleted", release)))
        self.assertEqual(self.api.sent_messages(), [])
        self.assertEqual(self.state()["releases"], {})

    def test_legacy_state_file_is_migrated_for_delete(self) -> None:
        self.state_file.write_text(
            json.dumps({"123": {"tag": "v1.0.0", "chat_id": "-1001234567890",
                                "message_id": 555}}),
            encoding="utf-8",
        )
        self.api.messages[555] = {
            "message_id": 555, "chat": "-1001234567890", "text": "old", "alive": True,
        }
        release = {"id": 123, "tag_name": "v1.0.0", "name": "v1.0.0"}
        proc = self.run_bot(event_path=self.write_event("deleted", release))
        self.assert_ok(proc)
        self.assertFalse(self.api.messages[555]["alive"])


class DryRunTests(BotTestCase):
    def test_dry_run_sends_nothing_and_writes_no_state(self) -> None:
        release = load_release("v0.6.0")
        proc = self.run_bot(
            "--dry-run",
            event_path=self.write_event("published", release),
            env={"TG_TOKEN": ""},
        )
        self.assert_ok(proc)
        self.assertEqual(self.api.telegram_calls, [])
        self.assertFalse(self.state_file.exists())
        self.assertIn("حالت آزمایشی", proc.stdout)
        self.assertIn("v0.6.0", proc.stdout)
        self.assertIn("ادامهٔ تغییرات", proc.stdout)

    def test_dry_run_does_not_require_token(self) -> None:
        release = load_release("v1.7.0")
        proc = self.run_bot(
            "--dry-run",
            event_path=self.write_event("published", release),
            env={"TG_TOKEN": "", "TG_CHAT": ""},
        )
        self.assert_ok(proc)
        self.assertIn("پیام", proc.stdout)


class ErrorHandlingTests(BotTestCase):
    def test_missing_token_is_explained(self) -> None:
        release = load_release("v1.7.0")
        proc = self.run_bot(
            event_path=self.write_event("published", release),
            env={"TG_TOKEN": ""},
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIn("TG_TOKEN", proc.stdout)

    def test_invalid_token_is_explained(self) -> None:
        self.api.bad_token = True
        release = load_release("v1.7.0")
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assertEqual(proc.returncode, 2)
        self.assertIn("توکن ربات نامعتبر", proc.stdout)

    def test_bot_without_post_rights_is_explained(self) -> None:
        self.api.chat_member = {"status": "left"}
        release = load_release("v1.7.0")
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assertEqual(proc.returncode, 2)
        self.assertIn("ادمین کانال", proc.stdout)

    def test_chat_not_found_is_explained(self) -> None:
        self.api.chat_missing = True
        release = load_release("v1.7.0")
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assertEqual(proc.returncode, 2)
        self.assertIn("چت مقصد پیدا نشد", proc.stdout)

    def test_rate_limit_is_retried(self) -> None:
        self.api.script_failure(
            "sendMessage", 429, "Too Many Requests: retry after 1", {"retry_after": 1}
        )
        release = load_release("v1.7.0")
        proc = self.run_bot(event_path=self.write_event("published", release))
        self.assert_ok(proc)
        self.assertEqual(len(self.api.calls("sendMessage")), 2)
        self.assertEqual(len(self.api.sent_messages()), 1)


class ManualModeTests(BotTestCase):
    def test_latest_mode_picks_newest_release(self) -> None:
        self.api.github_releases = [load_release("v0.6.0"), load_release("v1.7.0")]
        proc = self.run_bot("--mode", "latest")
        self.assert_ok(proc)
        messages = self.api.sent_messages()
        self.assertEqual(len(messages), 1)
        self.assertIn("v1.7.0", messages[0]["text"])

    def test_tag_mode_sends_requested_release(self) -> None:
        self.api.github_releases = [load_release("v0.5.4"), load_release("v1.7.0")]
        proc = self.run_bot("--mode", "tag", "--tag", "v0.5.4")
        self.assert_ok(proc)
        text = self.api.sent_messages()[0]["text"]
        self.assertIn("v0.5.4", text)
        self.assertNotIn("v1.7.0", text)

    def test_tag_mode_requires_tag(self) -> None:
        proc = self.run_bot("--mode", "tag")
        self.assertEqual(proc.returncode, 2)
        self.assertIn("TAG", proc.stdout)

    def test_tag_mode_unknown_tag(self) -> None:
        self.api.github_releases = [load_release("v1.7.0")]
        proc = self.run_bot("--mode", "tag", "--tag", "v9.9.9")
        self.assertEqual(proc.returncode, 2)
        self.assertIn("پیدا نشد", proc.stdout)


class WorkflowContractTests(BotTestCase):
    """The workflows drive the bot only through environment variables.

    These tests pin that contract so a renamed variable cannot silently break
    the GitHub Actions wiring.
    """

    def test_workflow_dispatch_env_contract(self) -> None:
        self.api.github_releases = [load_release("v0.5.4")]
        proc = self.run_bot(env={
            "MODE": "tag",
            "TAG": "v0.5.4",
            "FORCE": "false",
            "DRY_RUN": "true",
            "SILENT": "false",
            "CHAT_OVERRIDE": "@PacketYellow",
        })
        self.assert_ok(proc)
        self.assertEqual(self.api.telegram_calls, [])
        self.assertIn("v0.5.4", proc.stdout)

        proc = self.run_bot(env={
            "MODE": "tag",
            "TAG": "v0.5.4",
            "FORCE": "false",
            "DRY_RUN": "false",
            "SILENT": "true",
            "CHAT_OVERRIDE": "@PacketYellow",
        })
        self.assert_ok(proc)
        messages = self.api.sent_messages()
        self.assertEqual(len(messages), 1)
        self.assertTrue(messages[0]["silent"])

    def test_release_event_with_auto_mode(self) -> None:
        release = load_release("v1.7.0")
        proc = self.run_bot(
            env={"MODE": "auto", "FORCE": "", "DRY_RUN": "", "TAG": "", "SILENT": ""},
            event_path=self.write_event("published", release),
        )
        self.assert_ok(proc)
        self.assertEqual(len(self.api.sent_messages()), 1)

    def test_latest_mode_env_only(self) -> None:
        self.api.github_releases = [load_release("v1.7.0"), load_release("v0.6.0")]
        proc = self.run_bot(env={"MODE": "latest", "DRY_RUN": "true"})
        self.assert_ok(proc)
        self.api.telegram_calls.clear()
        proc = self.run_bot(env={"MODE": "latest", "DRY_RUN": "false"})
        self.assert_ok(proc)
        self.assertEqual(len(self.api.sent_messages()), 1)


class IntroAndDoctorTests(BotTestCase):
    def test_intro_sent_once(self) -> None:
        proc = self.run_bot("--mode", "intro")
        self.assert_ok(proc)
        self.assertEqual(len(self.api.sent_messages()), 1)
        text = self.api.sent_messages()[0]["text"]
        self.assertIn("ربات انتشار نسخه‌ها", text)
        self.assert_html_valid(text)
        intro_state = json.loads(self.intro_state_file.read_text(encoding="utf-8"))
        self.assertTrue(intro_state["intro_sent"])

        proc = self.run_bot("--mode", "intro")
        self.assert_ok(proc)
        self.assertEqual(len(self.api.sent_messages()), 1)

    def test_intro_force_replaces_previous(self) -> None:
        self.assert_ok(self.run_bot("--mode", "intro"))
        first_id = self.api.sent_messages()[0]["message_id"]
        self.assert_ok(self.run_bot("--mode", "intro", "--force"))
        alive = self.api.sent_messages()
        self.assertEqual(len(alive), 1)
        self.assertNotEqual(alive[0]["message_id"], first_id)
        self.assertFalse(self.api.messages[first_id]["alive"])

    def test_doctor_reports_success(self) -> None:
        self.api.github_releases = [load_release("v1.7.0")]
        proc = self.run_bot("--mode", "doctor")
        self.assert_ok(proc)
        self.assertIn("✅", proc.stdout)
        summary = self.summary_file.read_text(encoding="utf-8")
        self.assertIn("وضعیت ربات تلگرام", summary)
        self.assertIn("@packetyellow_bot", summary)

    def test_doctor_reports_invalid_token(self) -> None:
        self.api.bad_token = True
        proc = self.run_bot("--mode", "doctor")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("❌", proc.stdout)

    def test_doctor_works_without_secrets(self) -> None:
        proc = self.run_bot("--mode", "doctor", env={"TG_TOKEN": "", "TG_CHAT": ""})
        self.assertEqual(proc.returncode, 1)
        self.assertIn("تنظیم نشده", proc.stdout)


class WorkflowFileContractTests(unittest.TestCase):
    """Cheap guard: the YAML must keep calling the bot with the same env names."""

    REQUIRED_ENV = (
        "TG_TOKEN", "TG_CHAT", "THREAD_ID", "GH_TOKEN", "REPOSITORY",
        "MODE", "TAG", "FORCE", "DRY_RUN", "SILENT", "CHAT_OVERRIDE",
    )

    def workflow_text(self, name: str) -> str:
        return (REPO_ROOT / ".github" / "workflows" / name).read_text(encoding="utf-8")

    def test_main_workflow_env_contract(self) -> None:
        text = self.workflow_text("telegram.yml")
        for name in self.REQUIRED_ENV:
            self.assertIn(f"{name}:", text, f"{name} missing from the workflow env")
        self.assertIn("python .github/scripts/telegram_bot.py", text)
        self.assertIn("types: [published, prereleased, released, edited, deleted]", text)
        self.assertIn("continue-on-error: true", text)  # state persistence is best-effort

    def test_manual_workflows_use_the_same_script(self) -> None:
        for name in ("telegram-latest-release.yml", "telegram-intro.yml"):
            text = self.workflow_text(name)
            self.assertIn("python .github/scripts/telegram_bot.py", text)
            self.assertIn("MODE:", text)

    def test_state_files_are_committed_and_parseable(self) -> None:
        state = json.loads(
            (REPO_ROOT / ".github" / "telegram-message-map.json").read_text(encoding="utf-8")
        )
        self.assertEqual(state.get("version"), 2)
        intro = json.loads(
            (REPO_ROOT / ".github" / "telegram-intro-state.json").read_text(encoding="utf-8")
        )
        self.assertEqual(intro.get("version"), 2)


class PlanTests(unittest.TestCase):
    """Pure unit tests for message composition (no HTTP involved)."""

    def make_config(self, **overrides: Any) -> bot.Config:
        defaults = dict(
            mode="auto",
            token="123:TEST",
            chats=["@PacketYellow"],
            dry_run=True,
            state_file=Path("state.json"),
            intro_state_file=Path("intro.json"),
        )
        defaults.update(overrides)
        return bot.Config(**defaults)

    def test_all_fixtures_render_safe_plans(self) -> None:
        config = self.make_config()
        fixtures = sorted(FIXTURES.glob("release-*.json"))
        self.assertGreaterEqual(len(fixtures), 5)
        for path in fixtures:
            with self.subTest(fixture=path.name):
                release = json.loads(path.read_text(encoding="utf-8"))
                plan = bot.build_release_plan(config, release)
                self.assertGreaterEqual(len(plan.parts), 1)
                self.assertLessEqual(len(plan.parts), config.max_parts)
                for part in plan.parts:
                    self.assertLessEqual(T.telegram_length(part), T.TELEGRAM_LIMIT)
                    self.assertEqual(T.html_problems(part), [])
                    self.assertNotIn("\x00", part)
                if len(plan.parts) > 1:
                    self.assertNotIn("ادامهٔ تغییرات", plan.parts[0])
                    self.assertIn("ادامهٔ تغییرات", plan.parts[1])
                self.assertIn("صفحهٔ انتشار و همهٔ فایل‌ها", plan.parts[-1])
                self.assertEqual(plan.preview_url, release["html_url"])
                for row in plan.buttons:
                    for button in row:
                        self.assertLessEqual(len(button["text"].encode("utf-8")), 64)

    def test_huge_notes_are_capped_with_continuation_link(self) -> None:
        config = self.make_config(max_parts=4)
        release = dict(load_release("v1.7.0"))
        release["body"] = "\n".join(f"- تغییر شمارهٔ {index} که متن آن کوتاه است" for index in range(2000))
        plan = bot.build_release_plan(config, release)
        self.assertLessEqual(len(plan.parts), 4)
        self.assertTrue(
            any("ادامهٔ یادداشت انتشار" in part for part in plan.parts),
            "truncation pointer is missing",
        )
        self.assertIn("صفحهٔ انتشار و همهٔ فایل‌ها", plan.parts[-1])

    def test_draft_release_is_rejected(self) -> None:
        release = dict(load_release("v1.7.0"))
        release["draft"] = True
        with self.assertRaises(bot.BotError):
            bot.build_release_plan(self.make_config(), release)

    def test_nasty_markdown_body_stays_telegram_safe(self) -> None:
        release = dict(load_release("v1.7.0"))
        release["body"] = "\n".join(
            [
                "## عنوان **ناقص",
                "***همه با هم*** و *کج* و **پررنگ",
                "[لینک با `کد`](https://x.com/a?b=1&c=2)",
                "[نقل قول](https://x.com/a%22b)",
                "2 * 3 = 6 و 5 ** 2 = 25",
                "<script>alert(1)</script>",
                "`<b>code</b>`",
                "~~strike **bold**~~",
            ]
        )
        plan = bot.build_release_plan(self.make_config(), release)
        for part in plan.parts:
            self.assertEqual(T.html_problems(part), [], part)
            self.assertLessEqual(T.telegram_length(part), T.TELEGRAM_LIMIT)
            self.assertNotIn("\x00", part)

    def test_emoji_heavy_notes_stay_under_utf16_limit(self) -> None:
        config = self.make_config()
        release = dict(load_release("v1.7.0"))
        release["body"] = "\n".join(f"- 🎉🚀✅ تغییر شمارهٔ {index}" for index in range(800))
        plan = bot.build_release_plan(config, release)
        for part in plan.parts:
            self.assertLessEqual(T.telegram_length(part), T.TELEGRAM_LIMIT)
            self.assertEqual(T.html_problems(part), [])

    def test_prepare_part_downgrades_broken_html(self) -> None:
        text, parse_mode = bot._prepare_part("<b>unclosed")
        self.assertIsNone(parse_mode)
        self.assertNotIn("<b>", text)
        self.assertIn("unclosed", text)
        self.assertEqual(bot._prepare_part("<b>ok</b>"), ("<b>ok</b>", "HTML"))

    def test_buttons_include_installers(self) -> None:
        plan = bot.build_release_plan(self.make_config(), load_release("v1.7.0"))
        labels = [button["text"] for row in plan.buttons for button in row]
        self.assertTrue(any("EXE" in label for label in labels))
        self.assertTrue(any("MSI" in label for label in labels))
        self.assertTrue(any("صفحهٔ انتشار" in label for label in labels))

    def test_intro_plan_is_valid(self) -> None:
        plan = bot.build_intro_plan(self.make_config())
        self.assertEqual(len(plan.parts), 1)
        self.assertEqual(T.html_problems(plan.parts[0]), [])
        self.assertIn("PacketYellow", plan.parts[0])
        self.assertIn("Mehdi Jafari", plan.parts[0])


if __name__ == "__main__":
    unittest.main()
