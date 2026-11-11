# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-types (the source of truth).
"""Gate: the Python models accept and re-emit every Rust wire vector.

The R001.3 Rust builder computes one vector per wire method (request, result
and a canonical-bytes digest) and materializes it under
``tests/fixtures/wire_vectors/<domain>.json`` plus a ``MANIFEST.json``. This
gate validates every vector through the hand-written models in
``medulla.generated.models``, re-emits it, and compares the canonical bytes
with the vector: a model that lacks a Rust field, or one that accepts an
unknown field, fails. It also recomputes ``MANIFEST.schema_digest`` from the
vector files, so a stale manifest fails the gate.
"""

from __future__ import annotations

import copy
import json
import re
from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel, TypeAdapter, ValidationError

from medulla.generated import _manifest, models

pytestmark = pytest.mark.no_engine

_VECTORS = Path(__file__).resolve().parent / "fixtures" / "wire_vectors"

_METHOD: TypeAdapter[Any] = TypeAdapter(models.Method)


def _vector_files() -> list[Path]:
    files = sorted(_VECTORS.glob("*.json"))
    assert files, f"no wire vector files under {_VECTORS}"
    return files


def _rows() -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for path in _vector_files():
        if path.name == "MANIFEST.json":
            continue
        rows.extend(json.loads(path.read_text(encoding="utf-8")))
    return rows


