"""BotWorker birim testleri — NATS mock ile."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any
from unittest.mock import AsyncMock, MagicMock

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from bot_template import (  # noqa: E402
    TASK_COMPLETED,
    WORKERS_REGISTER,
    WORKERS_UNREGISTER,
    BotWorker,
    validate_task_payload,
    validate_worker_registration,
    worker_tasks_subject,
)
from grok_tester import GrokTester  # noqa: E402


def test_worker_tasks_subject():
    assert worker_tasks_subject("grok-tester") == "lounge.tasks.grok-tester"
    with pytest.raises(ValueError):
        worker_tasks_subject("Bad.Bot")


def test_registration_schema_ok():
    validate_worker_registration(
        {
            "bot_id": "grok-tester",
            "name": "Grok-Tester",
            "capabilities": ["echo"],
            "version": "0.1.0",
            "pid": 1,
            "action": "register",
            "created_at": "2026-09-18T12:00:00.000Z",
        }
    )


def test_registration_schema_reject():
    with pytest.raises(Exception):
        validate_worker_registration(
            {
                "bot_id": "Bad.Id",
                "name": "x",
                "capabilities": [],
                "version": "1",
                "pid": 1,
                "action": "register",
            }
        )


def test_task_schema_reject_extra():
    with pytest.raises(Exception):
        validate_task_payload(
            {
                "id": "11111111-1111-4111-8111-111111111111",
                "type": "task",
                "source_agent": "mcp:cursor",
                "project_id": "demo",
                "summary": "echo hi",
                "created_at": "2026-09-18T12:00:00.000Z",
                "evil": True,
            }
        )


def test_task_assignment_ok():
    wrapped = validate_task_payload(
        {
            "task": {
                "id": "11111111-1111-4111-8111-111111111111",
                "type": "task",
                "source_agent": "mcp:cursor",
                "target_agent": "grok-tester",
                "project_id": "demo",
                "summary": "ping",
                "created_at": "2026-09-18T12:00:00.000Z",
            },
            "context": {"experiences": []},
        }
    )
    assert wrapped["task"]["summary"] == "ping"


@pytest.mark.asyncio
async def test_grok_tester_echo_ping_summarize():
    bot = GrokTester()
    task = {
        "id": "11111111-1111-4111-8111-111111111111",
        "type": "task",
        "source_agent": "mcp:cursor",
        "target_agent": "grok-tester",
        "project_id": "demo",
        "summary": "ping",
        "created_at": "2026-09-18T12:00:00.000Z",
    }
    bot.publish_progress = AsyncMock()  # type: ignore[method-assign]
    out = await bot.handle_task(task, {})
    assert out["summary"] == "pong"

    out = await bot.handle_task({**task, "summary": "echo hello"}, {})
    assert out["summary"] == "echo: echo hello"

    ctx = {
        "experiences": [
            {
                "id": "e1",
                "project_id": "demo",
                "agent_id": "cursor",
                "topic": "NATS listen",
                "solution_summary": "spawn_blocking",
                "adr_record": "sync",
                "score": 0.7,
            }
        ]
    }
    out = await bot.handle_task({**task, "summary": "tecrübe özetle"}, ctx)
    assert out["summary"].startswith("özet:")
    assert "NATS listen" in out["summary"]


class _EchoBot(BotWorker):
    def __init__(self) -> None:
        super().__init__("echo-bot", "Echo", capabilities=["echo"])

    async def handle_task(self, task: dict[str, Any], context: dict[str, Any]) -> dict[str, Any]:
        out = dict(task)
        out["summary"] = f"echo: {task['summary']}"
        return out


@pytest.mark.asyncio
async def test_register_and_shutdown_publish():
    bot = _EchoBot()
    published: list[tuple[str, dict[str, Any]]] = []

    async def capture(subject: str, payload: dict[str, Any]) -> None:
        published.append((subject, payload))

    bot.publish = capture  # type: ignore[method-assign]
    bot._nc = MagicMock()
    await bot._register()
    assert any(s == WORKERS_REGISTER for s, _ in published)
    assert published[0][1]["action"] == "register"

    await bot.on_registered()
    await bot._unregister()
    assert any(s == WORKERS_UNREGISTER for s, _ in published)


@pytest.mark.asyncio
async def test_handle_message_completes_task():
    bot = _EchoBot()
    published: list[tuple[str, dict[str, Any]]] = []

    async def capture(subject: str, payload: dict[str, Any]) -> None:
        published.append((subject, payload))

    bot.publish = capture  # type: ignore[method-assign]
    bot._nc = MagicMock()

    body = {
        "task": {
            "id": "11111111-1111-4111-8111-111111111111",
            "type": "task",
            "source_agent": "kernel",
            "target_agent": "echo-bot",
            "project_id": "demo",
            "summary": "hello",
            "created_at": "2026-09-18T12:00:00.000Z",
        }
    }
    msg = MagicMock()
    msg.data = json.dumps(body).encode()
    await bot._handle_message(msg)
    completed = [p for s, p in published if s == TASK_COMPLETED]
    assert completed
    assert completed[0]["summary"] == "echo: hello"


@pytest.mark.asyncio
async def test_schema_reject_does_not_complete():
    bot = _EchoBot()
    published: list[tuple[str, dict[str, Any]]] = []

    async def capture(subject: str, payload: dict[str, Any]) -> None:
        published.append((subject, payload))

    bot.publish = capture  # type: ignore[method-assign]
    bot._nc = MagicMock()
    msg = MagicMock()
    msg.data = json.dumps({"not": "a task"}).encode()
    await bot._handle_message(msg)
    assert not any(s == TASK_COMPLETED for s, _ in published)
