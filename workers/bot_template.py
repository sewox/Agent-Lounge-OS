"""Agent Lounge OS — yeniden kullanılabilir NATS bot worker tabanı.

Bağlantı:
  1. lounge.workers.register ile kaydolur
  2. Periyodik lounge.workers.heartbeat gönderir
  3. lounge.tasks.<bot_id> dinler (Kernel onay sonrası yönlendirir)
  4. Sonucu lounge.task.completed / failed (+ isteğe bağlı experience) yayımlar
  5. SIGINT/SIGTERM'de unregister + düzgün kapanış

Güvenlik: Bu worker PENDING_APPROVAL / kota kapısını baypas etmez; yalnızca
Kernel'in onayladıktan sonra lounge.tasks.<bot> konusuna yazdığı işleri alır.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import signal
import sys
import uuid
from abc import ABC, abstractmethod
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

LOG = logging.getLogger("lounge.bot")

WORKERS_REGISTER = "lounge.workers.register"
WORKERS_UNREGISTER = "lounge.workers.unregister"
WORKERS_HEARTBEAT = "lounge.workers.heartbeat"
AGENT_STATUS = "lounge.agent.status"
TASK_COMPLETED = "lounge.task.completed"
TASK_FAILED = "lounge.task.failed"
EXPERIENCE_REPORTED = "lounge.experience.reported"
TASKS_INBOX_PREFIX = "lounge.tasks."

BOT_ID_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")

_SCHEMA_DIR = (
    Path(__file__).resolve().parents[1] / "shared" / "lounge_protocol" / "schemas"
)


def now_rfc3339() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"


def worker_tasks_subject(bot_id: str) -> str:
    cleaned = bot_id.strip().lower()
    if not BOT_ID_RE.match(cleaned):
        raise ValueError(f"geçersiz bot_id: {bot_id!r}")
    return f"{TASKS_INBOX_PREFIX}{cleaned}"


def _load_schema(name: str) -> dict[str, Any]:
    path = _SCHEMA_DIR / name
    with path.open(encoding="utf-8") as fh:
        return json.load(fh)


def validate_task_payload(payload: dict[str, Any]) -> dict[str, Any]:
    """LoungeTask veya TaskAssignment gövdesini doğrular; geçersizse ValueError."""
    try:
        import jsonschema
    except ImportError as exc:  # pragma: no cover
        raise RuntimeError("jsonschema gerekli: pip install -r workers/requirements.txt") from exc

    task_schema = _load_schema("task.schema.json")
    # Assignment: { task, context? }
    if "task" in payload and isinstance(payload["task"], dict):
        task = payload["task"]
        jsonschema.validate(instance=task, schema=task_schema)
        return payload
    jsonschema.validate(instance=payload, schema=task_schema)
    return {"task": payload, "context": {}}


def validate_worker_registration(payload: dict[str, Any]) -> None:
    try:
        import jsonschema
    except ImportError as exc:  # pragma: no cover
        raise RuntimeError("jsonschema gerekli") from exc
    schema = _load_schema("worker_registration.schema.json")
    jsonschema.validate(instance=payload, schema=schema)


class BotWorker(ABC):
    """Uzun yaşayan asyncio + nats-py worker tabanı."""

    bot_id: str
    name: str
    version: str = "0.1.0"
    capabilities: list[str]
    nats_url: str
    heartbeat_interval: float = 10.0

    def __init__(
        self,
        bot_id: str,
        name: str,
        *,
        capabilities: Optional[list[str]] = None,
        version: str = "0.1.0",
        nats_url: Optional[str] = None,
        heartbeat_interval: float = 10.0,
    ) -> None:
        self.bot_id = bot_id.strip().lower()
        if not BOT_ID_RE.match(self.bot_id):
            raise ValueError(f"geçersiz bot_id: {bot_id!r}")
        self.name = name
        self.version = version
        self.capabilities = list(capabilities or [])
        self.nats_url = nats_url or os.environ.get("LOUNGE_NATS_URL", "nats://127.0.0.1:4222")
        self.heartbeat_interval = heartbeat_interval
        self._nc: Any = None
        self._stop = asyncio.Event()
        self._tasks_subject = worker_tasks_subject(self.bot_id)

    @abstractmethod
    async def handle_task(
        self, task: dict[str, Any], context: dict[str, Any]
    ) -> dict[str, Any]:
        """Görevi işle; güncellenmiş LoungeTask dict döndür (summary sonucu içerebilir)."""

    async def on_registered(self) -> None:
        """Kayıt sonrası kanca — örn. lobiye Merhaba."""

    def registration_payload(self, action: str) -> dict[str, Any]:
        body = {
            "bot_id": self.bot_id,
            "name": self.name,
            "capabilities": self.capabilities,
            "version": self.version,
            "pid": os.getpid(),
            "action": action,
            "created_at": now_rfc3339(),
        }
        validate_worker_registration(body)
        return body

    async def publish(self, subject: str, payload: dict[str, Any]) -> None:
        assert self._nc is not None
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        await self._nc.publish(subject, data)

    async def publish_progress(self, task_id: str, phase: str, detail: str = "") -> None:
        await self.publish(
            AGENT_STATUS,
            {
                "bot_id": self.bot_id,
                "status": "busy",
                "phase": phase,
                "task_id": task_id,
                "detail": detail,
                "created_at": now_rfc3339(),
            },
        )

    async def publish_result(
        self,
        task: dict[str, Any],
        *,
        ok: bool = True,
        experience: Optional[dict[str, Any]] = None,
    ) -> None:
        subject = TASK_COMPLETED if ok else TASK_FAILED
        await self.publish(subject, task)
        if experience is not None:
            await self.publish(EXPERIENCE_REPORTED, experience)

    async def _register(self) -> None:
        await self.publish(WORKERS_REGISTER, self.registration_payload("register"))
        LOG.info("kayıt: %s → %s", self.bot_id, self._tasks_subject)
        await self.on_registered()

    async def _unregister(self) -> None:
        try:
            await self.publish(WORKERS_UNREGISTER, self.registration_payload("unregister"))
            LOG.info("kayıt silindi: %s", self.bot_id)
        except Exception as err:  # noqa: BLE001
            LOG.warning("unregister başarısız: %s", err)

    async def _heartbeat_loop(self) -> None:
        while not self._stop.is_set():
            try:
                await self.publish(WORKERS_HEARTBEAT, self.registration_payload("heartbeat"))
            except Exception as err:  # noqa: BLE001
                LOG.warning("heartbeat: %s", err)
            try:
                await asyncio.wait_for(self._stop.wait(), timeout=self.heartbeat_interval)
            except asyncio.TimeoutError:
                continue

    async def _handle_message(self, msg: Any) -> None:
        raw: Any = None
        try:
            raw = json.loads(msg.data.decode("utf-8"))
            wrapped = validate_task_payload(raw)
            task = wrapped["task"]
            context = wrapped.get("context") or {}
            await self.publish_progress(task.get("id", ""), "started")
            result = await self.handle_task(task, context)
            if not isinstance(result, dict) or result.get("type") != "task":
                raise ValueError("handle_task LoungeTask dict döndürmeli")
            await self.publish_result(result, ok=True)
        except Exception as err:  # noqa: BLE001
            LOG.exception("görev işlenemedi: %s", err)
            fallback = None
            if isinstance(raw, dict):
                fallback = raw.get("task") if isinstance(raw.get("task"), dict) else raw
            if isinstance(fallback, dict) and fallback.get("type") == "task":
                failed = dict(fallback)
                summary = failed.get("summary", "")
                failed["summary"] = f"{summary} [worker-error: {err}]".strip()
                try:
                    await self.publish_result(failed, ok=False)
                except Exception:  # noqa: BLE001
                    LOG.exception("failed yayınlanamadı")

    async def _connect(self) -> Any:
        import nats

        backoff = 1.0
        while not self._stop.is_set():
            try:
                nc = await nats.connect(self.nats_url)
                LOG.info("NATS bağlandı: %s", self.nats_url)
                return nc
            except Exception as err:  # noqa: BLE001
                LOG.warning("NATS bağlantı hatası (%s), %.1fs sonra…", err, backoff)
                try:
                    await asyncio.wait_for(self._stop.wait(), timeout=backoff)
                    break
                except asyncio.TimeoutError:
                    backoff = min(backoff * 2, 30.0)
        raise RuntimeError("durduruldu — NATS bağlanamadı")

    async def run(self) -> None:
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            try:
                loop.add_signal_handler(sig, self._stop.set)
            except NotImplementedError:  # Windows
                signal.signal(sig, lambda *_: self._stop.set())

        while not self._stop.is_set():
            try:
                self._nc = await self._connect()
                await self._register()
                await self._nc.subscribe(self._tasks_subject, cb=self._on_msg)
                hb = asyncio.create_task(self._heartbeat_loop(), name="heartbeat")
                await self._stop.wait()
                hb.cancel()
                await self._unregister()
                await self._nc.drain()
                break
            except Exception as err:  # noqa: BLE001
                LOG.error("oturum koptu: %s — yeniden bağlanılıyor", err)
                await asyncio.sleep(2.0)
            finally:
                self._nc = None

    async def _on_msg(self, msg: Any) -> None:
        asyncio.create_task(self._handle_message(msg))


def make_experience(
    task: dict[str, Any],
    adr_summary: str,
    *,
    outcome: str = "success",
    tags: Optional[list[str]] = None,
) -> dict[str, Any]:
    return {
        "id": str(uuid.uuid4()),
        "type": "experience",
        "agent": task.get("target_agent") or task.get("source_agent") or "worker",
        "project_id": task.get("project_id", "agent-lounge-os"),
        "adr_summary": adr_summary,
        "outcome": outcome,
        "related_task_id": task.get("id"),
        "tags": tags or [],
        "created_at": now_rfc3339(),
    }


def main_for(worker: BotWorker) -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s [%(name)s] %(message)s",
        stream=sys.stderr,
    )
    try:
        asyncio.run(worker.run())
    except KeyboardInterrupt:
        pass
