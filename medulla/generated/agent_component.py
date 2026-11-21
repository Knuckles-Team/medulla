# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-types (the source of truth).
"""Generated AgentComponent nested wire DTOs."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import BaseModel, BeforeValidator, ConfigDict, Field, PlainSerializer

from ._shared import (
    AgentComponentEntry,
    AgentComponentFacts,
    AgentComponentFactsModelProfile,
    AgentComponentFactsOpaque,
    AgentComponentFactsSystemPrompt,
    AgentComponentFactsTool,
    AgentComponentFactsToolset,
    AgentComponentKind,
    AgentLibraryLifecycle,
    AgentLibraryMutationContext,
    ComponentDependency,
    ComponentProvenance,
    ComponentProvenanceMcpServer,
    ComponentProvenanceNative,
    ComponentProvenanceSourcePackage,
    CostFacts,
    DeclaredLatency,
    FactQuality,
    MeasuredFact_ShellProficiency,
    MeasuredFact_StoppingBehaviour,
    ModalityFacts,
    ModelInputCapabilities,
    ModelInputMediaType,
    Nonce,
    ObservationRef,
    PriceSource,
    PriceSourceConnectorPack,
    PriceSourceOperator,
    PriceSourcePublisher,
    PromptMode,
    ShellProficiency,
    StoppingBehaviour,
    ToolEffect,
    ToolFactDeclarer,
    ToolsetTransport,
    ToolSideEffects,
)


def _eg_octets(value):
    if isinstance(value, list):
        if any(type(item) is not int or not 0 <= item <= 255 for item in value):
            raise ValueError("engine octets must be integers between 0 and 255")
        return bytes(value)
    if not isinstance(value, bytes):
        raise ValueError("engine octets must be bytes or an integer array")
    return value


_EgOctets = Annotated[
    bytes,
    BeforeValidator(_eg_octets),
    PlainSerializer(list, return_type=list[int], when_used="json"),
]


class AgentComponentContentRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component_id: str
    entry_revision: Annotated[int, Field(ge=0)] | None = None
    tenant_id: str


class AgentComponentContentResult(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    body: _EgOctets
    component_id: str
    content_digest: str
    definition_digest: str
    entry_revision: Annotated[int, Field(ge=0)]
    media_type: str
    schema_version: Annotated[int, Field(ge=0, le=65535)]


class AgentComponentBlueprintExportRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component_id: str
    entry_revision: Annotated[int, Field(ge=0)]
    tenant_id: str


class BlueprintExport(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    body: _EgOctets
    content_digest: str
    entry: AgentComponentEntry
    media_type: str
    schema_version: Annotated[int, Field(ge=0, le=65535)]


class SourceEffectRecordResult(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component_id: str
    receipt_digest: str
    recorded: bool
    revision: Annotated[int, Field(ge=0)]


class AgentComponentDraft(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    actor_scope: str
    attributes: dict[str, str] | None = None
    classification: list[str] = Field(default_factory=list)
    component_id: str
    content_digest: str
    content_ref: str | None = None
    declared_capabilities: list[str] = Field(default_factory=list)
    declared_required_capabilities: list[str] = Field(default_factory=list)
    facts: AgentComponentFacts
    kind: AgentComponentKind
    policy_digest: str
    provenance: ComponentProvenance
    purpose_id: str
    required_capabilities: list[str] = Field(default_factory=list)
    requires: list[ComponentDependency] = Field(default_factory=list)
    source_revision: str
    source_revision_digest: str
    summary: str
    tenant_id: str
    version: str


class AgentComponentOpPublish(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    op: Literal["publish"]
    request: AgentComponentPublishRequest


class AgentComponentOpRetire(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    op: Literal["retire"]
    request: AgentComponentRetireRequest


class AgentComponentOpCurrent(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component_id: str
    op: Literal["current"]
    tenant_id: str


class AgentComponentOpHistory(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component_id: str
    op: Literal["history"]
    tenant_id: str


class AgentComponentOpStatus(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    op: Literal["status"]
    request: AgentComponentStatusRequest


class AgentComponentOpSearch(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    op: Literal["search"]
    request: AgentComponentSearchRequest


class AgentComponentOpContent(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    op: Literal["content"]
    request: AgentComponentContentRequest


class SkillContractFieldWire(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    name: str
    required: bool
    type_name: str


class SkillMigrationWire(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    consumer_contract_digest: str
    migration_digest: str
    migration_id: str
    new_package_digest: str
    previous_package_digest: str
    receipt_passed: bool


class SkillAdmissionRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    cohort: list[str] = Field(default_factory=list)
    component_id: str
    held_admission_revision: Annotated[int, Field(ge=0)] | None = None
    migration: SkillMigrationWire | None = None
    new_contract: list[SkillContractFieldWire] = Field(default_factory=list)
    new_package_digest: str
    previous_contract: list[SkillContractFieldWire] = Field(default_factory=list)
    previous_package_digest: str
    tenant_id: str


class SkillAdmission(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    admission_revision: Annotated[int, Field(ge=0)]
    cohort: list[str]
    migration_digest: str | None = None
    migration_id: str | None = None
    new_contract_digest: str
    new_package_digest: str
    previous_contract_digest: str
    previous_package_digest: str
    verdict: Literal["compatible", "breaking_with_migration", "breaking_refused"]


class AgentComponentOpAdmitSkillUpgrade(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    op: Literal["admit_skill_upgrade"]
    request: SkillAdmissionRequest


class AgentComponentOpExportBlueprint(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    op: Literal["export_blueprint"]
    request: AgentComponentBlueprintExportRequest


class AgentComponentOpRecordSourceEffect(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component_id: str
    effect_revision: str
    op: Literal["record_source_effect"]
    path: str
    receipt_digest: str
    repository: str
    request_digest: str
    revision: Annotated[int, Field(ge=0)]
    tenant_id: str


AgentComponentOp = Annotated[
    AgentComponentOpPublish
    | AgentComponentOpRetire
    | AgentComponentOpCurrent
    | AgentComponentOpHistory
    | AgentComponentOpStatus
    | AgentComponentOpSearch
    | AgentComponentOpContent
    | AgentComponentOpAdmitSkillUpgrade
    | AgentComponentOpExportBlueprint
    | AgentComponentOpRecordSourceEffect,
    Field(discriminator="op"),
]


class AgentComponentPublishRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component: AgentComponentDraft
    context: AgentLibraryMutationContext
    evaluation_receipt_digest: str | None = None
    prompt_blueprint: str | None = None


class AgentComponentRetireRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component_id: str
    context: AgentLibraryMutationContext


class AgentComponentSearchPage(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    entries: list[AgentComponentEntry]
    next_cursor: str | None = None


class AgentComponentSearchRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    capabilities: list[str] = Field(default_factory=list)
    cursor: str | None = None
    kinds: list[AgentComponentKind] = Field(default_factory=list)
    limit: Annotated[int, Field(ge=0)] | None = None
    read_only: bool | None = None
    task: str | None = None
    tenant_id: str


class AgentComponentStatusRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    component_id: str
    context: AgentLibraryMutationContext
    kind: AgentComponentMutationKind


AgentComponentMutationKind = str | Literal["withdraw"] | Literal["republish"]

AgentComponentContentRequest.model_rebuild()

AgentComponentContentResult.model_rebuild()

AgentComponentBlueprintExportRequest.model_rebuild()

BlueprintExport.model_rebuild()

AgentComponentDraft.model_rebuild()

AgentComponentOpPublish.model_rebuild()

AgentComponentOpRetire.model_rebuild()

AgentComponentOpCurrent.model_rebuild()

AgentComponentOpHistory.model_rebuild()

AgentComponentOpStatus.model_rebuild()

AgentComponentOpSearch.model_rebuild()

AgentComponentOpContent.model_rebuild()

AgentComponentOpAdmitSkillUpgrade.model_rebuild()

AgentComponentOpExportBlueprint.model_rebuild()

AgentComponentOpRecordSourceEffect.model_rebuild()

SourceEffectRecordResult.model_rebuild()

SkillContractFieldWire.model_rebuild()

SkillMigrationWire.model_rebuild()

SkillAdmissionRequest.model_rebuild()

SkillAdmission.model_rebuild()

AgentComponentPublishRequest.model_rebuild()

AgentComponentRetireRequest.model_rebuild()

AgentComponentSearchPage.model_rebuild()

AgentComponentSearchRequest.model_rebuild()

AgentComponentStatusRequest.model_rebuild()


__all__ = [
    "AgentComponentOpRecordSourceEffect",
    "SourceEffectRecordResult",
    "AgentComponentBlueprintExportRequest",
    "AgentComponentOpExportBlueprint",
    "BlueprintExport",
    "AgentComponentContentRequest",
    "AgentComponentContentResult",
    "AgentComponentDraft",
    "AgentComponentEntry",
    "AgentComponentFacts",
    "AgentComponentFactsModelProfile",
    "AgentComponentFactsOpaque",
    "AgentComponentFactsSystemPrompt",
    "AgentComponentFactsTool",
    "AgentComponentFactsToolset",
    "AgentComponentKind",
    "AgentComponentMutationKind",
    "AgentComponentOp",
    "AgentComponentOpAdmitSkillUpgrade",
    "AgentComponentOpContent",
    "AgentComponentOpCurrent",
    "AgentComponentOpHistory",
    "AgentComponentOpPublish",
    "AgentComponentOpRetire",
    "AgentComponentOpSearch",
    "AgentComponentOpStatus",
    "AgentComponentPublishRequest",
    "AgentComponentRetireRequest",
    "AgentComponentSearchPage",
    "AgentComponentSearchRequest",
    "AgentComponentStatusRequest",
    "AgentLibraryLifecycle",
    "AgentLibraryMutationContext",
    "ComponentDependency",
    "ComponentProvenance",
    "ComponentProvenanceMcpServer",
    "ComponentProvenanceNative",
    "ComponentProvenanceSourcePackage",
    "CostFacts",
    "DeclaredLatency",
    "FactQuality",
    "MeasuredFact_ShellProficiency",
    "MeasuredFact_StoppingBehaviour",
    "ModalityFacts",
    "ModelInputCapabilities",
    "ModelInputMediaType",
    "Nonce",
    "ObservationRef",
    "PriceSource",
    "PriceSourceConnectorPack",
    "PriceSourceOperator",
    "PriceSourcePublisher",
    "PromptMode",
    "ShellProficiency",
    "StoppingBehaviour",
    "ToolEffect",
    "ToolFactDeclarer",
    "ToolSideEffects",
    "ToolsetTransport",
    "SkillAdmission",
    "SkillAdmissionRequest",
    "SkillContractFieldWire",
    "SkillMigrationWire",
]
