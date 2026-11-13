# Wire models for the engine methods, maintained by hand alongside the Rust
# serde types in crates/medulla-agent-types (the source of truth).
"""Generated ConceptLineage nested wire DTOs."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field

from ._shared import AgentLibraryMutationContext


class ConceptRecord(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    admitted_at_ms: Annotated[int, Field(ge=0)]
    concept_id: str
    repository: str


class ConceptEdgeRecord(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    admitted_at_ms: Annotated[int, Field(ge=0)]
    repository: str
    successor: str
    superseded: str


class ConceptReadResult(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    concept: ConceptRecord | None = None
    successors: list[ConceptEdgeRecord]
    truncated: bool


class ConceptLineageOpClaim(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    concept_id: str
    context: AgentLibraryMutationContext
    op: Literal["claim"]
    repository: str


class ConceptLineageOpAddEdge(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    context: AgentLibraryMutationContext
    op: Literal["add_edge"]
    repository: str
    successor: str
    superseded: str


class ConceptLineageOpRead(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, defer_build=True)

    concept_id: str
    op: Literal["read"]
    tenant_id: str


ConceptLineageOp = Annotated[
    ConceptLineageOpClaim | ConceptLineageOpAddEdge | ConceptLineageOpRead,
    Field(discriminator="op"),
]

__all__ = [
    "ConceptEdgeRecord",
    "ConceptLineageOp",
    "ConceptLineageOpAddEdge",
    "ConceptLineageOpClaim",
    "ConceptLineageOpRead",
    "ConceptReadResult",
    "ConceptRecord",
]
