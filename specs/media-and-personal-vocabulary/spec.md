# MEDULLA-MEDIA-VOCAB — Media and personal-content vocabulary

Owner: medulla. Cross-repo IDs: MEDULLA-CQ-CONN (consumer, specs/connector-competency); SCION-SOURCE-INGEST (provider of bookmark and paper envelopes); ARBORA-OPS (served adapters); OVERSTORY-APPS (renderers); MERISTEM-HARNESS-CYCLE (summary receipts).
Public provenance: gap found by specs/connector-competency, which marked the vocabulary of fifteen media and personal-content connectors MISSING.

## Delivery mapping

Component: ontology and packs, with the ingest and query entries named by each row.

Depends-on: `MEDULLA-DECISION-ENGINE-R010.100` for the refusal tables, `MEDULLA-DECISION-ENGINE-R651.1` for the memory record, `SCION-SOURCE-INGEST-R005.8` for bookmark envelopes. The connector operations that the competency questions need are specified in the connector repositories; until one lands, Medulla answers the affected question with a typed abstention naming the operation and its owner.

## User Scenarios & Testing

### User Story 1 - Shared media vocabulary and identity keys (Priority: P1)

An operator asks questions that span arr, qbittorrent, jellyfin, audiobookshelf, rom-manager, owncast, media-downloader and audio-transcriber. The engine needs one media vocabulary, join keys and shapes so that independent connectors meet on one entity.

**Acceptance Scenarios**:

1. **Given** the media vocabulary is loaded, **When** a film and a television series carry the same numeric provider id, **Then** they stay distinct records because identity keys are class-scoped.
2. **Given** a download transfer, **When** its info hash is not lower-case hex of 40 or 64 characters, **Then** the shape rejects it.

### User Story 2 - Content-channel vocabulary (Priority: P1)

An operator asks questions that span feeds, search, social posting, mailing and chat. The engine needs one content vocabulary that reuses the core document, team and person terms.

**Acceptance Scenarios**:

1. **Given** two search queries return the same URL, **When** both results are stored, **Then** each keeps its own identity and provenance.
2. **Given** a subscriber record, **When** it carries a raw address property, **Then** the shape refuses it and only the keyed hash is stored.

### User Story 3 - Per-connector virtual-graph mappings (Priority: P1)

An operator asks a single-source question of one connector. Each of the fifteen connectors has a mapping table and a recorded response that proves the question can be answered or abstained.

**Acceptance Scenarios**:

1. **Given** the recorded response of a connector, **When** its mapping rows are applied, **Then** every record becomes one node that uses only declared vocabulary.
2. **Given** a question whose data the connector cannot supply, **When** the single-source test runs, **Then** the answer is a typed abstention naming the missing operation and its owner.

### User Story 4 - Cross-source answers and connector operations (Priority: P2)

An operator asks a question that crosses connectors. Two worked examples join three sources each, and the connector repositories own the operations that are still missing.

**Acceptance Scenarios**:

1. **Given** requested, downloaded and library records from three sources, **When** the film question runs, **Then** the answer lists films downloaded but not in the library with provenance per leg.
2. **Given** a connector that lacks an operation, **When** its owner adds it, **Then** the Medulla test flips from an abstention to a positive answer.

### User Story 5 - Meeting transcripts and bookmarks (Priority: P2)

A meeting host corrects and finalizes a transcript and asks for a cited summary. A reader searches bookmarks and walks their relations. Medulla stores immutable revisions, issues bounded context and filters by authorization before counting.

**Acceptance Scenarios**:

1. **Given** a finalized transcript, **When** a summary is committed with a matching harness receipt, **Then** the summary stores its citations and a later correction marks it stale without rewriting it.
2. **Given** an allowed and a denied principal, **When** both search the same bookmark index, **Then** the denied principal sees no hit, edge or count that the allowed one sees.

### User Story 6 - Media tags and receipt-bound papers (Priority: P3)

An operator searches media by tag and reads paper facts. Every tag and every paper fact keeps its source provenance, and a record without provenance is refused.

**Acceptance Scenarios**:

1. **Given** a tag with a producer id and an evidence reference, **When** a caller who may see the item searches, **Then** the hit carries the tag id, the media revision and the authorization revision.
2. **Given** a mixed envelope of one identified and one unidentified paper, **When** it is ingested, **Then** only the unidentified paper is refused.

### User Story 7 - Media memory and added terms (Priority: P3)

A user sends an image or a voice note and later recalls it by its caption or transcript. The vocabulary also gains the engagement, stream-link, invoice-total and nutrition terms that the competency cases need.

**Acceptance Scenarios**:

1. **Given** an image stored once with a caption, **When** the user recalls by caption text, **Then** the memory record returns the image by digest.
2. **Given** a declared MIME type that differs from the sniffed type, **When** the attachment is remembered, **Then** it is refused and the store is unchanged.

### Edge Cases

- A connector that is not connected: the answer is a typed abstention, never an empty success.
- A tag, paper or summary without provenance: refused before any write.
- A stale index, a stale revision or an over-ceiling limit: refused with its stable code, never a truncated success.
- Two connectors declaring the same class IRI as the core: the collision test names exactly the two known aliases.
- Cross-tenant requests: refused before any write or count.

## Requirements