def _canonical_bytes(value: Any) -> bytes:
    """The vector encoding: compact JSON with sorted keys, bytes as octet arrays.

    Mirrors the Rust side, where the vector carries the decoded encoding of
    the sample and ``Vec<u8>`` renders as an array of integers; the models
    type those fields ``bytes``, so a dump converts them back to octets. A
    model dumps with ``by_alias`` wire names and ``exclude_unset``: a field
    the vector carried (even as an explicit null) is set, a field it left
    out (the Rust ``skip_serializing_if`` optionals) is not, so the dump
    reproduces the vector's own field presence.
    """

    def norm(item: Any) -> Any:
        if isinstance(item, (bytes, bytearray)):
            return list(item)
        if isinstance(item, dict):
            return {key: norm(value) for key, value in item.items()}
        if isinstance(item, list):
            return [norm(item) for item in item]
        if isinstance(item, BaseModel):
            return norm(item.model_dump(by_alias=True, exclude_unset=True))
        return item

    return json.dumps(
        norm(value),
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")


def _result_models() -> dict[str, dict[str, type[BaseModel]]]:
    """method -> {op (empty for one body) -> the declared result model}.

    Collected from the ``contract_decoder`` registrations of the generated
    domain modules; their docstrings carry the method and model names.
    """
    from medulla.generated import (
        agent_component,
        cluster,
        compute,
        connector_pack,
        coordination,
        decision,
        decision_commit,
        digest,
        graph,
        graph_schema,
        index_repository,
        ingestion,
        managed_index,
        messaging,
        policy_evolution,
        query,
        rdf_report,
        reasoning,
        security,
        server_registry,
        solve,
        source_ingestion,
        storage,
        transactions,
        write_back,
    )

    mapping: dict[str, dict[str, type[BaseModel]]] = {}
    pattern = re.compile(r"Validate the (\w+) result against (\w+)\.")
    for module in (
        agent_component,
        cluster,
        compute,
        connector_pack,
        coordination,
        decision,
        decision_commit,
        digest,
        graph,
        graph_schema,
        index_repository,
        ingestion,
        managed_index,
        messaging,
        policy_evolution,
        query,
        rdf_report,
        reasoning,
        security,
        server_registry,
        solve,
        source_ingestion,
        storage,
        transactions,
        write_back,
    ):
        for value in vars(module).values():
            doc = getattr(value, "__doc__", None)
            if not isinstance(doc, str):
                continue
            match = pattern.fullmatch(doc.splitlines()[0])
            if match is None:
                continue
            method, model_name = match.groups()
            model = getattr(models, model_name)
            name = getattr(value, "__name__", "")
            prefix = "decode_" + re.sub(r"(?<!^)(?=[A-Z])", "_", method).lower()
            op = name[len(prefix) + 1 :] if name.startswith(prefix + "_") else ""
            mapping.setdefault(method, {})[op] = model
    return mapping


def _set(document: Any, location: tuple[Any, ...], value: Any) -> None:
    """Set ``value`` at ``location``, skipping union-member tags pydantic
    interleaves into an error location."""
    path = []
    cursor = document
    for key in location:
        in_dict = isinstance(cursor, dict) and key in cursor
        in_list = isinstance(cursor, list) and isinstance(key, int)
        if in_dict or in_list:
            path.append(key)
            cursor = cursor[key]
    if not path:
        raise AssertionError(f"no document path for error location {location}")
    target = document
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = value


def _validate_request(row: dict[str, Any]) -> models.Method:
    request = copy.deepcopy(row["request"])
    for _ in range(64):
        try:
            return _METHOD.validate_python(request)
        except ValidationError as error:
            byte_errors = [
                item
                for item in error.errors()
                if item["type"] == "bytes_type" and isinstance(item["input"], list)
            ]
            if not byte_errors:
                raise
            _set(request, byte_errors[0]["loc"], bytes(byte_errors[0]["input"]))
    raise AssertionError("byte retyping did not converge")


def _result_bodies(
    result: Any, ops: dict[str, type[BaseModel]]
) -> list[tuple[str, Any]]:
    """The vector's result bodies per declared op (a dict keyed by op, or one)."""
    if isinstance(result, dict) and result and all(key in ops for key in result):
        return [(op, body) for op, body in result.items()]
    return [("", result)]


@pytest.mark.spec("MEDULLA-CONTRACT-R001.4")
def test_models_round_trip_every_rust_vector() -> None:
    rows = _rows()
    assert rows, "every Rust vector is checked in"
    # spec: MEDULLA-CONTRACT-R001.4
    result_models = _result_models()
    failures: list[str] = []
    for row in rows:
        method = row["method"]
        model = _validate_request(row)
        assert model.method == method
        if _canonical_bytes(model) != _canonical_bytes(row["request"]):
            failures.append(f"{method}: the request does not re-emit its vector bytes")
        bodies = _result_bodies(row["result"], result_models.get(method, {}))
        for op, body in bodies:
            model_class = result_models.get(method, {}).get(op)
            if model_class is None:
                continue
            # Some declared results are type aliases, not models (a list of
            # models, a map, an optional, a discriminated union); a
            # TypeAdapter validates every declared shape.
            try:
                decoded = TypeAdapter(model_class).validate_python(copy.deepcopy(body))
            except ValidationError as error:
                failures.append(
                    f"{method}: the result does not validate: {error.errors()[:3]}"
                )
                continue
            if _canonical_bytes(decoded) != _canonical_bytes(body):
                failures.append(
                    f"{method}: the result does not re-emit its vector bytes"
                )
    assert not failures, "\n".join(failures)
    # spec: MEDULLA-CONTRACT-R001.4
    digest = _manifest.vector_schema_digest(
        [path for path in _vector_files() if path.name != "MANIFEST.json"]
    )
    assert digest == _manifest.MANIFEST["schema_digest"]
    # spec: MEDULLA-CONTRACT-R001.4


@pytest.mark.spec("MEDULLA-CONTRACT-R001.4")
def test_unknown_field_is_rejected() -> None:
    rows = _rows()
    assert rows, "every Rust vector is checked in"
    # spec: MEDULLA-CONTRACT-R001.4
    result_models = _result_models()
    row = next((row for row in rows if row["method"] in result_models), rows[0])
    method = row["method"]

    bad = copy.deepcopy(row["request"])
    bad["unknown_top"] = 1
    with pytest.raises(ValidationError) as raised:
        _METHOD.validate_python(bad)
    assert raised.value.errors()[0]["type"] == "extra_forbidden"
    # spec: MEDULLA-CONTRACT-R001.4

    bad = copy.deepcopy(row["request"])
    bad["params"]["unknown_nested"] = 1
    with pytest.raises(ValidationError) as raised:
        _METHOD.validate_python(bad)
    assert raised.value.errors()[0]["type"] == "extra_forbidden"
    # spec: MEDULLA-CONTRACT-R001.4

    # Unknown-field rejection is a guarantee of the declared models; result
    # aliases (lists, maps, optionals, unions) carry no field surface of their
    # own, so only BaseModel results are probed here.
    for op, model_class in result_models[method].items():
        if not (isinstance(model_class, type) and issubclass(model_class, BaseModel)):
            continue
        body = _result_bodies(row["result"], {op: model_class})[0][1]
        bad_body = copy.deepcopy(body)
        bad_body["unknown_result_field"] = 1
        with pytest.raises(ValidationError) as raised:
            model_class.model_validate(bad_body)
        assert raised.value.errors()[0]["type"] == "extra_forbidden"
        # spec: MEDULLA-CONTRACT-R001.4
