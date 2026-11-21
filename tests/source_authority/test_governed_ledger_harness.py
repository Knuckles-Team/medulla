"""The governed-ledger served harness works end to end (MEDULLA-SRC-AUTH-R003.6)."""

from __future__ import annotations

import sys
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from governed_ledger import (
    BASELINE_FIELDS_CONTRACT,
    attach_approved,
    attach_operator_authority,
    authority_rows,
    candidate_digest,
    consume_approval_lease,
    governed_graph,
    issue_approval_lease,
    validate_repair,
)

from medulla.generated import reasoning
from medulla.generated.graph_schema import SchemaSourceOriginViewApproved

CLASS_C = "https://knuckles-team.github.io/ontology/core#Incident"
SOURCE_SOURCE = {"Mcp": {"server": "srcauth-harness-mcp", "tool": "search"}}

OPERATOR_AUTHORITY = """\
@prefix vg:  <https://knuckles-team.github.io/ontology/virtual-graph#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix kg:  <https://knuckles-team.github.io/ontology/core#> .
@prefix ex:  <urn:eg:fixture:srcauth:> .

ex:source a vg:SourceConnection ; vg:sourceKind "mcp" ; vg:endpointRef ex:ep ;
    vg:foreignSourceName "SOURCE_NAME" ; vg:sourceRole "live_truth" .
ex:ep vg:url "https://harness.example.invalid/mcp" .
ex:contract a vg:MetadataContract ; vg:contractSource ex:source ; vg:schemaVersion "1" ;
    vg:contentDigest "sha256:0000000000000000000000000000000000000000000000000000000000000001" ;
    vg:discoveredEntity "incident" ; vg:discoveredField "incident.id", "incident.title" .
ex:mapping a vg:VirtualMapping ; vg:mapsContract ex:contract ;
    vg:mapsOntologyClass kg:Incident ; vg:mapsEntity "incident" ; vg:keyField "incident.id" ;
    vg:approved true ; vg:hasPredicateFieldPair ex:title .
ex:title a vg:PredicateFieldPair ; vg:predicate rdfs:label ; vg:field "incident.title" .
ex:authority a vg:SourceAuthority ; vg:authoritativeSource ex:source ;
    vg:authorityFor kg:Incident ; vg:authorityMapping ex:mapping ;
    vg:authorityRank 1 ; vg:authorityContract "mcp" .
"""


@pytest.mark.spec("MEDULLA-SRC-AUTH-R003.6")
@pytest.mark.asyncio
async def test_harness_attaches_a_contract_and_reads_authority_rows() -> None:
    tag = uuid.uuid4().hex[:8]
    source_name = f"srcauth-harness-{tag}"
    approved_id = f"approved:srcauth-{tag}"
    lease_id = f"approval:srcauth-{tag}"
    async with governed_graph() as ledger:
        # The proposer connects the foreign source, so the coverage verdict can cover it.
        await ledger.proposer._send(
            "RegisterForeignSource",
            {"name": source_name, "source": SOURCE_SOURCE},
            None,
            idempotency_key=f"register-{tag}",
        )
        await attach_operator_authority(
            ledger,
            f"admin:srcauth-{tag}",
            OPERATOR_AUTHORITY.replace("SOURCE_NAME", source_name),
        )

        # An approved record contract: one conforming record, issue, consume, validate,
        # attach.
        await ledger.admin.nodes.add(
            f"record-{tag}",
            {"type": f"urn:eg:source:srcauth-{tag}#Record", "id": 1, "name": "alpha"},
        )
        fields = BASELINE_FIELDS_CONTRACT["fields"]
        await issue_approval_lease(
            ledger.admin,
            lease_id,
            candidate_digest(BASELINE_FIELDS_CONTRACT, approved_id),
            approved_id,
        )
        await consume_approval_lease(ledger.admin, lease_id)
        await validate_repair(ledger.admin, fields, approved_id)
        await attach_approved(ledger.admin, lease_id, fields, approved_id)

        verdicts = await authority_rows(ledger.proposer, [CLASS_C])
        covered = [row for row in verdicts if row["class"] == CLASS_C]
        assert len(covered) == 1
        assert covered[0]["verdict"] == "covered", covered[0].get("premises")
        assert covered[0]["source"] == source_name

        listed = await reasoning.send_graph_schema_list(ledger.admin, {})
        approved = [s for s in listed.dynamic_sources if s.source_id == approved_id]
        assert len(approved) == 1
        assert isinstance(approved[0].origin, SchemaSourceOriginViewApproved)
        assert approved[0].origin.approval_lease_id == lease_id
