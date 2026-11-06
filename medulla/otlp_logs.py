"""Typed OTLP log ingest for the observability listener (MEDULLA-CONTRACT-R050).

``OtlpLogBatch`` is a strict, hand-written model of one OTLP/HTTP JSON
``ExportLogsServiceRequest``. A batch holds at most
:data:`MAX_BATCH_RECORDS` records and encodes to at most
:data:`MAX_BATCH_BYTES` bytes; an oversize or malformed batch raises
``pydantic.ValidationError`` before anything is sent. The engine enforces the
same bounds (``LOG_BATCH_TOO_LARGE``) on ``POST /v1/logs``.
"""

from __future__ import annotations

import asyncio
import http.client
import json
from typing import Any

from pydantic import BaseModel, ConfigDict, Field, model_validator

MAX_BATCH_RECORDS = 1_000
MAX_BATCH_BYTES = 4 * 1024 * 1024


class _Strict(BaseModel):
    model_config = ConfigDict(extra="forbid", populate_by_name=True)


class OtlpLogRecord(_Strict):
    """One OTLP log record."""

    time_unix_nano: str | int | None = Field(default=None, alias="timeUnixNano")
    severity_text: str | None = Field(default=None, alias="severityText")
    severity_number: int | None = Field(default=None, alias="severityNumber")
    body: dict[str, Any] | None = None
    attributes: list[dict[str, Any]] = Field(default_factory=list)


class OtlpScopeLogs(_Strict):
    """The records one instrumentation scope emitted."""

    scope: dict[str, Any] | None = None
    log_records: list[OtlpLogRecord] = Field(default_factory=list, alias="logRecords")


class OtlpResourceLogs(_Strict):
    """The scopes one resource emitted."""

    resource: dict[str, Any] | None = None
    scope_logs: list[OtlpScopeLogs] = Field(default_factory=list, alias="scopeLogs")


class OtlpLogBatch(_Strict):
    """One OTLP/HTTP JSON log export: at most 1,000 records and 4 MiB."""

    resource_logs: list[OtlpResourceLogs] = Field(alias="resourceLogs")

    @property
    def record_count(self) -> int:
        """Total log records across every resource and scope."""
        return sum(
            len(scope.log_records)
            for resource in self.resource_logs
            for scope in resource.scope_logs
        )

    def to_json(self) -> str:
        """The OTLP JSON body, using the wire field names."""
        return self.model_dump_json(by_alias=True, exclude_none=True)

    @model_validator(mode="after")
    def _within_bounds(self) -> OtlpLogBatch:
        if self.record_count > MAX_BATCH_RECORDS:
            raise ValueError(
                f"a log batch holds at most {MAX_BATCH_RECORDS} records, "
                f"got {self.record_count}"
            )
        if len(self.to_json().encode("utf-8")) > MAX_BATCH_BYTES:
            raise ValueError(f"a log batch encodes to at most {MAX_BATCH_BYTES} bytes")
        return self


def _post_logs(obs_addr: str, body: bytes, timeout: float) -> tuple[int, str]:
    host, _, port = obs_addr.rpartition(":")
    conn = http.client.HTTPConnection(host, int(port), timeout=timeout)
    try:
        conn.request(
            "POST",
            "/v1/logs",
            body=body,
            headers={"content-type": "application/json"},
        )
        response = conn.getresponse()
        return response.status, response.read().decode("utf-8", errors="replace")
    finally:
        conn.close()


async def ingest_otlp_logs(
    batch: OtlpLogBatch | dict[str, Any], *, obs_addr: str, timeout: float = 30.0
) -> dict[str, Any]:
    """Post ``batch`` to the engine's ``--obs-addr`` listener (``POST /v1/logs``).

    A mapping is validated into an :class:`OtlpLogBatch` first, so an oversize
    or malformed batch raises ``pydantic.ValidationError`` without any
    network traffic. Returns the engine's OTLP ``partialSuccess`` body; an
    engine refusal raises :class:`RuntimeError` carrying the stable code
    (``LOG_BATCH_TOO_LARGE``).
    """
    if not isinstance(batch, OtlpLogBatch):
        batch = OtlpLogBatch.model_validate(batch)
    body = batch.to_json().encode("utf-8")
    status, text = await asyncio.to_thread(_post_logs, obs_addr, body, timeout)
    if status != 200:
        raise RuntimeError(f"{status}: {text}")
    return json.loads(text)
