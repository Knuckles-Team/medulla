# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-types (the source of truth).
"""Generated OwlReason nested wire DTOs."""

from __future__ import annotations

from enum import Enum
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field

from ._shared import Digest256


class DatalogReasoningResult(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    inferred_count: Annotated[int, Field(ge=0)]
    inferred_triples: list[dict[str, str]]
    schema_digests: list[str]


class OriginKind(str, Enum):
    """Where a temporal premise's authority comes from.

    Spec row: MEDULLA-FEDERATED-QUERY-R006.1.
    """

    SENTENCE = "sentence"
    AXIOM = "axiom"


class DerivedFact(BaseModel):
    """Origin record of one temporal rule-derived fact."""

    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    fact_id: str
    rule_digest: Digest256
    premise_ids: list[str]
    origin_kind: OriginKind
    origin_ref: str
    graph_generation: Annotated[int, Field(ge=0)]


class DerivedFactRecord(BaseModel):
    """One persisted revision of a derived fact over `[valid_from, valid_to)`."""

    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    fact: DerivedFact
    valid_from: int
    valid_to: int
    revision: Annotated[int, Field(ge=0)]
    tombstone: bool


class DerivationStep(BaseModel):
    """One step of the `RunTemporalRules` derivation trace."""

    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    order: Annotated[int, Field(ge=0)]
    round: Annotated[int, Field(ge=0)]
    fact_id: str
    premise_ids: list[str]
    valid_from: int
    valid_to: int


class RunTemporalRulesResult(BaseModel):
    """Reply of the served `RunTemporalRules` write."""

    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    rule_digest: Digest256
    graph_generation: Annotated[int, Field(ge=0)]
    committed_version: Annotated[int, Field(ge=0)]
    facts: list[DerivedFactRecord]
    trace: list[DerivationStep]


class OntologyClassView(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    comment: str | None = None
    iri: str
    label: str | None = None
    parents: list[str]


class OntologyCoverageView(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    declared: Annotated[int, Field(ge=0)]
    graph: str = ""
    mapped: list[str]
    pack_digest: str = ""
    pack_revision: str = ""
    schema_digest: str = ""
    source_digest: str
    source_id: str
    source_revision: str
    tenant: str | None = None
    unmapped: list[OntologyUnmappedClassView]


class OntologyCrosswalkView(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    crosswalk_digest: str
    origin: str
    relation: str
    source_class_iri: str
    source_id: str
    source_revision: str
    target_class_iri: str
    target_vocabulary_revision: str


class OntologyInspection(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    canonical_digest: str | None = None
    classes: list[OntologyClassView]
    committed: bool = False
    composed_digest: str | None = None
    coverage: list[OntologyCoverageView] = Field(default_factory=list)
    crosswalks: list[OntologyCrosswalkView] = Field(default_factory=list)
    datatype_properties: list[OntologyPropertyView]
    external_coverage: list[OntologyExternalCoverageView] = Field(default_factory=list)
    object_properties: list[OntologyPropertyView]
    ontologies: list[str]
    schema_digests: list[str]
    shape_target_classes: list[str]
    triple_count: Annotated[int, Field(ge=0)]


class OntologyPropertyView(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    comment: str | None = None
    domains: list[str]
    iri: str
    label: str | None = None
    ranges: list[str]
    symmetric: bool


class ExternalOntologyCoverage(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    coverage_digest: Digest256
    mapping_revision: str
    pack_revision: str
    platform_vocabulary_revision: str
    schema_revision: str
    schema_version: Annotated[int, Field(ge=0, le=65535)]
    source_revision: str
    source_schema_digest: Digest256
    terms: list[ExternalTerm]


class ExternalTermKind(str, Enum):
    CLASS = "class"
    OBJECT_PROPERTY = "object_property"
    DATATYPE_PROPERTY = "datatype_property"
    ANNOTATION_PROPERTY = "annotation_property"
    NAMED_INDIVIDUAL = "named_individual"


class ExternalTermMapped(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    state: Literal["mapped"] = "mapped"
    platform_iri: str
    provenance_digest: Digest256


class ExternalUnmappedReason(str, Enum):
    NO_DECLARED_MAPPING = "no_declared_mapping"


class ExternalTermUnmapped(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    state: Literal["unmapped"] = "unmapped"
    reason: ExternalUnmappedReason


ExternalTermResult = Annotated[
    ExternalTermMapped | ExternalTermUnmapped, Field(discriminator="state")
]


class ExternalTerm(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    external_iri: str
    kind: ExternalTermKind
    result: ExternalTermResult


class OntologyExternalCoverageView(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    coverage: ExternalOntologyCoverage
    graph: str = ""
    source_id: str
    tenant: str | None = None


class OntologyUnmappedClassView(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    class_iri: str
    reason: str


class OwlExplainResult(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    consistent: bool
    found: bool
    schema_digests: list[str]
    tree: ProofNodeWire | None = None
    unsatisfiable: list[str]


class OwlPropertyFact(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    asserted: bool
    axiom: str | None = None
    object: str
    predicate: str
    premises: list[OwlPropertyFactPremisesItem]
    rule: str
    subject: str


class OwlReasonResult(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    consistent: bool
    direct_subclasses: list[OwlReasonResultDirectSubclassesItem]
    instance_conf: list[float]
    instances: list[OwlReasonResultInstancesItem]
    property_facts: list[OwlPropertyFact]
    schema_digests: list[str]
    subclass_conf: list[float]
    subclasses: list[OwlReasonResultSubclassesItem]
    unsatisfiable: list[str]


class ProofNodeWire(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    axioms: list[str]
    confidence: float
    premises: list[ProofNodeWire]
    rule: str
    sub: str
    sup: str


class ShaclSeverity(str, Enum):
    VIOLATION = "Violation"
    WARNING = "Warning"
    INFO = "Info"


class ShaclValidationReport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    composed_digest: str | None = None
    conforms: bool
    results: list[ShaclValidationResult]
    schema_digests: list[str]


class ShaclValidationResult(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    constraint_component: str
    focus_node: str
    message: str | None = None
    path: str | None = None
    severity: ShaclSeverity
    source_shape: str
    value: str | None = None


OwlPropertyFactPremisesItem = Annotated[
    tuple[str, str, str],
    Field(
        min_length=3,
        max_length=3,
    ),
]


OwlReasonResultDirectSubclassesItem = Annotated[
    tuple[str, str],
    Field(
        min_length=2,
        max_length=2,
    ),
]


OwlReasonResultInstancesItem = Annotated[
    tuple[str, str],
    Field(
        min_length=2,
        max_length=2,
    ),
]


OwlReasonResultSubclassesItem = Annotated[
    tuple[str, str],
    Field(
        min_length=2,
        max_length=2,
    ),
]

DatalogReasoningResult.model_rebuild()

OntologyClassView.model_rebuild()

OntologyCrosswalkView.model_rebuild()

OntologyInspection.model_rebuild()

OntologyPropertyView.model_rebuild()

OwlExplainResult.model_rebuild()

OwlPropertyFact.model_rebuild()

OwlReasonResult.model_rebuild()

ProofNodeWire.model_rebuild()

ShaclValidationReport.model_rebuild()

ShaclValidationResult.model_rebuild()


__all__ = [
    "DatalogReasoningResult",
    "DerivationStep",
    "DerivedFact",
    "DerivedFactRecord",
    "ExternalOntologyCoverage",
    "ExternalTerm",
    "ExternalTermKind",
    "ExternalTermMapped",
    "ExternalTermResult",
    "ExternalTermUnmapped",
    "ExternalUnmappedReason",
    "OntologyClassView",
    "OntologyCoverageView",
    "OntologyCrosswalkView",
    "OntologyInspection",
    "OntologyExternalCoverageView",
    "OntologyPropertyView",
    "OntologyUnmappedClassView",
    "OriginKind",
    "OwlExplainResult",
    "OwlPropertyFact",
    "OwlPropertyFactPremisesItem",
    "OwlReasonResult",
    "OwlReasonResultDirectSubclassesItem",
    "OwlReasonResultInstancesItem",
    "OwlReasonResultSubclassesItem",
    "ProofNodeWire",
    "RunTemporalRulesResult",
    "ShaclSeverity",
    "ShaclValidationReport",
    "ShaclValidationResult",
]
