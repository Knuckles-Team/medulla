# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-types (the source of truth).
"""Generated SourceIngest nested wire DTOs."""

from __future__ import annotations

from enum import Enum
from typing import Annotated, Any

from pydantic import AfterValidator, BaseModel, ConfigDict, Field

from ._shared import (
    Digest256,
    McpCatalogSnapshotBinding,
)
from .digest import (
    framed_named_msgpack_digest,
)


class RawAdmissionReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    deduplicated: bool
    raw_digest: Digest256
    record_id: str
    stream: str


class RawRelationshipAdmissionReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    deduplicated: bool
    raw_digest: Digest256
    relationship_id: str


class SourceCheckpoint(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    content_hash: Digest256 | None = None
    pending_watermark: str | None = None
    position: SourceJson
    stream: str
    watermark: str | None = None


class SourceEntityRef(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    record_id: str
    stream: str


class SourceIngestStatus(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    accepted_checkpoint: SourceCheckpoint | None = None
    accepted_checkpoint_digest: Digest256 | None = None
    committed_graph_version: Annotated[int, Field(ge=0)] | None = None
    connector: str
    content_hash: Digest256 | None = None
    last_batch_digest: Digest256 | None = None
    last_receipt_id: str | None = None
    live_set_digest: Digest256 | None = None
    relationship_live_set_digest: Digest256 | None = None
    stream: str


class SourceBatchKind(str, Enum):
    RECORDS = "records"
    ACCESS = "access"
    GROUPS = "groups"
    ENUMERATION = "enumeration"
    CONTAINERS = "containers"
    PROPOSALS = "proposals"
    FACTS = "facts"
    TRANSCRIPTS = "transcripts"


SOURCE_BATCH_EXTENSION_BOUNDS = {
    "access": 1024,
    "groups": 64,
    "enumeration": 16384,
    "containers": 1024,
    "proposals": 64,
    "facts": 1024,
    "transcripts": 1024,
}


def _single_tagged_extension(value: dict[str, list[Any]]) -> dict[str, list[Any]]:
    """Mirror the Rust externally tagged enum: one known key within its bound."""
    if len(value) != 1:
        raise ValueError("a batch extension carries exactly one kind")
    ((key, items),) = value.items()
    bound = SOURCE_BATCH_EXTENSION_BOUNDS.get(key)
    if bound is None or len(items) > bound:
        raise ValueError(f"unknown or oversized batch extension: {key}")
    return value


SourceBatchExtension = Annotated[
    dict[str, list[Any]], AfterValidator(_single_tagged_extension)
]


class SourceIngestionBatch(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    authoritative_live_ids: BoundedVec_SourceEntityRef_16384 | None = None
    connector: str
    empty_authoritative_approval: str | None = None
    expected_previous_checkpoint: SourceCheckpoint | None = None
    extension: SourceBatchExtension | None = None
    kind: SourceBatchKind = SourceBatchKind.RECORDS
    mode: SourceIngestionMode
    provider_checkpoint: SourceCheckpoint
    records: BoundedVec_SourceRecord_1024
    relationships: BoundedVec_SourceRelationship_4096 = Field(default_factory=list)
    strict_schema: bool
    withdrawals: BoundedVec_SourceWithdrawal_1024 = Field(default_factory=list)


class SourceIngestionDisposition(str, Enum):
    COMMITTED = "committed"
    REPLAYED = "replayed"


class SourceIngestionMode(str, Enum):
    FULL = "full"
    DELTA = "delta"
    RECONCILE = "reconcile"


class GovernanceReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    governed: Annotated[int, Field(ge=0)]
    marked: Annotated[int, Field(ge=0)]
    refused: Annotated[int, Field(ge=0)]


class DriftClass(str, Enum):
    COMPATIBLE = "compatible"
    WIDENING = "widening"
    NARROWING = "narrowing"
    RENAME = "rename"
    BREAKING = "breaking"


class EvolutionPolicy(str, Enum):
    QUARANTINE = "quarantine"
    APPLY_WIDENING = "apply_widening"
    APPLY_MONOTONIC = "apply_monotonic"


class DriftAction(str, Enum):
    ACCEPTED = "accepted"
    APPLIED = "applied"
    QUARANTINED = "quarantined"


class FieldDrift(str, Enum):
    UNCHANGED = "unchanged"
    WIDENED = "widened"
    NARROWED = "narrowed"
    RETYPED = "retyped"
    REQUIRED_MISSING = "required_missing"
    OPTIONAL_MISSING = "optional_missing"
    NEW_REQUIRED = "new_required"
    NEW_OPTIONAL = "new_optional"
    RENAMED_OUT = "renamed_out"
    RENAMED_IN = "renamed_in"


class DriftReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    action: DriftAction
    approved_digest: str
    class_: Annotated[DriftClass, Field(alias="class")]
    fields_: dict[str, FieldDrift] = Field(..., alias="fields")
    policy: EvolutionPolicy
    shape_digest: str


class StreamDrift(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    report: DriftReport
    stream: str


class SourceIngestionReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    accepted_checkpoint: SourceCheckpoint
    accepted_checkpoint_digest: Digest256
    affected_count: Annotated[int, Field(ge=0)]
    batch_digest: Digest256
    committed_graph_version: Annotated[int, Field(ge=0)]
    content_hash: Digest256 | None = None
    disposition: SourceIngestionDisposition
    drift: list[StreamDrift] = Field(default_factory=list)
    governance: GovernanceReport
    live_set_digest: Digest256 | None = None
    mappings: BoundedVec_SourceMappingReceipt_5120
    mode: SourceIngestionMode
    previous_content_digest: Digest256 | None = None
    raw_admissions: BoundedVec_RawAdmissionReceipt_1024
    receipt_digest: Digest256
    receipt_id: str
    relationship_count: Annotated[int, Field(ge=0)]
    relationship_live_set_digest: Digest256 | None = None
    relationship_raw_admissions: BoundedVec_RawRelationshipAdmissionReceipt_4096
    relationship_tombstoned_count: Annotated[int, Field(ge=0)]
    relationship_tombstones: BoundedVec_SourceRelationshipTombstoneReceipt_4096
    stream_object_id: str
    tombstoned_count: Annotated[int, Field(ge=0)]
    tombstones: BoundedVec_SourceTombstoneReceipt_16384


class SourceMappingKind(str, Enum):
    ENTITY = "entity"
    RELATIONSHIP = "relationship"


class SourceMappingReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    catalog: McpCatalogSnapshotBinding
    connector_pack_digest: Digest256
    kind: SourceMappingKind
    mapping_digest: Digest256
    mapping_reference: str


class SourceRecord(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    mapping_reference: str
    payload: SourceJson
    provenance: SourceRecordProvenance
    record_id: str
    stream: str
    updated_at: str | None = None


class SourceRecordProvenance(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    adapter_kind: str
    connector: str
    server: str
    source_uri: str
    tool: str
    tool_schema_sha256: Digest256


class SourceRelationship(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    properties: SourceJson | None = None
    provenance: SourceRecordProvenance
    relation_reference: str
    source: SourceEntityRef
    target: SourceEntityRef


class SourceRelationshipTombstoneReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    reason: str
    relationship_id: str
    source: SourceEntityRef
    target: SourceEntityRef


class SourceTombstoneReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    entity: SourceEntityRef
    node_id: str
    reason: str


class SourceWithdrawal(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    entity: SourceEntityRef
    reason: str


BoundedVec_RawAdmissionReceipt_1024 = Annotated[
    list[RawAdmissionReceipt],
    Field(
        max_length=1024,
    ),
]


BoundedVec_RawRelationshipAdmissionReceipt_4096 = Annotated[
    list[RawRelationshipAdmissionReceipt],
    Field(
        max_length=4096,
    ),
]


BoundedVec_SourceEntityRef_16384 = Annotated[
    list[SourceEntityRef],
    Field(
        max_length=16384,
    ),
]


BoundedVec_SourceMappingReceipt_5120 = Annotated[
    list[SourceMappingReceipt],
    Field(
        max_length=5120,
    ),
]


BoundedVec_SourceRecord_1024 = Annotated[list[SourceRecord], Field(max_length=1024)]


BoundedVec_SourceRelationshipTombstoneReceipt_4096 = Annotated[
    list[SourceRelationshipTombstoneReceipt],
    Field(
        max_length=4096,
    ),
]


BoundedVec_SourceRelationship_4096 = Annotated[
    list[SourceRelationship],
    Field(
        max_length=4096,
    ),
]


BoundedVec_SourceTombstoneReceipt_16384 = Annotated[
    list[SourceTombstoneReceipt],
    Field(
        max_length=16384,
    ),
]


BoundedVec_SourceWithdrawal_1024 = Annotated[
    list[SourceWithdrawal],
    Field(
        max_length=1024,
    ),
]


class SourceIngestionRequest(SourceIngestionBatch):
    def canonical_digest(self) -> str:
        value = self.model_dump(mode="json")
        projection = {
            "connector": value["connector"],
            "mode": value["mode"],
            "strict_schema": value["strict_schema"],
            "records": value["records"],
            "relationships": value["relationships"],
            "provider_checkpoint": value["provider_checkpoint"],
            "expected_previous_checkpoint": value["expected_previous_checkpoint"],
            "authoritative_live_ids": value["authoritative_live_ids"],
            "empty_authoritative_approval": value["empty_authoritative_approval"],
            "withdrawals": value["withdrawals"],
            "kind": value["kind"],
            "extension": value["extension"],
        }
        return framed_named_msgpack_digest(
            b"eg/source-ingestion-batch/v3",
            projection,
            canonical_json_paths={
                "records[*].payload",
                "relationships[*].properties",
                "provider_checkpoint.position",
                "expected_previous_checkpoint.position",
            },
            omit_none_paths={
                "records[*].updated_at",
                "relationships[*].properties",
                "provider_checkpoint.content_hash",
                "provider_checkpoint.watermark",
                "provider_checkpoint.pending_watermark",
                "expected_previous_checkpoint.content_hash",
                "expected_previous_checkpoint.watermark",
                "expected_previous_checkpoint.pending_watermark",
            },
            named_struct_paths={
                "records[*]": (
                    "stream",
                    "record_id",
                    "mapping_reference",
                    "payload",
                    "updated_at",
                    "provenance",
                ),
                "records[*].provenance": (
                    "connector",
                    "adapter_kind",
                    "server",
                    "tool",
                    "tool_schema_sha256",
                    "source_uri",
                ),
                "relationships[*]": (
                    "source",
                    "target",
                    "relation_reference",
                    "properties",
                    "provenance",
                ),
                "relationships[*].source": (
                    "stream",
                    "record_id",
                ),
                "relationships[*].target": (
                    "stream",
                    "record_id",
                ),
                "relationships[*].provenance": (
                    "connector",
                    "adapter_kind",
                    "server",
                    "tool",
                    "tool_schema_sha256",
                    "source_uri",
                ),
                "provider_checkpoint": (
                    "stream",
                    "position",
                    "content_hash",
                    "watermark",
                    "pending_watermark",
                ),
                "expected_previous_checkpoint": (
                    "stream",
                    "position",
                    "content_hash",
                    "watermark",
                    "pending_watermark",
                ),
                "authoritative_live_ids[*]": (
                    "stream",
                    "record_id",
                ),
                "withdrawals[*]": (
                    "entity",
                    "reason",
                ),
                "withdrawals[*].entity": (
                    "stream",
                    "record_id",
                ),
            },
        )


SourceJson = Any

RawAdmissionReceipt.model_rebuild()

RawRelationshipAdmissionReceipt.model_rebuild()

SourceCheckpoint.model_rebuild()

SourceEntityRef.model_rebuild()

SourceIngestStatus.model_rebuild()

SourceIngestionBatch.model_rebuild()

GovernanceReport.model_rebuild()
SourceIngestionReceipt.model_rebuild()

SourceMappingReceipt.model_rebuild()

SourceRecord.model_rebuild()

SourceRecordProvenance.model_rebuild()

SourceRelationship.model_rebuild()

SourceRelationshipTombstoneReceipt.model_rebuild()

SourceTombstoneReceipt.model_rebuild()

SourceWithdrawal.model_rebuild()


__all__ = [
    "BoundedVec_RawAdmissionReceipt_1024",
    "BoundedVec_RawRelationshipAdmissionReceipt_4096",
    "BoundedVec_SourceEntityRef_16384",
    "BoundedVec_SourceMappingReceipt_5120",
    "BoundedVec_SourceRecord_1024",
    "BoundedVec_SourceRelationshipTombstoneReceipt_4096",
    "BoundedVec_SourceRelationship_4096",
    "BoundedVec_SourceTombstoneReceipt_16384",
    "BoundedVec_SourceWithdrawal_1024",
    "Digest256",
    "McpCatalogSnapshotBinding",
    "RawAdmissionReceipt",
    "RawRelationshipAdmissionReceipt",
    "SourceCheckpoint",
    "SourceEntityRef",
    "SourceIngestStatus",
    "SourceIngestionBatch",
    "SourceIngestionDisposition",
    "SourceIngestionMode",
    "GovernanceReport",
    "SourceIngestionReceipt",
    "SourceIngestionRequest",
    "SourceJson",
    "SourceMappingKind",
    "SourceMappingReceipt",
    "SourceRecord",
    "SourceRecordProvenance",
    "SourceRelationship",
    "SourceRelationshipTombstoneReceipt",
    "SourceTombstoneReceipt",
    "SourceWithdrawal",
]