- **FR-001**: media-v1.ttl and content-v1.ttl define the classes, properties, identity keys and shapes of the plan, reusing existing core terms. Rows: `MEDULLA-MEDIA-VOCAB-R001`, `MEDULLA-MEDIA-VOCAB-R002`, `MEDULLA-MEDIA-VOCAB-R003`, `MEDULLA-MEDIA-VOCAB-R004`.
- **FR-002**: each of the fifteen connectors has a mapping table, a recorded response, a single-source test and a docs-truth check. Rows: `MEDULLA-MEDIA-VOCAB-R005`, `MEDULLA-MEDIA-VOCAB-R006`, `MEDULLA-MEDIA-VOCAB-R007`, `MEDULLA-MEDIA-VOCAB-R008`, `MEDULLA-MEDIA-VOCAB-R009`, `MEDULLA-MEDIA-VOCAB-R010`, `MEDULLA-MEDIA-VOCAB-R011`, `MEDULLA-MEDIA-VOCAB-R012`, `MEDULLA-MEDIA-VOCAB-R013`, `MEDULLA-MEDIA-VOCAB-R014`, `MEDULLA-MEDIA-VOCAB-R015`, `MEDULLA-MEDIA-VOCAB-R016`, `MEDULLA-MEDIA-VOCAB-R017`, `MEDULLA-MEDIA-VOCAB-R018`, `MEDULLA-MEDIA-VOCAB-R019`.
- **FR-003**: two cross-source examples run over the shared vocabulary. Rows: `MEDULLA-MEDIA-VOCAB-R020`.
- **FR-004**: where a connector cannot supply data, the question is answered by a typed abstention and a cross-repo row owned by that connector. Rows: `MEDULLA-MEDIA-VOCAB-R028`, `MEDULLA-MEDIA-VOCAB-R029`, `MEDULLA-MEDIA-VOCAB-R030`, `MEDULLA-MEDIA-VOCAB-R031`, `MEDULLA-MEDIA-VOCAB-R032`, `MEDULLA-MEDIA-VOCAB-R033`.
- **FR-005**: meeting transcript revisions, bounded summary context and receipt-bound summary commit. Rows: `MEDULLA-MEDIA-VOCAB-R001.101`.
- **FR-006**: bookmark semantic search and authorized relations on one pinned snapshot. Rows: `MEDULLA-MEDIA-VOCAB-R001.102`.
- **FR-007**: provenanced media tags with authorized search, and receipt-bound paper facts. Rows: `MEDULLA-MEDIA-VOCAB-R001.100`, `MEDULLA-MEDIA-VOCAB-R002.5`.
- **FR-008**: media memory over the blob substrate, the added vocabulary terms and the food-composition types. Rows: `MEDULLA-MEDIA-VOCAB-R650`, `MEDULLA-MEDIA-VOCAB-R651`, `MEDULLA-MEDIA-VOCAB-R652`, `MEDULLA-MEDIA-VOCAB-R653`, `MEDULLA-MEDIA-VOCAB-R654`, `MEDULLA-MEDIA-VOCAB-R655`.

## Success Criteria

- **SC-001**: no duplicate class IRI across the ontology files except the two documented connector aliases. Rows: `MEDULLA-MEDIA-VOCAB-R001.6`.
- **SC-002**: every mapping row of every connector names an existing class or property. Rows: `MEDULLA-MEDIA-VOCAB-R005.1`.
- **SC-003**: no served surface returns a denied tag, edge or count, and no refusal changes the store. Rows: `MEDULLA-MEDIA-VOCAB-R001.102.4`, `MEDULLA-MEDIA-VOCAB-R001.101.6`.

## Assumptions

- The `mv:` and `cv:` prefixes are documentation prefixes for sections of the shared `http://knuckles.team/kg#` namespace; every IRI stays in `:`.
- The vision producer of media tags is held by its owner; no row names it.
- Citation edges wait for the scion-sdk contract that serves cited-paper ids and target identity normalization.
- `content-v1.ttl` and the media and content shapes are loaded by tests today; registering them as core sources is an open owner item.

## Why a new spec

The search of `crates/medulla-core/ontology/` and of the existing specs found no owner for media, feed, search, social, mailing or chat vocabulary: `personal-v1.ttl` covers calendar, tasks and voice messages, `core-foundation-v1.ttl` the creative-work and document roots, and `documentation-v1.ttl` documentation pages only. The connector-competency spec consumes the vocabulary and the decision-engine and federated-query specs own routing, so none of them owns the vocabulary.

## Scope and interfaces

Owned: vocabulary files, shapes, mapping fixtures and their tests in Medulla, plus the ingest and query surfaces named by the rows. Connector operations belong to the connector packages. Routing and federation belong to the decision-engine and federated-query specs. Non-goals: bulk ingestion of media metadata (virtual-first), and the archive, paper, file and genealogy vocabularies of other specs.

## Traceability

| Label | Rows | Design section | Test ID | Evidence |
|---|---|---|---|---|
| FR-001 | `MEDULLA-MEDIA-VOCAB-R001` | plan.md Vocabulary | T-001 | PENDING |
| FR-002 | `MEDULLA-MEDIA-VOCAB-R005` | plan.md Connector mapping tables | T-002 | PENDING |
| FR-003 | `MEDULLA-MEDIA-VOCAB-R020` | plan.md Worked examples | T-003 | PENDING |
| FR-004 | `MEDULLA-MEDIA-VOCAB-R028` | plan.md Decisions | T-004 | PENDING |
| FR-005 | `MEDULLA-MEDIA-VOCAB-R001.101` | plan.md Production caller chain | T-005 | PENDING |
| FR-006 | `MEDULLA-MEDIA-VOCAB-R001.102` | plan.md Production caller chain | T-006 | PENDING |
| FR-007 | `MEDULLA-MEDIA-VOCAB-R001.100` | plan.md Production caller chain | T-007 | PENDING |
| FR-008 | `MEDULLA-MEDIA-VOCAB-R650` | plan.md Production caller chain | T-008 | PENDING |
