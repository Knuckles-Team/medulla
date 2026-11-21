"""MEDULLA-TYPED-PACKS-R100.101 served acceptance: skill consumer compatibility.

``AgentComponent.admit_skill_upgrade`` judges a skill upgrade against the contract
its consumers declared, never against a version or a pin. An additive change is
compatible; a removal or a type change is breaking and is admitted only with a
declared migration bound to the exact previous package digest, new package digest
and new consumer contract digest whose receipt passed. A refusal writes no
admission. A held admission stops holding the moment the consumer contract or the
package in force differs from the one it was taken against.
"""

from __future__ import annotations

import hashlib
import struct
import uuid

import pytest
from ontology_pack_support import connect

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.agent_component import (
    SkillAdmissionRequest,
    SkillContractFieldWire,
    SkillMigrationWire,
)
from medulla.generated.storage import send_agent_component_admit_skill_upgrade

from conftest import TEST_TENANT  # isort: skip

CONTRACT_DOMAIN = b"eg/consumer-contract/v1"


def _framed(domain: bytes, fields: list[bytes]) -> str:
    """The engine's framed SHA-256 (``Digest256::framed``), as lowercase hex."""
    hasher = hashlib.sha256()
    hasher.update(b"eg/framed-sha256/v1\0")
    hasher.update(struct.pack(">I", len(domain)))
    hasher.update(domain)
    hasher.update(struct.pack(">I", len(fields)))
    for field in fields:
        hasher.update(struct.pack(">Q", len(field)))
        hasher.update(field)
    return hasher.hexdigest()


def _field(name: str, type_name: str, required: bool) -> SkillContractFieldWire:
    return SkillContractFieldWire(name=name, type_name=type_name, required=required)


def _contract_digest(fields: list[SkillContractFieldWire]) -> str:
    framed: list[bytes] = []
    for field in sorted(fields, key=lambda item: item.name):
        framed += [
            field.name.encode(),
            field.type_name.encode(),
            b"\x01" if field.required else b"\x00",
        ]
    return _framed(CONTRACT_DOMAIN, framed)


def _package(label: str) -> str:
    return hashlib.sha256(label.encode()).hexdigest()


PREVIOUS_PACKAGE = _package("skill-package-v1")
NEW_PACKAGE = _package("skill-package-v2")
OTHER_PACKAGE = _package("skill-package-other")
PREVIOUS = [_field("a", "int", True), _field("b", "text", False)]
ADDITIVE = [*PREVIOUS, _field("c", "int", False)]
BREAKING = [_field("a", "text", True), _field("b", "text", False)]


def _request(
    component: str,
    new_contract: list[SkillContractFieldWire],
    *,
    new_package: str = NEW_PACKAGE,
    migration: SkillMigrationWire | None = None,
    held: int | None = None,
) -> SkillAdmissionRequest:
    return SkillAdmissionRequest(
        tenant_id=TEST_TENANT,
        component_id=component,
        previous_package_digest=PREVIOUS_PACKAGE,
        new_package_digest=new_package,
        previous_contract=PREVIOUS,
        new_contract=new_contract,
        cohort=["consumer:alpha", "consumer:beta"],
        migration=migration,
        held_admission_revision=held,
    )


def _migration(
    new_contract: list[SkillContractFieldWire],
    *,
    previous_package: str = PREVIOUS_PACKAGE,
    new_package: str = NEW_PACKAGE,
    contract_digest: str | None = None,
    passed: bool = True,
) -> SkillMigrationWire:
    return SkillMigrationWire(
        migration_id="migration:v1-to-v2",
        migration_digest=_package("migration-definition"),
        previous_package_digest=previous_package,
        new_package_digest=new_package,
        consumer_contract_digest=contract_digest or _contract_digest(new_contract),
        receipt_passed=passed,
    )


async def _admit(client: MedullaClient, request: SkillAdmissionRequest):
    return await send_agent_component_admit_skill_upgrade(
        client, request, idempotency_key=f"skill-admit:{uuid.uuid4().hex}"
    )


