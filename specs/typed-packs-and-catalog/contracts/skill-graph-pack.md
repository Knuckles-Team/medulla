# Seam: skill graph pack

Provider: scion-sdk SCION-CONNECTOR-CONTROL-R071.1
Consumer: medulla MEDULLA-TYPED-PACKS-R700.8

## Wire shape

The pack holds, per graph: one `skill` entry (`skill://<graph>/SKILL.md`, front matter `skill_type: graph`), one `skill_file` entry per `index.json`, `sources.json` and `reference/<hash>.md` (named `<graph>/<path>`, URI `skill-file://<graph>/<path>`, referencing the skill), and optionally one `ontology` entry named `skillgraph/<graph>`. The graph format is in the skill-graphs data model (`SG-CONTENT-R002`, `SG-CONTENT-R003`).

## Refusal codes

`SKILL_GRAPH_HASH_MISMATCH`, `SKILL_GRAPH_INDEX_MISSING`, `SKILL_GRAPH_ENTRY_MISSING` (`R700.4`), `SKILL_GRAPH_FILE_LIMIT_EXCEEDED`, `SKILL_FILE_NOT_GRAPH` (`R700.8`), `SKILL_GRAPH_LAYER_IRI_MISMATCH`, `SKILL_GRAPH_LAYER_WITHOUT_GRAPH` (`R701.4`), `ONTOLOGY_ORPHAN`, `ONTOLOGY_PACK_IMPORTS_CORE` (`R105`).

## Authoritative types

`PackEntryKind` in `crates/medulla-catalog-types/src/connector_pack/index.rs`. `SkillGraphManifest` and `SkillGraphLayer` in `crates/medulla-catalog-types/src/connector_pack/skill_graph.rs` (new).

## Test vectors

The pack built by `tests/test_skill_graph_ontology.py` of skill-graphs (`SG-CONTENT-R003.4`) is the vector for `tests/packs/test_skill_graph_pack_served.py`.
