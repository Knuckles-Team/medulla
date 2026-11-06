# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-types (the source of truth).
"""Shared runtime for the generated engine-contract client.

OpaqueResult is what a method returns when this client does not model its
declared body (a DTO, a caller-shaped body, or a result the contract has not
classified): the decoded payload paired with the method id that produced it,
so a caller can never mistake it for a value the client validated.
"""

from __future__ import annotations

import functools
import re
from collections.abc import Callable, Coroutine
from types import ModuleType
from typing import Any, NamedTuple

from pydantic import TypeAdapter, ValidationError

from ._ids import METHOD_IDS


class OpaqueResult(NamedTuple):
    method: str
    payload: Any


class ContractViolation(RuntimeError):
    """The engine returned a shape the contract does not declare for the method.

    The engine encodes every declared result through a compile-checked marker, so a
    violation means the client and the engine were built from different contracts.
    The generated send checks the decoded payload rather than returning it under a
    signature that would lie, and names the method, the declared encoding and the
    shape actually observed.
    """


def _violation(method: str, claimed: str, payload: Any) -> ContractViolation:
    return ContractViolation(
        f"{method}: contract claims ResultPayload::{claimed}, engine returned "
        f"{type(payload).__name__}"
    )


def decode_result(method: str, annotation: Any, result: OpaqueResult) -> Any:
    """Validate an OpaqueResult against the model the contract declares for it."""
    if result.method != method:
        raise ValueError(f"a {result.method} result passed to the {method} decoder")
    try:
        return _adapter(annotation).validate_python(result.payload)
    except ValidationError as error:
        raise ContractViolation(
            f"{method}: the engine result does not match its contract model"
        ) from error


@functools.cache
def _adapter(annotation: Any) -> TypeAdapter[Any]:
    return TypeAdapter(annotation)


def models() -> ModuleType:
    """The generated models module, imported on first use rather than at import.

    ``models.py`` holds every contract model; loading it costs far more than a
    domain module, so a domain module reaches it only through this accessor.
    """
    from . import models as loaded

    return loaded


def expect_bool(method: str, payload: Any) -> bool:
    if not isinstance(payload, bool):
        raise _violation(method, "Bool", payload)
    return payload


def expect_count(method: str, payload: Any) -> int:
    if isinstance(payload, bool) or not isinstance(payload, int):
        raise _violation(method, "Count", payload)
    return payload


def expect_float(method: str, payload: Any) -> float:
    # An f64 that happens to be integral arrives from MessagePack as an int.
    if isinstance(payload, bool) or not isinstance(payload, (int, float)):
        raise _violation(method, "Float", payload)
    return float(payload)


def expect_string(method: str, payload: Any) -> str:
    if not isinstance(payload, str):
        raise _violation(method, "String", payload)
    return payload


def expect_ids(method: str, payload: Any) -> list[str]:
    if not isinstance(payload, list) or not all(
        isinstance(item, str) for item in payload
    ):
        raise _violation(method, "Ids", payload)
    return payload


def _rows(method: str, claimed: str, payload: Any, width: int) -> list[Any]:
    if not isinstance(payload, list) or not all(
        isinstance(row, (list, tuple)) and len(row) == width for row in payload
    ):
        raise _violation(method, claimed, payload)
    return payload


def expect_nodelist(method: str, payload: Any) -> list[Any]:
    return _rows(method, "NodeList", payload, 2)


def expect_edgelist(method: str, payload: Any) -> list[Any]:
    return _rows(method, "EdgeList", payload, 3)


async def send_by_id(
    client: Any,
    method: str,
    params: dict[str, Any] | None = None,
    graph: str | None = None,
    *,
    idempotency_key: str | None = None,
) -> Any:
    """Send a method chosen at run time.

    The hand-written helpers that pick a method from a parameter reach the wire
    through this one entry point, so no method id is spelled outside generated code.
    """
    if method not in METHOD_IDS:
        raise ValueError(f"{method} is not an engine-contract method")
    return await client._send(
        method,
        params,
        graph,
        idempotency_key=idempotency_key,
    )


Sender = Callable[..., Coroutine[Any, Any, Any]]
Decoder = Callable[[OpaqueResult], Any]


def _snake(method: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", method).lower()


def contract_sender(
    method: str,
    params_model: str,
    expect: Callable[[str, Any], Any] | None = None,
) -> Sender:
    """One sender from its contract row: validate the params, send, check the result.

    ``params_model`` names the ``models`` class mirroring the Rust request variant;
    ``expect`` checks a scalar payload (``expect_bool`` and friends). Without it the
    payload comes back as an ``OpaqueResult`` for the method's ``decode_*``.
    """

    async def send(
        client: Any,
        params: dict[str, Any] | None = None,
        graph: str | None = None,
        *,
        idempotency_key: str | None = None,
    ) -> Any:
        getattr(models(), params_model).model_validate(params or {})
        payload = await client._send(
            method,
            params,
            graph,
            idempotency_key=idempotency_key,
        )
        if expect is None:
            return OpaqueResult(method, payload)
        return expect(method, payload)

    send.__name__ = send.__qualname__ = f"send_{_snake(method)}"
    send.__doc__ = f"Send one engine-contract request.\n\nMethod:\n    {method}\n"
    return send


def contract_decoder(method: str, result_model: str, op: str = "") -> Decoder:
    """Validate a method's ``OpaqueResult`` against the named ``models`` result model.

    ``op`` names the request op whose body ``result_model`` is, for a method that
    declares one result body per op.
    """

    def decode(result: OpaqueResult) -> Any:
        return decode_result(method, getattr(models(), result_model), result)

    suffix = f"_{op}" if op else ""
    decode.__name__ = decode.__qualname__ = f"decode_{_snake(method)}{suffix}"
    decode.__doc__ = f"Validate the {method} result against {result_model}."
    return decode
