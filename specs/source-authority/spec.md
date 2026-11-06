# MEDULLA-SRC-AUTH — Source authority: the ontology says which connected source answers a concept

Owner: Medulla. Cross-repo IDs: Arbora (served intent tool prints the "sources used and why" block, connected-source list), agent-packages connectors (operation contracts that a pack's mapping proposals cite). Consumers in this repository: `MEDULLA-CQ-XSRC-001` (`R001`, `R005`), `MEDULLA-CQ-ORCH-001` (`.3` children of `R001` to `R007`), `MEDULLA-CQ-CONN-001` (cross-source children).

## User Scenarios & Testing

### User Story 1 - Declare which source is authoritative for a concept (Priority: P1)

The user is an operator. The outcome is a statement in the ontology, such as "for `:Incident`, ServiceNow is authoritative at rank 1 and PagerDuty is the rank 2 fallback", that the engine validates before it takes effect.

**Acceptance scenarios**

1. **Given** a connected source with an approved mapping of `:Incident`, **When** the operator attaches a `vg:SourceAuthority` row naming that source, class, rank and contract kind, **Then** the shapes accept the row.
2. **Given** a row with no rank, a contract kind that contradicts the connection, or an accelerated policy without reason, **When** it is attached, **Then** the shapes refuse it with the message of the failed constraint.

Independent test: the bound tests of `MEDULLA-SRC-AUTH-R002.1` run this story against the registered shapes.

### User Story 2 - Only an operator asserts authority (Priority: P1)

The user is an operator reviewing a connector's proposal. The outcome is that a connector can propose a row and can never make it effective.

**Acceptance scenarios**

1. **Given** a connector pack that ships `vg:approved true`, **When** the pack attaches, **Then** the attach is refused with `APPROVAL_CLAIM_REFUSED`.
2. **Given** a candidate proposed by one operator, **When** a different operator approves its exact digest, **Then** the row attaches under the `Approved` origin; a changed byte or the same operator is refused.

Independent test: the bound tests of `MEDULLA-SRC-AUTH-R003.3` run this story through the served `GraphSchema` method.

### User Story 3 - Ask by class and get the right source or a typed abstention (Priority: P1)

The user is an analyst whose question names only classes. The outcome is the winning source per class with ordered alternates, or an abstention that names the source and the concept.

**Acceptance scenarios**

1. **Given** two approved rows for `:Incident` at ranks 1 and 2, **When** the analyst asks through `SourceCoverage`, **Then** rank 1 wins and rank 2 is listed as alternate.
2. **Given** the rank 1 source is not connected and the row does not allow fallback, **When** the analyst asks, **Then** the verdict is `Abstained` naming the source and the class.
3. **Given** two equal rows from different sources, **When** the analyst asks, **Then** the verdict is `Abstained::Ambiguous` listing both rows and never chooses by name.

Independent test: the bound tests of `MEDULLA-SRC-AUTH-R004.6.1` run this story through the served method.

### User Story 4 - See the sources used and why (Priority: P2)

The user is a reader of a cross-source report. The outcome is an audit that equals the selection premises, including the rows not used and their reason.

**Acceptance scenarios**

1. **Given** a served answer, **When** the reader reads `audit`, **Then** each used source carries its authority premise and mode, and each unused row carries its reason.
2. **Given** two ranks swapped in the ontology and equal query text, **When** the question is asked again, **Then** `sources_used` changes and the query text does not.

Independent test: the bound tests of `MEDULLA-SRC-AUTH-R009.4` run this story through the served method.

### User Story 5 - Prefer live truth and mark untrusted content (Priority: P2)

The user is an operator of a run that reads outside content. The outcome is that a live source outranks a historical one on conflict, an inspiration source never answers a current-state question, and content that enters a run carries a stored trust mark.

**Acceptance scenarios**

1. **Given** a `live_truth` and a `historical_context` source in conflict, **When** the question is asked, **Then** the live source wins whatever the rank.
2. **Given** a node written from a tool result, **When** a client sets `trust_mark=trusted` without approval, **Then** the write is refused with `TRUST_MARK_DOWNGRADE_REFUSED` and the node stays `untrusted`.

Independent test: the bound tests of `MEDULLA-SRC-AUTH-R010.6` and `MEDULLA-SRC-AUTH-R011.6` run this story through served methods.

### User Story 6 - Keep the graph as system of record while a secondary source syncs two ways (Priority: P2)

The user is an operator who connects a process engine, an architecture tool or a metadata store as a federated secondary. The outcome is that changes flow both ways and a conflict follows a declared rule.

**Acceptance scenarios**

1. **Given** three synced records, **When** one changes in the source, one in the graph and one on both sides, **Then** sync applies one, proposes one outbound and records one conflict.
2. **Given** a conflict under `system_of_record_wins`, **When** sync runs, **Then** the graph value stays and an outbound proposal carries it.

Independent test: the bound tests of `MEDULLA-SRC-AUTH-R012.9` run this story through the served method.

### Edge Cases

- An authority row under a `Pack` or `Ingestion` origin never takes effect.
- Two mappings on one class are both kept; no silent overwrite.
- A class with no authority row and one approved mapping is served with an `Implicit` premise; several approved mappings abstain `Ambiguous`.
- A stale copy falls back to a live read, or abstains `Stale` when no live route exists.
- A source name registered by another principal is unconnected for the caller and never named.
- A virtual row never resolves to a copy route.

## Requirements

- **FR-001**: A `vg:SourceAuthority` row states source, concept, optional facet, rank, scope, freshness, contract kind and materialisation policy. Rows: `MEDULLA-SRC-AUTH-R001`.
- **FR-002**: SHACL refuses a malformed or contradictory row. Rows: `MEDULLA-SRC-AUTH-R002`.
- **FR-003**: Only an operator-approved row has effect; a connector may propose, never assert. Rows: `MEDULLA-SRC-AUTH-R003`, `MEDULLA-SRC-AUTH-R801`.
- **FR-004**: `select_sources` reads effective rows and names the winner per class or facet with alternates. Rows: `MEDULLA-SRC-AUTH-R004`.
- **FR-005**: Tie-breaking is a total, documented order; an unresolvable tie abstains. Rows: `MEDULLA-SRC-AUTH-R005`.
- **FR-006**: An unconnected, unapproved, stale, mismatched or ambiguous authority yields a typed abstention or a declared fallback. Rows: `MEDULLA-SRC-AUTH-R006`.
- **FR-007**: Every answer carries "sources used and why" built from the premises. Rows: `MEDULLA-SRC-AUTH-R007`.
- **FR-008**: Rows copy data only under a declared, reasoned acceleration policy; the default is virtual. Rows: `MEDULLA-SRC-AUTH-R008`.
- **FR-009**: A source trust role orders sources before rank. Rows: `MEDULLA-SRC-AUTH-R010`.
- **FR-010**: Content that enters a run carries a stored trust mark that cannot be lowered without approval. Rows: `MEDULLA-SRC-AUTH-R011`.
- **FR-011**: A federated secondary source syncs two ways with the graph as system of record, under a declared conflict rule and without a second store. Rows: `MEDULLA-SRC-AUTH-R012`.

## Success Criteria

- **SC-001**: Changing only an authority row changes the sources used for an equal query text. Rows: `MEDULLA-SRC-AUTH-R009`.
- **SC-002**: A tenant never sees a verdict, a registered name or a mark of another tenant. Rows: `MEDULLA-SRC-AUTH-R004.6`, `MEDULLA-SRC-AUTH-R011.6`.

## Assumptions

- Arbora supplies a verified principal and tenant on every served request.
- The owner-scoped foreign catalog of `MEDULLA-FEDERATED-QUERY-R013` is the only registry of connected sources.
- The scope `governance:approve-source-authority` is generated by the identity registry of `MEDULLA-IDENTITY-R004`.

## Scope and interfaces

Owned here: the authority vocabulary and shapes, the approval path for authority, the authority-aware extension of `medulla_virtual_graph::source_selection`, the abstention and audit types, the trust mark. Not owned: the federated planner and pushdown (`MEDULLA-FEDERATED-QUERY-R046`, `R069`, `R073`), virtual-graph execution (`MEDULLA-UNIFIED-DATA-PLANE-R005`, `R006`), acceleration (`R038`), pack import (`MEDULLA-TYPED-PACKS`), decision-engine routing (`MEDULLA-DECISION-ENGINE`). Non-goals: ingesting source data as a default; a second catalog of connectors.

This spec is the single owner of the authority vocabulary (`vg:SourceAuthority`, `vg:authoritativeSource`, `vg:authorityFor`, `vg:authorityRank`, `vg:allowFallback`); the competency specs cite it and define no term of their own.

## Traceability

| Requirement | Design section | Test ID | Evidence |
|---|---|---|---|
| FR-001 | plan.md, Vocabulary | T-001 | PENDING |
| FR-002 | plan.md, Vocabulary (SHACL) | T-002 | PENDING |
| FR-003 | plan.md, Decisions D3 | T-003 | PENDING |
| FR-004 | plan.md, Wiring | T-004 | PENDING |
| FR-005 | plan.md, Decisions D4 | T-005 | PENDING |
| FR-006 | plan.md, Decisions D5 | T-006 | PENDING |
| FR-007 | plan.md, Audit trail | T-007 | PENDING |
| FR-008 | plan.md, Vocabulary | T-008 | PENDING |
| FR-009 | plan.md, Source trust role | T-010 | PENDING |
| FR-010 | plan.md, Untrusted taint mark | T-011 | PENDING |
| FR-011 | plan.md, Two-way sync with a federated secondary (R012) | T-012 | PENDING |
| SC-001 | plan.md, Worked example | T-009 | PENDING |