async def _refused(
    client: MedullaClient, request: SkillAdmissionRequest, code: str
) -> None:
    with pytest.raises(EngineResponseError, match=code):
        await _admit(client, request)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R100.101")
@pytest.mark.asyncio
async def test_breaking_upgrade_requires_exact_consumer_migration() -> None:
    client = await connect()
    try:
        component = f"skill:admission-{uuid.uuid4().hex[:8]}"

        # An additive change is compatible: nothing a consumer relied on changed.
        compatible = await _admit(client, _request(component, ADDITIVE))
        assert compatible.verdict == "compatible"
        assert compatible.admission_revision == 1
        assert compatible.migration_id is None
        assert compatible.previous_package_digest == PREVIOUS_PACKAGE
        assert compatible.new_package_digest == NEW_PACKAGE
        assert compatible.previous_contract_digest == _contract_digest(PREVIOUS)
        assert compatible.new_contract_digest == _contract_digest(ADDITIVE)
        assert compatible.cohort == ["consumer:alpha", "consumer:beta"]

        # A retyped field is breaking and a breaking change without a declared
        # migration is refused before anything is written.
        await _refused(
            client, _request(component, BREAKING), "SKILL_BREAKING_NO_MIGRATION"
        )

        # A migration bound to any other change, or one whose receipt failed, is
        # not the exact consumer migration.
        for wrong in (
            _migration(BREAKING, previous_package=OTHER_PACKAGE),
            _migration(BREAKING, new_package=OTHER_PACKAGE),
            _migration(BREAKING, contract_digest=_contract_digest(ADDITIVE)),
            _migration(BREAKING, passed=False),
        ):
            await _refused(
                client,
                _request(component, BREAKING, migration=wrong),
                "SKILL_MIGRATION_RECEIPT_INVALID",
            )

        # The exact migration admits the breaking upgrade, and the refusals above
        # took no revision: this admission is the second one, not the sixth.
        exact = _migration(BREAKING)
        admitted = await _admit(client, _request(component, BREAKING, migration=exact))
        assert admitted.verdict == "breaking_with_migration"
        assert admitted.admission_revision == 2
        assert admitted.migration_id == exact.migration_id
        assert admitted.migration_digest == exact.migration_digest
        assert admitted.new_contract_digest == _contract_digest(BREAKING)
        assert admitted.previous_package_digest == PREVIOUS_PACKAGE
        assert admitted.new_package_digest == NEW_PACKAGE

        # The earlier admission is still readable and unchanged.
        reread = await _admit(client, _request(component, ADDITIVE, held=1))
        assert reread == compatible
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R100.101")
@pytest.mark.asyncio
async def test_changed_consumer_contract_invalidates_admission() -> None:
    client = await connect()
    try:
        component = f"skill:invalidate-{uuid.uuid4().hex[:8]}"
        held = await _admit(client, _request(component, ADDITIVE))
        assert held.verdict == "compatible"
        assert held.admission_revision == 1

        # While the package and the consumer contract in force are the ones it was
        # taken against, the admission holds and is answered unchanged.
        holding = await _admit(client, _request(component, ADDITIVE, held=1))
        assert holding == held

        # A consumer contract that differs in any field invalidates it: an added
        # optional field, a removal, a retype.
        for changed in (
            [*ADDITIVE, _field("d", "int", False)],
            [_field("a", "int", True)],
            BREAKING,
        ):
            await _refused(
                client,
                _request(component, changed, held=1),
                "SKILL_CONSUMER_CONTRACT_CHANGED",
            )

        # A different package digest invalidates it too.
        await _refused(
            client,
            _request(component, ADDITIVE, new_package=OTHER_PACKAGE, held=1),
            "SKILL_CONSUMER_CONTRACT_CHANGED",
        )

        # The refusals left the record alone: it still holds, and no revision was taken.
        assert await _admit(client, _request(component, ADDITIVE, held=1)) == held
        next_admission = await _admit(client, _request(component, ADDITIVE))
        assert next_admission.admission_revision == 2

        # A revision never admitted is not found.
        with pytest.raises(EngineResponseError, match="no skill admission"):
            await _admit(client, _request(component, ADDITIVE, held=9))
    finally:
        await client.close()
