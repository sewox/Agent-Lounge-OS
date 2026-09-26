#!/usr/bin/env python3
"""Grok-Tester — deterministik sanal test botu (dış LLM yok).

Yetenekler:
  - echo: özeti geri yansıt
  - ping: pong
  - summarize_experience: context.experiences içinden kısa özet
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

from bot_template import AGENT_STATUS, BotWorker, main_for, now_rfc3339  # noqa: E402

CAPABILITIES = ["echo", "ping", "summarize_experience"]


class GrokTester(BotWorker):
    def __init__(self, **kwargs: Any) -> None:
        super().__init__(
            bot_id="grok-tester",
            name="Grok-Tester",
            capabilities=CAPABILITIES,
            version="0.1.0",
            **kwargs,
        )

    async def on_registered(self) -> None:
        await self.publish(
            AGENT_STATUS,
            {
                "bot_id": self.bot_id,
                "name": self.name,
                "status": "online",
                "message": "Merhaba",
                "created_at": now_rfc3339(),
            },
        )

    async def handle_task(
        self, task: dict[str, Any], context: dict[str, Any]
    ) -> dict[str, Any]:
        summary = (task.get("summary") or "").strip()
        kind = self._detect_kind(summary)
        await self.publish_progress(task.get("id", ""), "running", kind)

        if kind == "ping":
            result_text = "pong"
        elif kind == "summarize_experience":
            result_text = self._summarize(context)
        else:
            result_text = f"echo: {summary}"

        out = dict(task)
        out["summary"] = result_text
        out["target_agent"] = self.bot_id
        return out

    def _detect_kind(self, summary: str) -> str:
        lower = summary.lower()
        if re.search(r"\bping\b", lower) or lower.strip() == "ping":
            return "ping"
        if "summarize" in lower or "özet" in lower or "tecrübe" in lower:
            return "summarize_experience"
        return "echo"

    def _summarize(self, context: dict[str, Any]) -> str:
        experiences = context.get("experiences") or []
        if not experiences:
            return "özet: (tecrübe yok)"
        parts: list[str] = []
        for hit in experiences[:3]:
            topic = hit.get("topic") or hit.get("solution_summary") or "?"
            score = hit.get("score")
            if isinstance(score, (int, float)):
                parts.append(f"{topic} ({float(score):.2f})")
            else:
                parts.append(str(topic))
        return "özet: " + "; ".join(parts)


if __name__ == "__main__":
    main_for(GrokTester())
