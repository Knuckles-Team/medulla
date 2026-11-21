# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-types (the source of truth).
"""Generated SourceIngest nested wire DTOs."""

from __future__ import annotations

from enum import Enum
from typing import Annotated, Any, Literal

from pydantic import (
    BaseModel,
    BeforeValidator,
    ConfigDict,
    Field,
    PlainSerializer,
    field_validator,
)

from ._shared import (
    Digest256,
    EnumerationPage,
    McpCatalogSnapshotBinding,
    SourceEntityRef,
    SqlColumnProfile,
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
    SKILL_DOCUMENTS = "skill_documents"
    ACTION_RECORDS = "action_records"


SOURCE_BATCH_EXTENSION_BOUNDS = {
    "access": 1024,
    "groups": 64,
    "enumeration": 16384,
    "containers": 1024,
    "proposals": 64,
    "facts": 1024,
    "transcripts": 1024,
    "skill_documents": 64,
    "action_records": 256,
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


class SourceBatchExtensionAccess(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    access: Annotated[list[SourceJson], Field(max_length=1024)]


class SourceBatchExtensionGroups(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    groups: Annotated[list[SourceJson], Field(max_length=64)]


class SourceBatchExtensionEnumeration(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    enumeration: EnumerationPage


class SourceBatchExtensionContainers(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    containers: Annotated[list[SourceJson], Field(max_length=1024)]


class SourceBatchExtensionProposals(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    proposals: Annotated[list[SourceJson], Field(max_length=64)]


class SourceBatchExtensionFacts(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    facts: Annotated[list[SourceJson], Field(max_length=1024)]


class SourceBatchExtensionTranscripts(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    transcripts: Annotated[list[SourceJson], Field(max_length=1024)]


class SourceBatchExtensionSkillDocuments(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    skill_documents: Annotated[list[SourceJson], Field(max_length=64)]


class SourceBatchExtensionActionRecords(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    action_records: Annotated[list[SourceJson], Field(max_length=256)]


SourceBatchExtension = (
    dict
    | SourceBatchExtensionAccess
    | SourceBatchExtensionGroups
    | SourceBatchExtensionEnumeration
    | SourceBatchExtensionContainers
    | SourceBatchExtensionProposals
    | SourceBatchExtensionFacts
    | SourceBatchExtensionTranscripts
    | SourceBatchExtensionSkillDocuments
    | SourceBatchExtensionActionRecords
)


def _feature_octets(value: Any) -> bytes:
    if isinstance(value, list):
        if any(type(item) is not int or not 0 <= item <= 255 for item in value):
            raise ValueError("engine octets must be integers between 0 and 255")
        return bytes(value)
    if not isinstance(value, bytes):
        raise ValueError("engine octets must be bytes or an integer array")
    return value


class SourceFeature(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    feature_id: str
    kind: str
    model_version: str
    object_id: str
    value_msgpack: Annotated[
        bytes,
        BeforeValidator(_feature_octets),
        PlainSerializer(list, return_type=list[int], when_used="json"),
    ]


BoundedVec_SourceFeature_1024 = Annotated[list[SourceFeature], Field(max_length=1024)]


class SourceIngestionBatch(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    authoritative_live_ids: BoundedVec_SourceEntityRef_16384 | None = None
    connector: str
    empty_authoritative_approval: str | None = None
    expected_previous_checkpoint: SourceCheckpoint | None = None
    extension: SourceBatchExtension | None = None
    features: BoundedVec_SourceFeature_1024 = Field(default_factory=list)
    kind: SourceBatchKind = SourceBatchKind.RECORDS
    mode: SourceIngestionMode
    provider_checkpoint: SourceCheckpoint
    records: BoundedVec_SourceRecord_1024
    relationships: BoundedVec_SourceRelationship_4096 = Field(default_factory=list)
    source_chunk: str | None = None
    strict_schema: bool
    withdrawals: BoundedVec_SourceWithdrawal_1024 = Field(default_factory=list)


class SourceIngestionDisposition(str, Enum):
    COMMITTED = "committed"
    REPLAYED = "replayed"


class SourceIngestionMode(str, Enum):
    FULL = "full"
    DELTA = "delta"
    RECONCILE = "reconcile"


class AccessReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    refused: Annotated[int, Field(ge=0)]
    stale: Annotated[int, Field(ge=0)]
    updated: Annotated[int, Field(ge=0)]


class ReconcileReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    deleted_count: Annotated[int, Field(ge=0)]
    ids_digest: str
    missing_count: Annotated[int, Field(ge=0)]
    override_principal: str


class GovernanceReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    governed: Annotated[int, Field(ge=0)]
    marked: Annotated[int, Field(ge=0)]
    refused: Annotated[int, Field(ge=0)]


class DocumentReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    atoms_emitted: Annotated[int, Field(ge=0)] = 0
    atoms_refused: Annotated[int, Field(ge=0)] = 0
    duplicates: Annotated[int, Field(ge=0)] = 0


class PaperRefusal(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    record_id: str
    code: str


class PaperReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    refusals: list[PaperRefusal] = Field(default_factory=list)


class FactOutcome(str, Enum):
    CREATED = "created"
    UPDATED = "updated"
    UNCHANGED = "unchanged"
    REJECTED = "rejected"


class FactUnit(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    claim_id: str
    unit_hash: str
    outcome: FactOutcome
    detail: str = ""


class FactChunk(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    chunk_id: str
    content_hash: str
    created: Annotated[int, Field(ge=0)] = 0
    updated: Annotated[int, Field(ge=0)] = 0
    unchanged: Annotated[int, Field(ge=0)] = 0
    rejected: Annotated[int, Field(ge=0)] = 0
    superseded: Annotated[int, Field(ge=0)] = 0


class FactReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    created: Annotated[int, Field(ge=0)] = 0
    updated: Annotated[int, Field(ge=0)] = 0
    unchanged: Annotated[int, Field(ge=0)] = 0
    rejected: Annotated[int, Field(ge=0)] = 0
    links: Annotated[int, Field(ge=0)] = 0
    entries: Annotated[list[FactUnit], Field(max_length=1024)] = Field(
        default_factory=list
    )
    chunks: list[FactChunk] = Field(default_factory=list)


class RowVersionReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    refused: Annotated[int, Field(ge=0)]
    stored: Annotated[int, Field(ge=0)]
    superseded: Annotated[int, Field(ge=0)]


class IngestBatchReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    refused: Annotated[int, Field(ge=0)]
    unchanged: Annotated[int, Field(ge=0)]
    written: Annotated[int, Field(ge=0)]


class ClassReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    ontology_class: str
    report: IngestBatchReport


class ColumnExclusionReason(str, Enum):
    FOREIGN_KEY = "foreign_key"
    BOOLEAN = "boolean"
    TIMESTAMP = "timestamp"
    NUMERIC = "numeric"
    ENUMERATION = "enumeration"
    INSUFFICIENT_SAMPLES = "insufficient_samples"


class ColumnExclusion(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    column: str
    reason: ColumnExclusionReason
    stream: str


class EmbeddingSetEntry(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    column: str
    record_id: str
    stream: str


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


class TranscriptReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    accepted_checkpoint_digest: Digest256
    concept_link_count: Annotated[int, Field(ge=0)]
    connector: str
    dialect: str
    message_count: Annotated[int, Field(ge=0)]
    session_id: str
    source_digest: Digest256
    stream: str


class SourceIngestionReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    accepted_checkpoint: SourceCheckpoint
    accepted_checkpoint_digest: Digest256
    access: AccessReport | None = None
    affected_count: Annotated[int, Field(ge=0)]
    batch_digest: Digest256
    classes: Annotated[list[ClassReport], Field(max_length=1024)]
    column_exclusions: list[ColumnExclusion] = Field(default_factory=list)
    committed_graph_version: Annotated[int, Field(ge=0)]
    content_hash: Digest256 | None = None
    disposition: SourceIngestionDisposition
    documents: DocumentReport = Field(default_factory=DocumentReport)
    papers: PaperReport = Field(default_factory=PaperReport)
    drift: list[StreamDrift] = Field(default_factory=list)
    facts: FactReport = Field(default_factory=FactReport)
    embedding_set: list[EmbeddingSetEntry] = Field(default_factory=list)
    governance: GovernanceReport
    live_set_digest: Digest256 | None = None
    mappings: BoundedVec_SourceMappingReceipt_5120
    mode: SourceIngestionMode
    previous_content_digest: Digest256 | None = None
    raw_admissions: BoundedVec_RawAdmissionReceipt_1024
    receipt_digest: Digest256
    receipt_id: str
    reconcile: ReconcileReport | None = None
    row_versions: RowVersionReport = Field(
        default_factory=lambda: RowVersionReport(refused=0, stored=0, superseded=0)
    )
    relationship_count: Annotated[int, Field(ge=0)]
    report: IngestBatchReport
    relationship_live_set_digest: Digest256 | None = None
    relationship_raw_admissions: BoundedVec_RawRelationshipAdmissionReceipt_4096
    relationship_tombstoned_count: Annotated[int, Field(ge=0)]
    relationship_tombstones: BoundedVec_SourceRelationshipTombstoneReceipt_4096
    stream_object_id: str
    tombstoned_count: Annotated[int, Field(ge=0)]
    tombstones: BoundedVec_SourceTombstoneReceipt_16384
    transcript: TranscriptReceipt | None = None


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

    column_profiles: dict[str, SqlColumnProfile] | None = None
    mapping_reference: str
    payload: SourceJson
    provenance: SourceRecordProvenance
    record_id: str
    stream: str
    updated_at: str | None = None

    @field_validator("column_profiles")
    @classmethod
    def _empty_profiles_are_absent(
        cls, value: dict[str, SqlColumnProfile] | None
    ) -> dict[str, SqlColumnProfile] | None:
        # Rust omits an empty map on the wire, so `{}` and None are one value.
        return value or None


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
        if self.features:
            projection["features"] = [
                feature.model_dump(mode="python") for feature in self.features
            ]
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
                "records[*].column_profiles",
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
                    "column_profiles",
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


class QuarantineReason(str, Enum):
    NORMALIZATION_FAILED = "normalization_failed"
    MISSING_PROVENANCE = "missing_provenance"
    MAPPING_REVISION_MISMATCH = "mapping_revision_mismatch"


class ReplayDisposition(str, Enum):
    REPLAYABLE = "replayable"
    TERMINAL = "terminal"


class QuarantineReceipt(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    committed_graph_version: Annotated[int, Field(ge=0)]
    mapping_revision: str
    prior_accepted_checkpoint: SourceCheckpoint | None = None
    reason: QuarantineReason
    replay: ReplayDisposition
    restricted_evidence_ref: str
    rule_id: str
    source_digest: Digest256


class QuarantineEvidence(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    mapping_revision: str
    prior_accepted_checkpoint: SourceCheckpoint | None = None
    reason: QuarantineReason
    replay: ReplayDisposition
    restricted_evidence_ref: str
    rule_id: str
    source_digest: Digest256


class QuarantineReadView(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    quarantine: QuarantineEvidence | None = None


class TranscriptReadMessage(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    concept_refs: list[str] = Field(default_factory=list)
    event_time_ms: int | None = None
    message_id: str
    narrative_time_ms: int | None = None
    role: Literal["system", "user", "assistant", "tool"]
    sequence: Annotated[int, Field(ge=0)]
    source_record_id: str
    source_uri: str
    storage_time_ms: int | None = None
    text: str


class TranscriptThreadRead(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    dialect: str
    message_count: Annotated[int, Field(ge=0)]
    messages: list[TranscriptReadMessage]
    next_after_sequence: Annotated[int, Field(ge=0)] | None = None
    thread_id: str


class TranscriptReadView(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    thread: TranscriptThreadRead | None = None


class SourceIngestionAccepted(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    outcome: Literal["accepted"]
    receipt: SourceIngestionReceipt


class SourceIngestionQuarantined(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    outcome: Literal["quarantined"]
    receipt: QuarantineReceipt


SourceIngestionOutcome = Annotated[
    SourceIngestionAccepted | SourceIngestionQuarantined,
    Field(discriminator="outcome"),
]

RawAdmissionReceipt.model_rebuild()

RawRelationshipAdmissionReceipt.model_rebuild()

SourceCheckpoint.model_rebuild()

SourceEntityRef.model_rebuild()

SourceIngestStatus.model_rebuild()

SourceIngestionBatch.model_rebuild()

DocumentReport.model_rebuild()
PaperRefusal.model_rebuild()
PaperReport.model_rebuild()
FactUnit.model_rebuild()
FactReport.model_rebuild()

RowVersionReport.model_rebuild()

GovernanceReport.model_rebuild()
TranscriptReceipt.model_rebuild()

SourceIngestionReceipt.model_rebuild()

QuarantineReceipt.model_rebuild()

QuarantineEvidence.model_rebuild()

QuarantineReadView.model_rebuild()

TranscriptReadMessage.model_rebuild()

TranscriptThreadRead.model_rebuild()

TranscriptReadView.model_rebuild()

SourceIngestionAccepted.model_rebuild()

SourceIngestionQuarantined.model_rebuild()

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
    "BoundedVec_SourceFeature_1024",
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
    "SourceFeature",
    "SourceIngestStatus",
    "SourceIngestionBatch",
    "SourceIngestionDisposition",
    "SourceIngestionMode",
    "DocumentReport",
    "PaperRefusal",
    "PaperReport",
    "FactChunk",
    "FactOutcome",
    "FactReport",
    "FactUnit",
    "RowVersionReport",
    "GovernanceReport",
    "IngestBatchReport",
    "ClassReport",
    "ColumnExclusionReason",
    "ColumnExclusion",
    "EmbeddingSetEntry",
    "TranscriptReceipt",
    "SourceIngestionReceipt",
    "QuarantineReason",
    "QuarantineReceipt",
    "QuarantineEvidence",
    "QuarantineReadView",
    "TranscriptReadMessage",
    "TranscriptThreadRead",
    "TranscriptReadView",
    "ReplayDisposition",
    "SourceIngestionAccepted",
    "SourceIngestionOutcome",
    "SourceIngestionQuarantined",
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
