# Test and quality specification

## Evidence rule

Tests may run in an isolated ephemeral Medulla/Graft SDK environment using synthetic packs and local engine state. No private service, inventory or live tenant is a prerequisite. Record exact commit, command, fixture version, count, exit status and relevant artifact digest. A skipped or unavailable gate remains unverified. Every refusal asserts unchanged head, members, holders, records, body liveness and outbox, not only the error string.

| Test ID | Requirements | Fixture/action | Expected result and evidence |
|---|---|---|---|
| TP-01 | PACK-01, 05, 10 | Replay `contract/fixtures/connector_pack_digest_vectors.json` in Rust and Graft SDK; permute archive offsets/order and package version; change one body byte. | Semantic digest stable under repackaging, changes on content, server pins follow declared contract, and generated fixtures/receipt match committed bytes. |
| TP-02 | PACK-01, 06 | Plant one failing index/archive case for G1–G6: unsorted/duplicate URI, wrong scheme/kind, two servers, each size edge, missing/corrupt archive, overflow/overlap/gap, wrong digest, forbidden or duplicate component ID. | Named code; no write; mutation deletion of each validation guard makes its test fail. |
| TP-03 | PACK-06 | Plant G7–G12: BOM, invalid UTF-8, duplicate JSON key, too deep/nodes, NaN, missing tool input schema, YAML alias/tag/front-matter mismatch, malformed native IRI, foreign capability claim, invalid currency/cost/latency/model values. | Invalid bodies refused; foreign absolute capability IRI accepted as unresolved claim with warning; empty description accepted with fallback warning. |
| TP-04 | PACK-06, 08 | Plant G13–G21/K1–K7: `@base`, remote `owl:imports`, overlong IRI, inconsistent ontology, derivation budget exhaustion, `SERVICE SILENT`, per-file blank-node collision, conflicting shape owner, SHACL violation, bad/cyclic ref, invalid draft, unbound importer, >50% withdrawal, 300 violations and stale schema source. | Exact refusal/warning and bounded runtime; 256 maximum violations with exhaustion flag; no remote fetch or partial mutation. The planted G14 and malformed G15 cases must be red before the fix and green after. |
| TP-05 | PACK-03, 04 | Import same pack twice. Lose response and retry same operation identity. Retry with different bytes. Race two expected heads. Change only package version. Change server contract version. Remove and restore one tool. | No-op creates no revision. Replay returns same receipt. Changed bytes conflict. One racing head refuses. Contract change cascades pins, package-only change does not. Withdrawal/republication preserves ID. |
| TP-06 | PACK-03, 11 | Kill child after body store and before owner commit, during owner commit, after commit and before response. Cancel timed-out import and immediately Bind same tenant. | Orphan bodies are reclaimed after grace. Owner state is all-or-nothing. Receipt replays. Lock is released and next operation is not blocked. |
| TP-07 | PACK-02, 04, 11 | Authorized and unauthorized `Status`, `Content`, Bind/Unbind/Retire/Reproject, direct `mcp:` Publish, cross-tenant and graph reads, clustered writer. | Authz actions and stable codes match descriptors. No cross-tenant bytes/metadata. Reserved ID and LocalOnly refusals. Historic pin may resolve withdrawn revision but new selection cannot. |
| TP-08 | PACK-07, 08 | Crash projection between graph attach and visibility ack. Inject graph grant denial. Process a stale outbox event after a newer head. Retry a failed event. Attach a pack snapshot to another graph. | At most one revision is visible. A stale worker refuses. Denial becomes durable failed status. Later events are not collateral-dead-lettered. Static attachment reports staleness. |
| TP-09 | PACK-08 | Attach two compatible sources. Test conflicting shape owners, duplicate shared classes, source bound overflow and `IcvConfigure` compatibility. Attach→detach→attach the same bytes; compare `GraphSchemaClasses`. | Composed policy/digest and class list deterministic; only real conflict refuses; cache cannot suppress repeated attach; no retroactive data scan is implied. |
| TP-10 | PACK-09, 10 | Run generated-contract fixpoint and committed-artifact comparison. Replay method-body signing vectors in Python/Go/JS. Every OpKind/PredKind printer/parser round trip, 512 property cases and fuzz target. | Receipt and generated files match by content. Every public client signs and decodes same typed contract. No variant silently falls to Dynamic. |
| TP-11 | PACK-09, 12 | Synthetic connector from Graft SDK manifest/tool probe through pack import, visible projection, fleet catalog join and body read. Restart and restore. | Content and liveness have distinct authorities. No duplicate catalog row. Served result matches local typed result after restart. |
| TP-12 | PACK-12 | Full Rust nextest suite including integration binaries, root Python suite, Go and JS client suites, planted pack tests and release workflow replica at the exact head. | Zero failures, zero signal-killed hidden tests and no omitted root suite. If a test binary dies by signal, rerun that binary under process-isolated nextest and account for every test. |
| TP-13 | PACK-08, 10 | Migrate a representative ontology object, schema pack DTO and schema-drift candidate through Medulla typed/generated APIs; compare prior read/write behavior and provenance before deleting duplicate types. | One Medulla schema authority, no lost semantics, no caller-owned SHACL rendering and deterministic drift gate refusal/acceptance. |

## Quality and release gates

The repository pins CCCC 1.6.0, KISS 0.4.12, Dupehound 0.1.2 (`threshold 0.85`, at least 40 tokens), and jscpd 5.0.16 (at least 50 tokens and 5 lines, mild mode) in `pyproject.toml`. Use the repository's scanner source manifest and release workflow rather than arbitrary file subsets. `release.yml` runs scanner install and validation. `scripts/check_dupehound_census.py`, `scripts/validate_cccc_census.py`, `scripts/check_kiss_census.py`, and jscpd differential are the configured proofs. Keep generated contract artifacts in their explicit generated exclusions only; source duplicate pairs need resolution or reviewed explanation. Apply KISS by extending typed owner modules and a shared validator instead of a second registry, parser or cache. Run Rust fmt/Clippy, Python lint/type/test, `gen_contract --check`, contract generated-artifact test, architecture/pin-resolution checks and release gate replica where applicable. The current gate's exact command should be copied into the evidence, not replaced with an invented score.

## Acceptance matrix

PACK-01: TP-01/02; PACK-02: TP-07/10; PACK-03: TP-05/06; PACK-04: TP-05/07; PACK-05: TP-01/04; PACK-06: TP-02/03/04; PACK-07: TP-08; PACK-08: TP-04/09/13; PACK-09: TP-10/11; PACK-10: TP-01/10/13; PACK-11: TP-06/07/08; PACK-12: TP-11/12 and all quality gates. Qualification requires the mapped tests and applicable gates to pass against an exact merged head.

## Boundary and verification notes (MEDULLA-TYPED-PACKS-R020, R052, R060, R063, R082)

These five requirements are boundary or verify-against-existing-contract rows. They add no net-new pack behavior. Existing evidence covers each row; no new fixture is required.

| ID | Note |
|---|---|
| MEDULLA-TYPED-PACKS-R020 | The tenant-profile administrative capability defect is owned and tested by the dependency that defines tenant profiles (see the ruled-decision table in `tasks.md`); no pack-write code path in this repository depends on it, so this spec's own release gate (TP-01..TP-13, scanner census) is the full proof that no corresponding change was made here. |
| MEDULLA-TYPED-PACKS-R052 | Head-state projection for the first Agent Library outbox consumer waits on the general outbox reject/re-drive primitive (PACK-07, P7); per this row's own text, no pack-specific stand-in is shipped while that primitive is pending, so there is no placeholder code for this gate to find in this repository today. |
| MEDULLA-TYPED-PACKS-R060 | Manifest-read refusal for an incompatible owner layout precedes any import path relying on it; this ordering is exercised by the same incompatible-layout refusal coverage TP-06 already asserts for owner-transaction recovery (lock release, no partial commit), so no import path may be added here ahead of that refusal. |
| MEDULLA-TYPED-PACKS-R063 | Before any shared schema version bump, the affected agent-layer row types must be enumerated explicitly; TP-10's generated-contract fixpoint and committed-artifact comparison is the existing check this enumeration must pass, so a bump without an explicit per-type list fails TP-10 rather than passing silently. |
| MEDULLA-TYPED-PACKS-R082 | TP-13 (migrate a representative ontology object, schema pack DTO and schema-drift candidate through Medulla typed/generated APIs, compare prior read/write behavior) is this row's Medulla-side proof; the hand-written-DTO modules it names (`models/knowledge_graph`, `models/schema_definition`, `models/evidence_bundle`, `models/knowledge_pack`, `models/codemap`, `models/graph`, `models/knowledge_base`, `schema_pack`, `schema_packs`) are owned by `Meristem`, so their migration and the `MERISTEM-SEMANTIC-R005` `kg/domain_packs` follow-on are cross-repo children delivered against that repository, consuming Medulla's generated types as the destination-copies-first side of the move. |

## Cross-cutting import-safety defect regression mapping (MEDULLA-TYPED-PACKS-R021)

Every cross-cutting pack-import-safety defect is a named rule (G1-G21) in `medulla/testing/synthetic/packs/catalogue.py`'s `MUTATIONS` (TP-02/03/04 above), and each rule's every variant below has its own regression test case: one parametrized run of `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule`, one per `MUTATIONS` entry, with the test id computed by that module's own `_case_id()` (a mutation function's `__name__`, or a case object's `f"{rule}_{variant}"` -- the two mutation sources in `MUTATIONS` use each form; `_case_id`'s docstring names both, and `pytest --collect-only tests/test_connector_pack_planted.py` lists the exact ids). The one documented exception is the tenant-profile administrative capability defect (MEDULLA-TYPED-PACKS-R020): it is fixed in, and tested by, the dependency that defines tenant profiles, not by this gate.

| Rule | Variants, each with its own regression case in `MUTATIONS` |
|---|---|
| G1 | connector_with_slash, invalid_connector, name_over_256_bytes, schema_version, scheme_mismatch, skill_file_traversal, two_servers, unknown_kind, unsorted_uris |
| G2 | archive_over_16mib, body_over_2mib, entries_over_8192, index_over_16mib, iri_list_over_64, references_over_64, schema_over_1mib |
| G3 | archive_length, archive_not_uploaded, archive_sha256 |
| G4 | offset_overflow, overlapping_sections, uncovered_bytes |
| G5 | flipped_body_byte, forged_pack_digest |
| G6 | decide_owned_kind, duplicate_component_id |
| G7 | invalid_utf8, utf8_bom |
| G8 | array_input_schema, duplicate_keys, json_depth_65, nan_literal |
| G9 | front_matter_over_16kib, missing_front_matter, name_mismatch, unknown_skill_type, yaml_alias |
| G10 | missing_input_schema |
| G11 | empty_description |
| G12 | cost_over_bound, currency_not_iso_4217, foreign_namespace_iri, malformed_iri, max_output_over_context, mistyped_native_iri, modality_in_provides, p50_over_p95, tool_mode_on_prompt |
| G13 | base_directive, iri_over_4kib, relative_iri, remote_import, shapes_base_directive, unparseable |
| G14 | derivation_budget, unsatisfiable_class |
| G15 | duplicate_shape_iri, malformed_path, scoped_blank_node_labels, service_silent |
| G16 | instance_without_label |
| G17 | missing_reference, reference_cycle, reference_kind_mismatch |
| G18 | control_character_summary |
| G19 | unconfigured_importer |
| G20 | empty_pack, withdraw_sixty_percent |
| G21 | three_hundred_violations |

Every row's regression test must pass against an exact merged head, as PACK-06/08 requires.

| Repair | Requirement | Regression | Acceptance |
| --- | --- | --- | --- |
| TP-PARSE-IDENTITY | MEDULLA-TYPED-PACKS-R093.1 | Native public and bounded Turtle readers parse the same explicit and anonymous blank nodes independently. They then union the results. | Six subjects stay distinct across three parses. Same-document references remain equal. Composition retains explicit source labels and existing bounded refusal tests pass. |
| TP-REPROJECT-IDENTITY | MEDULLA-TYPED-PACKS-R093.2 | Served reproject combines independent connector-pack members with reused blank labels. | Isolated member identities and repeatable composed digests; remains open until the served regression executes. |

## Capability annotation evidence

| Requirement | Native regression | Result |
| --- | --- | --- |
| MEDULLA-TYPED-PACKS-R042.1 | `crates/medulla-h-admin/src/admin/connector_pack/import/capability_tests.rs::both_capability_fields_refuse_relative_and_invalid_scheme_claims` and `foreign_terms_warn_and_stay_only_in_the_declared_partition`: both typed fields accept native terms and retain exact foreign claims with warnings. Native typos, roots, wrong kinds, invalid schemes, and malformed typed lists refuse. | Eight production-module tests passed on 2026-10-11 with Rust 1.96.0 and cached `medulla_types` and `serde_json` dependencies. Restoring the old scheme check makes the invalid-scheme test fail at `provides: 1vendor:search`. |
| MEDULLA-TYPED-PACKS-R042.2 | `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::a_rejected_import_mutates_nothing_in_the_catalog`. `crates/medulla-h-admin/src/admin/connector_pack/tests/content_model.rs::capability_annotations_remain_declared_after_import`. | Both signed, served native regressions pass. Exact source, commands and results are in `../decision-engine/evidence/gr1080-native-20261011/final-native-results.json`. This focused slice does not verify the parent. |

The native harness loads the unchanged production module path with `#[path]`; it uses no behavior substitutes. To repeat from the repository root with compatible cached Rust dependencies:

```sh
work=$(mktemp -d)
printf '#[path = "%s/crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs"]\nmod annotations;\n' "$PWD" > "$work/native.rs"
rustc --test --edition=2021 "$work/native.rs" \
  -L "dependency=$CACHED_RUST_DEPS" \
  --extern "medulla_types=$(find "$CACHED_RUST_DEPS" -name 'libeg_types-*.rlib' -print -quit)" \
  --extern "serde_json=$(find "$CACHED_RUST_DEPS" -name 'libserde_json-*.rlib' -print -quit)" \
  -o "$work/native-tests"
"$work/native-tests" --nocapture
```

The full served test suite and hosted CI were not run for this native slice.

R042.2 uses signed imports for four G12 refusals, two per capability field.
The admin fixture compares head, members, component revisions, import records, holders, body liveness, and outbox rows.
The fixture checks blob counts after archive upload to detect rejected body admission.
The positive fixture compares both native and declared fields and exact foreign warning receipts.
R093.2 extends `an_admin_forced_reproject_redrives_a_stale_projection_without_losing_later_work`.
The fixture reads authoritative persisted schema after signed Reproject and checks two distinct blank identities.
A fresh request key repeats Reproject and preserves the composed digest, source catalogue, and connector head.
Scoped formatting and diff checks do not execute these native tests.
The R042.2 fixtures and R093.2 forced-reprojection fixture pass native execution;
their exact evidence is in the shared fixture-repair receipt linked above.
Test bindings record the existing acceptance fixtures, independently of execution results.

## Modality regression scope (MEDULLA-TYPED-PACKS-R043)

The following tests cover existing native validation and digest behavior. They do not establish complete PACK-09 acceptance.

| Source and proving test | Required observation |
|---|---|
| `crates/medulla-h-admin/src/admin/connector_pack/import/modality_tests.rs::modalities_accept_native_terms_for_tools_prompts_and_skills` | Both fields accept native terms for all three entry kinds and round-trip without loss. |
| `crates/medulla-h-admin/src/admin/connector_pack/import/modality_tests.rs::modalities_refuse_foreign_roots_typos_and_wrong_kinds` | Both fields refuse foreign, root, typo, wrong-kind, relative, and empty IRIs with the typed code. |
| `crates/medulla-h-admin/src/admin/connector_pack/import/modality_tests.rs::modality_lists_enforce_wire_types_and_both_size_edges` | Zero and 64 elements pass; 65 elements, null, scalars, and non-string elements refuse. |
| `crates/medulla-h-admin/src/admin/connector_pack/import/modality_tests.rs::modality_digest_replay_is_canonical_and_direction_sensitive` | Rust replays committed full/empty vectors. Reordering and duplication preserve identity; content and direction changes do not. |
| `tests/test_connector_pack_client.py::test_modality_annotations_replay_golden_vector_and_affect_identity` | Python replays the full vector. Both fields change tool, prompt, and skill pack identity. |

TP-10, TP-11, and applicable quality gates remain required on the merged exact head.
These partial regressions intentionally carry no acceptance binding that would mark R043 VERIFIED.

## Harness configuration evidence

- `MEDULLA-TYPED-PACKS-R048.1`: `crates/medulla-agent-types/src/agent_component/facts.rs::tests::context_window_zero_is_refused`.
- `MEDULLA-TYPED-PACKS-R048.2`: `crates/medulla-agent-types/src/agent_component/facts.rs::tests::measured_fact_requires_run_ref`.
- `MEDULLA-TYPED-PACKS-R048.3`: `crates/medulla-agent-types/src/agent_component.rs::tests::absent_shell_proficiency_keeps_digest`.
- `MEDULLA-TYPED-PACKS-R048.4`: `crates/medulla-agent-types/src/agent_component.rs::tests::measured_model_facts_change_digest`.
- `MEDULLA-TYPED-PACKS-R048.5`: `tests/typed_packs/test_model_harness_facts.py::test_model_profile_round_trip`.
- `MEDULLA-TYPED-PACKS-R066.1`: `crates/medulla-core/tests/harness_shapes.rs::model_fact_shapes_reject_missing_run`.
- `MEDULLA-TYPED-PACKS-R066.2`: `crates/medulla-shacl/tests/swarm_topology_shapes.rs::shell_centric_must_be_boolean`.
- `MEDULLA-TYPED-PACKS-R066.3`: `crates/medulla-core/tests/swarm_topology_shapes.rs::shell_centric_inherits_or_is_absent`.
- `MEDULLA-TYPED-PACKS-R045.1`: `crates/medulla-catalog-types/src/connector_pack/annotations.rs::tests::new_fields_roundtrip`.
- `MEDULLA-TYPED-PACKS-R045.2`: `crates/medulla-catalog-types/src/connector_pack/annotations.rs::tests::absent_facts_take_safe_defaults`.
- `MEDULLA-TYPED-PACKS-R045.3`: `crates/medulla-catalog-types/src/connector_pack/digest.rs::tests::absent_new_fields_keep_golden_vectors`.
- `MEDULLA-TYPED-PACKS-R045.4`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::tests::unknown_side_effect_is_refused`.
- `MEDULLA-TYPED-PACKS-R045.5`: `crates/medulla-h-admin/src/admin/connector_pack/import/facts.rs::tests::mcp_hint_read_only_is_unproven`.
- `MEDULLA-TYPED-PACKS-R045.6`: `crates/medulla-h-decide/src/decide/stat_tests/tool_subset_tests.rs::missing_schema_tokens_abstains`.
- `MEDULLA-TYPED-PACKS-R045.7`: `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::mcp_hint_read_only_premise_is_unproven`.
- `MEDULLA-TYPED-PACKS-R045.8`: `crates/medulla-capabilities/src/contract/python/surfaces/tests.rs::pack_annotations_expose_harness_tool_facts`.
- `MEDULLA-TYPED-PACKS-R045.9`: `tests/harness_config_tool_facts_served.rs::two_tool_pack_subset_respects_budget`.

## Measured model producer tests (R048.3, R048.4)

`absent_shell_proficiency_keeps_digest` checks fixed v3 and v4 component digest
vectors and decodes omitted and explicit-null fields. `measured_model_facts_change_digest`
checks each measured value, evaluation ID and evaluation digest independently.
`measured_model_facts_refuse_malformed_references` checks decode and publication
refusal. `measured_model_facts_project_ontology_names` calls the existing
`component_atoms` projector and checks all enum values and absent fields.

These are producer unit tests. R048.5 still requires signed pack import and served
readback evidence. The generated lifecycle records test bindings, not execution
results. Native types/RDF test execution and contract generation are required
before integration of the producer fields.

Skill package rendering: a changed referenced node changes the digest on the next listing. A dangling term is refused and the row stays listed.

## R071.1 and R097.2 crosswalk acceptance

All cases below are required future evidence.

| Scenario | Requirements | Required evidence |
|---|---|---|
| Typed records and domain kinds | R071.1.1, R097.2.1 | Round-trip revisions, origins, relations and coverage. Reject malformed IRIs, unknown kinds and foreign coverage entries. |
| Deterministic generation | R071.1.2, R097.2.2 | Regenerate RDF twice. Compare declared mappings and exact unmapped sets against independently enumerated source classes. |
| Real production path | R071.1.3, R097.2.3 | Signed import reaches production validation and schema attachment. Generated client inspection reads the committed revision and digest. |
| Refusal before activation | R071.1.4, R097.2.4 | Reject unknown and out-of-scope sources, conflicts, category mismatch, stale revision, forged evidence and every bound. Read pack status and active schema independently afterward. |
| Evidence ownership | R071.1.5, R097.2.5 | Correlate tenant, graph, pack, vocabulary and schema digests. Inline reports cannot claim committed activation. |
| Core metadata served proof | R071.1.6 | Real server returns the exact golden source/target/relation tuples below, not merely three results. A rejected Person-to-Event mapping preserves the prior schema. |
| Domain served proof | R097.2.6 | Import medical, technology and labor fixtures with mapped and unmapped classes. Query exact coverage, restart, and read it again. |

Proving integration owner: `tests/test_ontology_crosswalk_served.py`. Use generated client operations and independent schema/pack readback. Mocked routes, inline-only inspection and direct helper calls cannot close these rows.

### Golden tuples and source-resolution refusals

For R071.1.2 and R071.1.6, compare these literal tuples after served import and inspection. Preserve each IRI's scheme. A count-only assertion is insufficient.

| Source class IRI | Target class IRI | Declared relation |
|---|---|---|
| `http://knuckles.team/kg#Person` | `http://schema.org/Person` | `SeeAlso` |
| `http://knuckles.team/kg#Event` | `http://schema.org/Event` | `SeeAlso` |
| `http://knuckles.team/kg#Document` | `http://schema.org/DigitalDocument` | `SeeAlso` |

The fixture pins source revision `core-crosswalk-fixture-v1` and target revision `schemaorg-crosswalk-fixture-v1`, with their document digests. These are fixture labels, not claimed upstream releases. The report must return both labels and digests unchanged. Medulla Document includes digital and physical documents. Its reference does not license `SkosExactMatch`, `owl:sameAs` or a new equivalence axiom. Existing core axioms remain unchanged.

For directional acceptance, the fixture declares Document and DigitalDocument in both source and target vocabularies. DigitalDocument is narrower than Document in each approved fixture. Pin both revisions and assert these exact tuples:

| Source class IRI | Target class IRI | Declared relation |
|---|---|---|
| `urn:eg:crosswalk-fixture:source:DigitalDocument` | `urn:eg:crosswalk-fixture:target:Document` | `SkosBroadMatch` |
| `urn:eg:crosswalk-fixture:source:Document` | `urn:eg:crosswalk-fixture:target:DigitalDocument` | `SkosNarrowMatch` |

These asymmetric fixtures must detect swapped endpoints or relations. The broadMatch target is broader than its source; the narrowMatch target is narrower. Compare emitted predicates and served records separately. A submitted reversed relation must return `CROSSWALK_CONFLICT` against the pinned fixture declaration.

For R071.1.4 and R097.2.4, use valid targets in every source-refusal case:

| Source fixture | Exact refusal | Independent readback |
|---|---|---|
| Undeclared class, or a property IRI used as a class | `CROSSWALK_SOURCE_UNKNOWN` | Pack head, receipts and schema digest remain unchanged. |
| Class declared only by a later source revision | `CROSSWALK_SOURCE_UNKNOWN` | The pinned earlier revision cannot resolve it. |
| Known class in an unselected pack or unauthorized source scope | `CROSSWALK_SOURCE_OUT_OF_SCOPE` | No crosswalk or unmapped entry is published for that class. |

Each refusal identifies the source IRI, requested source revision and selected scope. Exercise these through real import and schema attachment. Query pack status, GraphSchemaClasses and OntologyInspect afterward to prove unchanged committed state. Scope refusal must not disclose foreign class metadata.


## Prompt and residual acceptance matrix

Every test below is proposed. This definition adds no runtime test binding or execution receipt.
Retain all original PACK and TP acceptance above, including every original R001/R002/R094 obligation.

| ID | Positive proof | Negative or boundary proof | Proposed proving test |
|---|---|---|---|
| MEDULLA-TYPED-PACKS-R001.1 | Round-trip every original index and entry field. | Unknown kinds and malformed index fields refuse. | `tests/test_connector_pack_contract.py::test_pack_index_entry_contract` |
| MEDULLA-TYPED-PACKS-R001.2 | Round-trip every section and annotation field without changing existing semantics. | Invalid section bounds and malformed typed annotations refuse. | `tests/test_connector_pack_contract.py::test_pack_section_annotation_contract` |
| MEDULLA-TYPED-PACKS-R001.3 | Preserve ConnectorPackOp and AgentComponent.Content operation and result shapes. | Missing operations and incompatible result shapes fail qualification. | `tests/test_connector_pack_contract.py::test_pack_content_operation_contract` |
| MEDULLA-TYPED-PACKS-R001.4 | Every published connector_pack_digest_vectors.json sample matches byte-for-byte. | Changing a framed input or expected digest fails. | `tests/test_connector_pack_contract.py::test_original_framed_digest_vectors` |
| MEDULLA-TYPED-PACKS-R001.5 | The complete generated-client inventory shares the original wire shapes and golden bytes. | A stale client, omitted client or changed field fails. | `tests/test_connector_pack_contract.py::test_all_clients_share_pack_contract` |
| MEDULLA-TYPED-PACKS-R001.6 | A typed object round-trips all identity and revision fields. | Changing a schema or body pin cannot preserve the old definition identity. | `crates/medulla-agent-types/src/agent_component.rs::tests::prompt_blueprint_identity` |
| MEDULLA-TYPED-PACKS-R001.7 | Nested valid objects retain every admitted field and extension value. | Missing task, wrong field types, nonfinite numbers and oversized objects refuse. | `crates/medulla-agent-types/src/agent_component/facts.rs::tests::prompt_blueprint_schema_refusals` |
| MEDULLA-TYPED-PACKS-R001.8 | Repeated identical inputs produce the same bytes and render digest. | A changed variable changes the result; invalid input produces no render result. | `crates/medulla-compute/src/assemble/prompt.rs::tests::prompt_render_vectors` |
| MEDULLA-TYPED-PACKS-R001.9 | Generated clients encode and decode every declared request, result and typed refusal. | Missing render provenance or an untyped fallback fails. | `tests/test_prompt_blueprint_contract.py::test_generated_prompt_contract` |
| MEDULLA-TYPED-PACKS-R002.1 | Staged bytes use their verified SHA-256 address. | A digest mismatch refuses; staging alone exposes no component. | `tests/test_connector_pack_storage.py::test_body_staging` |
| MEDULLA-TYPED-PACKS-R002.2 | An unchanged import keeps exact holder/refcount values. | Repeated identical import cannot increment or decrement a holder. | `tests/test_connector_pack_storage.py::test_unchanged_holder` |
| MEDULLA-TYPED-PACKS-R002.3 | A revised component binds its new body and preserves historical revision holders. | A changed revision cannot orphan a body held by history. | `tests/test_connector_pack_storage.py::test_revised_holder` |
| MEDULLA-TYPED-PACKS-R002.4 | A withdrawn component retains body access through a valid historical pin. | Withdrawal cannot reclaim a held historical body. | `tests/test_connector_pack_storage.py::test_withdrawn_holder` |
| MEDULLA-TYPED-PACKS-R002.5 | A fault before owner commit leaves an orphan reclaimed only after its grace window. | An unexpired orphan or held body survives reconciliation and restart. | `tests/test_connector_pack_storage.py::test_orphan_grace_fault` |
| MEDULLA-TYPED-PACKS-R002.6 | AgentComponent.Content returns the exact body of its pinned component revision. | Corrupt bytes and denied tenants refuse without leaking body data. | `tests/test_connector_pack_storage.py::test_verified_body_read` |
| MEDULLA-TYPED-PACKS-R002.7 | A successful write has one revision and live holder with its exact body digest. | A fault before owner commit leaves only an unreferenced body subject to the existing grace rule. | `tests/test_prompt_blueprints_served.py::test_prompt_owner_commit` |
| MEDULLA-TYPED-PACKS-R002.8 | Independent graph queries read the exact prompt revision and dependency links. | Replaying an old event cannot regress the visible revision. | `tests/test_prompt_blueprints_served.py::test_prompt_graph_projection` |
| MEDULLA-TYPED-PACKS-R002.9 | A real generated-client request reaches the renderer and returns the pinned revision and expected bytes. | Invalid requests leave graph, body holders, head and render-result publication unchanged. | `tests/test_prompt_blueprints_served.py::test_prompt_render_route` |
| MEDULLA-TYPED-PACKS-R002.10 | A valid served journey renders the pinned version and causes one observed model request. | Each invalid case yields a typed refusal and zero requests at the real model transport boundary. | `tests/test_prompt_blueprints_served.py::test_invalid_prompt_prevents_dispatch` |
| MEDULLA-TYPED-PACKS-R002.11 | Current and historical revisions, graph links and verified body bytes survive restart. | A refused stale update leaves the previous durable head intact. | `tests/test_prompt_blueprints_served.py::test_prompt_restart_readback` |
| MEDULLA-TYPED-PACKS-R094.1 | A generated Python client reads exact verified pack body bytes. | A refusal-only dispatch arm fails this acceptance. | `tests/test_connector_pack_content.py::test_stable_content_returns_body` |
| MEDULLA-TYPED-PACKS-R094.2 | Export preserves nested extension values, nulls, arrays and exact numeric values. | An omitted field, changed revision or lost numeric precision fails. | `tests/test_prompt_blueprints_served.py::test_lossless_blueprint_export` |
| MEDULLA-TYPED-PACKS-R094.3 | Changing one field creates one new revision and preserves every other field. | Concurrent writers cannot overwrite a newer head; reserved pack identities require pack admission. | `tests/test_prompt_blueprints_served.py::test_modified_blueprint_import` |
| MEDULLA-TYPED-PACKS-R094.4 | Replay produces no extra revision, holder or outbox event. | A conflicting retry preserves the prior committed result. | `tests/test_prompt_blueprints_served.py::test_blueprint_retry_identity` |
| MEDULLA-TYPED-PACKS-R094.5 | The new revision contains the one intended change and all unchanged values. | Stale import and denied-tenant cases preserve both historical and current readback. | `tests/test_prompt_blueprints_served.py::test_blueprint_roundtrip_restart` |
| MEDULLA-TYPED-PACKS-R094.6 | Authorized graph adoption uses the normal publication boundary and records the source revision. | A proposal alone causes no active-head change, model dispatch or external write. | `tests/test_prompt_blueprints_served.py::test_prompt_proposal_has_no_authority` |
| MEDULLA-TYPED-PACKS-R094.7 | A real authorized producer journey writes only the approved source and returns a bound receipt. | Absent producers leave this acceptance unfulfilled; graph persistence cannot substitute. | `tests/test_prompt_blueprints_served.py::test_approved_source_effect_dependency` |

### Deterministic fixtures and real served journey

Use synthetic, versioned prompt fixtures with a pinned manifest digest and independently authored expected values.
Include nested custom fields, arrays, Unicode, null and absent fields, empty collections and the exact integer 9007199254740993.
Include an exact decimal 0.10000000000000000001; no binary64 conversion may change it.
The fixture schema explicitly admits both numbers. A narrower schema must refuse rather than round them.
The render fixture requires task="summarize", topic="alpha" and a declared audience variable with value "engineers".
Its canonical rendered bytes must be exactly the following UTF-8 sequence, without a trailing newline:

```json
{"audience":"engineers","task":"summarize","topic":"alpha"}
```

Repeat rendering with object keys reordered. The bytes and digest must remain equal.
Change audience to "reviewers"; only that value and the render digest change.
Test exact and exceeded body, schema, depth, node and output bounds from the admitted profile.

Start a real Medulla server with disposable durable stores and the required declared features.
Use generated public clients with authenticated identities; do not call private store helpers as the served proof.
Publish a schema and prompt, wait for observable graph readiness, then read them through a second client.
Render the exact prompt revision and observe the named production handler and renderer.
Export that revision, change one nested field, import with its expected base, and read the new revision.
Restart the server and compare graph identity, historical revisions, complete payload, schema pins and body digests.
Retry after a lost reply and assert one committed revision and receipt.

Exercise malformed schemas, invalid payloads, unsupported revisions, missing variables and substituted digest pins separately.
Race two updates from the same base; one succeeds and the stale update refuses.
Use a different actor and tenant for every public read and write boundary.
Assert unchanged head, graph visibility, holders and committed outbox on each rejected mutation.
Uncommitted staged bodies follow the existing orphan grace rule; they must never become readable prompt revisions.

The dispatch proof traverses the actual Meristem consumer and its model transport against a disposable recording endpoint.
A valid control request produces one observed model call with the pinned rendered bytes.
Every invalid schema, body, revision and scope case produces zero calls and a typed refusal.
No helper-only counter, mocked dispatcher or missing consumer can satisfy R002.10.
Record exact Medulla and consumer heads, generated schema revisions, request IDs, production call sites and readback digests.
Publish sanitized assertions; exclude credentials, private prompt content and raw request logs.

R094.7 also requires the actual approved Graft SDK/Arbora source-effect producer journey.
Use a disposable repository and prove exact-path/base-revision approval, conflict refusal and receipt-bound readback.
Use [Graft SDK R004.5 through R004.12](https://github.com/Knuckles-Team/agent-connector-sdk/tree/0c6c53dd63fe2a8f9b2ac3568b6d0e8f2c602734/specs/SCION-GOVERNED-WRITEBACK) and [Arbora R003.4 through R003.10](https://github.com/Knuckles-Team/graph-os/tree/86737b45aaceb2eebe29b3a98ada5a55f8b71955/specs/fleet-catalog-and-tools).
Their merged definitions identify the exact contracts; this amendment has not run the source-effect journey.

Pass the exact exported bytes through RepositorySourceEffectRequest inside Arbora RepositoryWriteBackParams.
Serve `fleet.write_back.preview`, then prepare and confirm `fleet.write_back.commit` with the identical full request and trusted binding.
Keep the original Graft SDK effect key, independent recorded approval and commit-operation plan association.
Observe the actual Graft SDK conditional write, separate provider read at its immutable revision, and a fresh durable graph receipt read.
Match complete payload values and exact written bytes/digest/size, approved path, base, actor, tenant, mode and receipt identity.
A returned applied flag or request-key echo is insufficient evidence.

For the original blueprint compatibility obligation, pin a manifest of every prompt blueprint at the selected source revision.
Record the complete file inventory and actual count; the original input described more than sixty prompts.
Load, modify one declared field, export and reimport every entry, preserving all remaining custom fields and exact numeric values.
Check declared JSON formatting and final-newline policy when producing repository bytes; those exact bytes enter the approved Graft SDK payload.
An omitted file or parse/schema failure fails corpus qualification. Lost extension fields and rounded values also fail.
Repository writes remain separately approved, single-file effects; corpus coverage cannot grant a batch write permission.

Repeat absent/mismatched approval, stale base, path escape, changed bytes, wrong actor/tenant and unbound receipt cases through both surfaces.
Each pre-effect refusal must produce zero provider mutations; a later readback failure must preserve uncertainty without an additional write.
Race identical requests and restart Arbora plus the connector. Recover the same proven receipt or reconcile before any retry.
Record exact Medulla/Graft SDK/Arbora implementation heads separately from the merged definition revisions.
No new runtime receipt or lifecycle promotion follows from these contract links.

R002.10 still requires a separately named Meristem owning requirement and production dispatch call site.
Its existing valid/invalid model-transport acceptance is unchanged. Repository persistence evidence cannot substitute for that consumer proof.

<!-- spec-definition-patch:owner-batch-20261012-typed-packs-and-catalog -->
## Owner acceptance targets

All tests below are planned and unexecuted; they create no implementation or served receipts.

- **`MEDULLA-TYPED-PACKS-R100.7`:** Matching exact-package passing validation evidence enables the catalog package. Require `run_set_complete=true` and canonical sorted unique run refs. Missing evidence, wrong tenant, candidate/package/graph/policy digest mismatch, incomplete run set, stale source or failed verdict refuses. The refusal leaves no publishable package. Re-run current dangling-ontology refusal. Hold positive Meristem receipt production and its pass criterion until Meristem defines the exact operation and rubric. Source record SHA-256: `96ff38761f9e21d52f608ba593c902aff4c93ca7b1ed13088ad39efd3e968a3a`.
- **`MEDULLA-TYPED-PACKS-R097.2.7`:** Import a pinned schema with every supported term kind and mapped/unmapped examples. Independently read full coverage through `OntologyInspect` after commit/restart. Read class activation through `GraphSchemaClasses`. Verify the exact sorted `Mapped`/`Unmapped{NoDeclaredMapping}` report and provenance. Verify unmapped terms are not adopted, order/digest are deterministic, partial packs are admitted, and R071.1 reverse links remain unchanged. Each malformed, duplicate, unsupported-kind, kind-mismatch, stale or forged case refuses with pack head and active schema unchanged.
- **`MEDULLA-TYPED-PACKS-R100.8`:** Export/import a workflow with named graphs, unknown predicates, typed and language-tagged literals, and ontology references. Preserve the canonical N-Quads SHA-256 digest and RDF graph isomorphism. Tampered digest, invalid RDF, unresolved references and unsupported serializer refuse without writes. Equal/older export refusal and newer export admission require a resolved stored-version baseline/order contract; positive end-to-end consumer remains held. Source record SHA-256: `620b1ae05d8e35696c7522f207672ef6f374525004d5488fe62fb4b27fc6cf5a`.
<!-- /spec-definition-patch -->

<!-- spec-definition-patch:reviewed-mapped-subsets-20261012 -->
## Planned mapped acceptance

No test named here has run; names describe acceptance obligations, not evidence.

- **`MEDULLA-TYPED-PACKS-R069.100` (AU:69; source `8ca81dc16448fefc73440f2eb9ea07b2e433f7e38075563f86800071029846e9`):** Exercise five outcomes. (1) Exact repeated full identity/profile is fully idempotent. (2) An identical blob at another branch/ref or normalized path writes a distinct source binding and provenance. This holds even when parsed content is reused. (3) A new commit with cosmetic-only content writes source revision/provenance but leaves semantic graph revision unchanged. (4) A parser/normalization-profile revision reparses the same raw blob and records the new profile binding. Graph mutation occurs only if the recomputed semantic fingerprint changes. (5) Structural token changes update both source binding and semantic graph revision. In every case, read back source-binding revision/digest separately from semantic graph revision. Unsupported language uses raw digest and is not classified cosmetic. A source identity plus profile key, not raw blob digest alone, determines full no-op. Producer/consumer boundary: RepositorySnapshotProvider -> Graft SDK transport -> generated IndexRepository -> Medulla parser/index mutation. R079 embedding dedupe is not structural-mutation suppression. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only; no implementation or receipt is asserted. Planned proving tests (names only; not present or executed evidence): `tests/typed_packs/test_repository_fingerprint_identity_served.py::test_source_identity_profile_drives_mutation_decision`, `tests/typed_packs/test_repository_fingerprint_identity_served.py::test_unsupported_language_uses_raw_digest_only`. No runtime/test receipt is asserted.

- **`MEDULLA-TYPED-PACKS-R100.100` (AU:90; source `1b717c4a284896dbf8c7b4fb1f00ebba80fa6a75e930a020c5bc01706634f582`):** A served export/edit/reimport test changes one frontmatter value and one step. Assert all non-target byte ranges, custom fields, comments, spacing, ordering and CRLF/LF style remain identical. Malformed anchors, overlapping patches, stale source digest and unknown edit spans refuse without writes. A prompt-blueprint JSON roundtrip is not accepted as evidence. Producer/consumer boundary: Medulla owns skill parse/admission and R100 rendering; Graft SDK owns source transport/edit write-back. Do not substitute a prompt blueprint or whole-document normalization path. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only; no implementation or receipt is asserted. Planned proving tests (names only; not present or executed evidence): `tests/typed_packs/test_skill_lossless_edit_served.py::test_frontmatter_and_step_edits_preserve_unmodified_bytes`, `tests/typed_packs/test_skill_lossless_edit_served.py::test_stale_or_ambiguous_edit_refuses_before_write`. No runtime/test receipt is asserted.

- **`MEDULLA-TYPED-PACKS-R100.101` (AU:98; source `531cb0ba1accd9e88833249fb253a9f5f7a3c1a911f653b3c9a5bfc42341f978`):** Serve compatible upgrade, breaking upgrade without migration (refused before effects), and breaking upgrade with exact declared migration. Changing a consumer contract, package digest or migration receipt invalidates admission. Keep historical package readable and read back the admitted new version only after validation. Producer/consumer boundary: Medulla skill package read/admission -> Arbora invoke certification and consumer code; Arbora owns its invocation gate. Do not equate existing version/dependency pins with compatibility. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only; no implementation or receipt is asserted. Planned proving tests (names only; not present or executed evidence): `tests/typed_packs/test_skill_compatibility_admission_served.py::test_breaking_upgrade_requires_exact_consumer_migration`, `tests/typed_packs/test_skill_compatibility_admission_served.py::test_changed_consumer_contract_invalidates_admission`. No runtime/test receipt is asserted.

- **`MEDULLA-TYPED-PACKS-R001.100` (AU:52; source `e03b25156a4f84d27c4afba63d0cd9e526b549b5905994a53664d193416d048c`):** Round-trip nested schemas and version metadata through generated clients and canonical digest vectors. Test additive optional-field evolution, required-field removal, incompatible type change, stale base digest and unknown nested fields. Verify Graft SDK authority remains separate and no tool dispatch occurs from pack admission. Producer/consumer boundary: Medulla ConnectorPack typed authority -> generated public pack APIs; Graft SDK retains tool compatibility and execution authority. Preserve R001 index/entry/section/annotation and digest contract. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only; no implementation or receipt is asserted. Planned proving tests (names only; not present or executed evidence): `tests/typed_packs/test_connector_nested_schema_served.py::test_nested_schema_evolution_preserves_digest`, `tests/typed_packs/test_connector_nested_schema_served.py::test_incompatible_nested_schema_refuses_before_dispatch`. No runtime/test receipt is asserted.

- **`MEDULLA-TYPED-PACKS-R072.100` (AU:97; source `f73767d1a3fdba29a702254ed1db2c9904158b6136f6f4b38a3c9a0d3e4fcb03`):** Served registry tests register and read a workflow. Prove no eager Meristem engine initialization and one initialization on first invocation. Invoke by registry id. Refuse unknown/withdrawn/stale ids with no run. Read back registry and invocation receipt independently. Producer/consumer boundary: Medulla typed registry and SourceIngest R002 own durable registration; Meristem owns lazy client initialization/execution. Preserve ARBORA-HOST-R022 workflow-store semantics. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only; no implementation or receipt is asserted. Planned proving tests (names only; not present or executed evidence): `tests/typed_packs/test_workflow_registry_served.py::test_registration_is_lazy_and_invokes_exact_revision`, `tests/typed_packs/test_workflow_registry_served.py::test_unknown_withdrawn_or_stale_registry_id_refuses`. Owner prerequisite: MERISTEM-CONTROL-R037.5 fills only a requirement gap. It does not define lazy initialization timing, the first-use operation or the execution receipt. No exact Meristem companion requirement ID is identified. Keep that consumer as an explicit open dependency until its owner publishes the operation and receipt. No invocation evidence is claimed. No runtime/test receipt is asserted.

- **`MEDULLA-TYPED-PACKS-R071.100` (EG:8; source `525e39d1d194ac8a820bbd741a1c261ceab0c7d564981c33c731c06c1eb641d8`):** Cross-repository fixtures admit a new unique path. They refuse duplicate canonical paths, alias collisions, incompatible parent assignments and stale vocabulary revisions. Read back canonical path and unchanged published IRIs. Retain R071 generated ontology authority and R001.4 digest vectors. Producer/consumer boundary: Medulla vocabulary validation owns canonical identity; repository producers submit proposed paths. Preserve existing published IRIs and federated query vocabulary. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only; no implementation or receipt is asserted. Planned proving tests (names only; not present or executed evidence): `tests/typed_packs/test_concept_path_admission_served.py::test_owner_approved_path_grammar_is_shared`, `tests/typed_packs/test_concept_path_admission_served.py::test_duplicate_or_conflicting_paths_refuse_without_iri_changes`. Owner decision hold: the canonical segment grammar is not approved for separators/escaping, Unicode normalization, case, or whitespace. Keep admission SPECIFIED until the vocabulary owner publishes one versioned grammar shared by producers; preserve existing published IRIs. No runtime/test receipt is asserted.
<!-- /spec-definition-patch -->

## Connector packs as native RDF (R101, R102)

- `MEDULLA-TYPED-PACKS-R101`: Rollup closes when every live child passes its proving test.
- `MEDULLA-TYPED-PACKS-R101.1`: `crates/medulla-core/tests/connector_pack_vocabulary.rs::contract_terms_resolve_with_domain_and_range`.
- `MEDULLA-TYPED-PACKS-R101.2`: `crates/medulla-shacl/tests/connector_pack_shapes.rs::each_contract_shape_accepts_valid_and_refuses_invalid`.
- `MEDULLA-TYPED-PACKS-R101.3`: Retired; see R101.10 and R101.11.
- `MEDULLA-TYPED-PACKS-R101.4`: Retired; see R101.10 and R101.11.
- `MEDULLA-TYPED-PACKS-R101.5`: Retired; see R101.10 and R101.11.
- `MEDULLA-TYPED-PACKS-R101.6`: Retired; see R101.10 and R101.11.
- `MEDULLA-TYPED-PACKS-R101.7`: Retired; see R101.10 and R101.11.
- `MEDULLA-TYPED-PACKS-R101.8`: Retired; see R101.10 and R101.11.
- `MEDULLA-TYPED-PACKS-R101.9`: a served test imports a pack through `ConnectorPack.Import` and submits one mapped record through `SourceIngest`; the engine accepts it.
- `MEDULLA-TYPED-PACKS-R101.10`: `crates/medulla-h-admin/src/admin/connector_pack/tests/scale.rs::the_selector_resolves_to_the_declared_class_and_field_map`; `crates/medulla-h-admin/src/admin/connector_pack/tests/scale.rs::relations_resolve_from_the_pack_ontology`.
- `MEDULLA-TYPED-PACKS-R101.11`: `crates/medulla-h-admin/src/admin/connector_pack/tests/scale.rs::an_undeclared_type_is_refused_with_a_typed_error`.
- `MEDULLA-TYPED-PACKS-R102`: Rollup closes when every child `.1` to `.7` passes its proving test.
- `MEDULLA-TYPED-PACKS-R102.1`: `crates/medulla-shacl/tests/connector_pack_shapes.rs::surface_shapes_accept_valid_and_refuse_invalid`.
- `MEDULLA-TYPED-PACKS-R102.2`: `src/server/connector_pack_projection/surface_tests.rs::surface_nodes_follow_pack_head`.
- `MEDULLA-TYPED-PACKS-R102.3`: `src/server/handlers/admin/connector_pack/import/contract_tests.rs::core_class_redeclaration_is_refused`.
- `MEDULLA-TYPED-PACKS-R102.4`: `src/server/connector_pack_projection/surface_tests.rs::outbox_consumer_writes_surface_nodes`.
- `MEDULLA-TYPED-PACKS-R102.5`: `src/server/handlers/admin/connector_pack/import/contract_tests.rs::surface_refusals_leave_state_unchanged`.
- `MEDULLA-TYPED-PACKS-R102.6`: `src/server/connector_pack_projection/surface_tests.rs::surface_nodes_carry_pack_digest`.
- `MEDULLA-TYPED-PACKS-R102.7`: `tests/typed_packs/test_connector_surface_rdf_served.py::test_surface_query_follows_pack_head`.
- `MEDULLA-TYPED-PACKS-R103`: Rollup closes when every child `.1` to `.6` passes its proving test.
- `MEDULLA-TYPED-PACKS-R103.1`: `crates/medulla-catalog-types/src/graph_schema.rs::tests::ontology_import_ops_round_trip`.
- `MEDULLA-TYPED-PACKS-R103.2`: `src/server/graph_schema/ontology_import_tests.rs::import_is_idempotent_by_digest_and_revises`.
- `MEDULLA-TYPED-PACKS-R103.3`: `src/server/graph_schema/ontology_import_tests.rs::gateway_routes_ontology_import`.
- `MEDULLA-TYPED-PACKS-R103.4`: `src/server/graph_schema/ontology_import_tests.rs::ontology_import_refusals_leave_schema_unchanged`.
- `MEDULLA-TYPED-PACKS-R103.5`: `src/server/graph_schema/ontology_import_tests.rs::revision_records_digest_importer_and_classes`.
- `MEDULLA-TYPED-PACKS-R103.6`: `tests/typed_packs/test_runtime_ontology_import_served.py::test_import_revise_withdraw_end_to_end`.

## MEDULLA-TYPED-PACKS-R104 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-TYPED-PACKS-R104.1` | `crates/medulla-catalog-types/src/connector_pack/seed_graph.rs::tests::seed_import_round_trips`: A serde round trip keeps every field and rejects an unknown part key. |

| Behavior | `MEDULLA-TYPED-PACKS-R104.2` | `crates/medulla-h-admin/src/admin/connector_pack/seed_tests.rs::changed_part_reimports_alone`: A second import with one changed part replaces that part and leaves the other two byte-identical. |

| Wiring | `MEDULLA-TYPED-PACKS-R104.3` | `crates/medulla-h-admin/src/admin/connector_pack/seed_tests.rs::served_import_projects_seed`: A served import with a seed body reaches `project_head` and writes the seed triples. |

| Refusal | `MEDULLA-TYPED-PACKS-R104.4` | `crates/medulla-h-admin/src/admin/connector_pack/seed_tests.rs::seed_refusals_write_nothing`: Each case returns its code and the graph revision stays unchanged. |

| Evidence | `MEDULLA-TYPED-PACKS-R104.5` | `crates/medulla-h-admin/src/admin/connector_pack/seed_tests.rs::receipt_names_part_outcomes`: A served SPARQL read finds the receipt and the `seed:fromSeed` link of one catalog object. |

| Hot-path proof | `MEDULLA-TYPED-PACKS-R104.6` | `tests/connector_pack/test_seed_graph_served.py::test_seed_idempotent_and_partial_reimport`: The repeat returns `unchanged`, the change replaces only `catalog`, and the query sees the new objects. |


Hot-path proof for `MEDULLA-TYPED-PACKS-R104`: `tests/connector_pack/test_seed_graph_served.py::test_seed_idempotent_and_partial_reimport`.

## R085.3 session-liveness check

`MEDULLA-TYPED-PACKS-R085.3`: served tests cover live, revoked and expired answers. Revoked covers single and revoke_all. Expired covers idle and absolute expiry under a fake clock. Further tests cover `not_session_bound` for API-key and external-identity callers. Others cover refusal of unknown and cross-tenant references, no token text on the wire, and no idle-expiry extension. All stay pending until the operation exists.

## R085.1 durable session projection

The native session_evidence source fixtures cover post-touch facts, distinct
references, omitted legacy/unbound metadata and existing refusals. Their source
presence does not establish execution. Server stamping fixtures require caller
metadata replacement and positive creation under the configured test policy.
These checks do not qualify Arbora admission or a live host startup.

`verified_api_key_resolution_omits_session_record` executes the existing
IssueApiKey and VerifyApiKey operations and checks wire omission.
`failed_session_touch_persistence_publishes_no_record_projection` uses the
existing failing RBAC persistence store and asserts the touch returns an error,
restores the prior state, and publishes facts only after a successful retry.
Its existing none-mode fixture is storage testing, not served admission proof.

`exact_session_facts_and_revocation_survive_durable_reopen` uses the existing
native RbacStore test opener and checked writes. It closes and reopens the redb
store after touch and revocation, asserts exact reference, creation tenant and
MFA timestamp, and refuses another credential. Test-owned storage proofs do not
establish production admission. `resolve_preserves_creation_tenant_and_exact_mfa_timestamp`
refuses to overwrite stored provenance with a later stamp's metadata.
`successful_session_resolution_runs_boundary_store_and_public_response` requires
creation-to-resolution reference continuity. Existing pending-MFA served refusal
requires an error response with no result. Canonical artifact freshness passes. Newly added native cases remain pending execution.

## Glossary, stewardship, sensitive data detection, ontology change impact and data products

| Row | Proving test |
|---|---|
| `MEDULLA-TYPED-PACKS-R801` | See each child. |
| `MEDULLA-TYPED-PACKS-R801.1` | `crates/medulla-types/tests/glossary.rs::term_round_trips_and_maps_to_skos` |
| `MEDULLA-TYPED-PACKS-R801.2` | `crates/medulla-core/tests/glossary.rs::alt_label_search_finds_term_and_deprecated_names_replacement` |
| `MEDULLA-TYPED-PACKS-R801.3` | `tests/typed_packs/test_glossary_served.py::test_import_scheme_and_bind_term` |
| `MEDULLA-TYPED-PACKS-R801.4` | `tests/typed_packs/test_glossary_served.py::test_glossary_refusals` |
| `MEDULLA-TYPED-PACKS-R801.5` | `tests/typed_packs/test_glossary_served.py::test_status_history_reads_back` |
| `MEDULLA-TYPED-PACKS-R801.6` | `tests/typed_packs/test_glossary_served.py::test_import_scheme_bind_term_and_review_lifecycle` |
| `MEDULLA-TYPED-PACKS-R802` | See each child. |
| `MEDULLA-TYPED-PACKS-R802.1` | `crates/medulla-types/tests/stewardship.rs::records_round_trip` |
| `MEDULLA-TYPED-PACKS-R802.2` | `crates/medulla-core/tests/stewardship.rs::most_specific_assignment_wins_and_certification_expires` |
| `MEDULLA-TYPED-PACKS-R802.3` | `tests/typed_packs/test_stewardship_served.py::test_change_request_flow` |
| `MEDULLA-TYPED-PACKS-R802.4` | `tests/typed_packs/test_stewardship_served.py::test_stewardship_refusals` |
| `MEDULLA-TYPED-PACKS-R802.5` | `tests/typed_packs/test_stewardship_served.py::test_request_evidence_reads_back` |
| `MEDULLA-TYPED-PACKS-R802.6` | `tests/typed_packs/test_stewardship_served.py::test_change_request_flow` |
| `MEDULLA-TYPED-PACKS-R803` | See each child. |
| `MEDULLA-TYPED-PACKS-R803.1` | `crates/medulla-types/tests/sensitive_detect.rs::records_round_trip`, `crates/medulla-h-jobs/src/jobs/sensitive_scan_served_tests.rs::profile_job_proposes_claim_without_values` |
| `MEDULLA-TYPED-PACKS-R803.2` | `crates/medulla-core/tests/sensitive_detect.rs::scan_proposes_claim_without_values` |
| `MEDULLA-TYPED-PACKS-R803.3` | `crates/medulla-h-jobs/src/jobs/sensitive_scan_served_tests.rs::profile_job_proposes_claim_without_values` |
| `MEDULLA-TYPED-PACKS-R803.4` | `crates/medulla-h-jobs/src/jobs/sensitive_scan_served_tests.rs::detector_and_promotion_refusals` |
| `MEDULLA-TYPED-PACKS-R803.5` | `crates/medulla-h-jobs/src/jobs/sensitive_scan_served_tests.rs::claim_evidence_reads_back` |
| `MEDULLA-TYPED-PACKS-R803.6` | `crates/medulla-core/benches/sensitive_scan.rs`, `crates/medulla-h-jobs/src/jobs/sensitive_scan_served_tests.rs::profile_job_proposes_claim_without_values` |
| `MEDULLA-TYPED-PACKS-R804` | See each child. |
| `MEDULLA-TYPED-PACKS-R804.1` | `crates/medulla-types/tests/ontology_diff.rs::diff_round_trips` |
| `MEDULLA-TYPED-PACKS-R804.2` | `crates/medulla-h-admin/src/graph_schema/ontology_diff_tests.rs::removed_class_reports_impact` |
| `MEDULLA-TYPED-PACKS-R804.3` | `crates/medulla-h-admin/src/graph_schema/ontology_diff_tests.rs::import_reports_impact_and_opens_change_request` |
| `MEDULLA-TYPED-PACKS-R804.4` | `crates/medulla-h-admin/src/graph_schema/ontology_diff_tests.rs::diff_refusals` |
| `MEDULLA-TYPED-PACKS-R804.5` | `crates/medulla-h-admin/src/graph_schema/ontology_diff_tests.rs::impact_names_items` |
| `MEDULLA-TYPED-PACKS-R804.6` | `crates/medulla-h-admin/benches/ontology_diff.rs`, `crates/medulla-h-admin/src/graph_schema/ontology_diff_tests.rs::import_reports_impact_and_opens_change_request` |
| `MEDULLA-TYPED-PACKS-R805` | See each child. |
| `MEDULLA-TYPED-PACKS-R805.1` | `crates/medulla-types/tests/data_product.rs::records_round_trip` |
| `MEDULLA-TYPED-PACKS-R805.2` | `crates/medulla-core/tests/data_product.rs::breach_records_violation` |
| `MEDULLA-TYPED-PACKS-R805.3` | `crates/medulla-h-admin/src/catalog/data_product_served_tests.rs::request_grants_access_after_steward_approval` |
| `MEDULLA-TYPED-PACKS-R805.4` | `crates/medulla-h-admin/src/catalog/data_product_served_tests.rs::product_refusals` |
| `MEDULLA-TYPED-PACKS-R805.5` | `crates/medulla-h-admin/src/catalog/data_product_served_tests.rs::violation_evidence_reads_back` |
| `MEDULLA-TYPED-PACKS-R805.6` | `crates/medulla-core/src/data_product.rs::tests::contract_check_is_linear_in_promises`, `crates/medulla-core/benches/data_contract_check.rs` |

## Trust flags, critical data elements and model-proposed curation

| Row | Proving test |
|---|---|
| `MEDULLA-TYPED-PACKS-R801.7` | `tests/typed_packs/test_glossary_served.py::test_proposed_description_needs_review` |
| `MEDULLA-TYPED-PACKS-R802.7` | `tests/typed_packs/test_stewardship_served.py::test_deprecation_lowers_rank_and_emits_event` |
| `MEDULLA-TYPED-PACKS-R802.8` | `tests/typed_packs/test_stewardship_served.py::test_critical_element_gaps_reported` |
| `MEDULLA-TYPED-PACKS-R805.7` | `crates/medulla-h-admin/src/catalog/data_product_served_tests.rs::usage_cluster_yields_product_proposal` |

- `MEDULLA-TYPED-PACKS-R106`: See each child.
- `MEDULLA-TYPED-PACKS-R106.1`: `crates/medulla-rdf/tests/metadata_catalog_classes.rs::catalog_classes_roll_up_to_eg_core_and_align_to_w3c`, `crates/medulla-rdf/tests/metadata_catalog_classes.rs::owner_steward_and_lineage_properties_specialise_prov`, `crates/medulla-rdf/tests/metadata_catalog_classes.rs::catalog_shapes_accept_a_complete_catalog`, `crates/medulla-rdf/tests/metadata_catalog_classes.rs::catalog_shapes_refuse_incomplete_or_open_vocabulary_records`.
- `MEDULLA-TYPED-PACKS-R106.2`: `crates/medulla-rdf/tests/metadata_catalog_classes.rs::every_asset_has_owner_version_lineage_pair_and_class_link`.
- `MEDULLA-TYPED-PACKS-R106.3`: `tests/typed_packs/test_connector_catalog_served.py::test_onboarding_batch_is_validated_by_the_guard_and_found_by_query`.
- `MEDULLA-TYPED-PACKS-R106.4`: `tests/typed_packs/test_connector_catalog_served.py::test_unresolved_class_unowned_resource_unpaired_edge_and_oversize_batch_refuse`.
- `MEDULLA-TYPED-PACKS-R106.5`: `tests/typed_packs/test_connector_catalog_served.py::test_asset_carries_digest_version_revision_and_class_counts`.
- `MEDULLA-TYPED-PACKS-R106.6`: `tests/typed_packs/test_connector_catalog_served.py::test_catalog_search_finds_assets_with_owner_term_and_lineage`.
- `MEDULLA-TYPED-PACKS-R106.7`: `tests/typed_packs/test_catalog_competency_queries.py::test_owner_lineage_term_and_class_queries_answer_from_the_graph`. Vectors, with `X` the fixture's first tool:
  - Owner: `SELECT ?principal ?role WHERE { ?s cat:assignedTo <X> ; cat:principal ?principal ; cat:role ?role }` returns one row, role `owner`.
  - Upstream lineage: `SELECT ?up WHERE { ?up cat:lineageTo+ <X> }` returns the connector and the server.
  - Downstream lineage: `SELECT ?down ?hop WHERE { <X> cat:lineageTo+ ?down . ?e cat:from <X> ; cat:to ?down ; cat:hop ?hop }` returns the resource and its fields.
  - Glossary term: `SELECT ?term ?label WHERE { ?term :definesAsset <X> ; skos:prefLabel ?label }` returns the term that names the tool's class.
  - Parent class: `SELECT ?asset WHERE { ?asset a/rdfs:subClassOf* cat:Asset ; cp:ofConnector <C> }` returns the two tools, the skill, the prompt, the two resources and their fields.
  - UQL equivalents return the same rows, for example `MATCH (s:StewardshipAssignment)-[:assignedTo]->(a {id: X}) RETURN s.principal, s.role`.
- `MEDULLA-TYPED-PACKS-R107`: See each child.
- `MEDULLA-TYPED-PACKS-R107.1`: `crates/medulla-types/tests/pack_contract.rs::version_grant_and_break_round_trip`.
- `MEDULLA-TYPED-PACKS-R107.2`: `crates/medulla-h-admin/src/admin/connector_pack/contract_diff.rs::tests::non_breaking_applies_breaking_waits_and_queries_use_active_version`.
- `MEDULLA-TYPED-PACKS-R107.3`: `tests/typed_packs/test_pack_contract_served.py::test_diff_runs_in_import_and_orphan_report_is_served`.
- `MEDULLA-TYPED-PACKS-R107.4`: `tests/typed_packs/test_pack_contract_served.py::test_ungranted_incomplete_breaking_and_in_use_refuse`.
- `MEDULLA-TYPED-PACKS-R107.5`: `tests/typed_packs/test_pack_contract_served.py::test_receipt_carries_digests_break_count_request_and_grant`.
- `MEDULLA-TYPED-PACKS-R107.6`: `tests/typed_packs/test_pack_contract_served.py::test_breaking_release_waits_for_approval_then_serves`.

- `MEDULLA-TYPED-PACKS-R108`: See each child.
- `MEDULLA-TYPED-PACKS-R108.1`: `crates/medulla-rdf/tests/metadata_catalog_classes.rs::six_families_validate_and_share_one_parent_asset_class`.
- `MEDULLA-TYPED-PACKS-R108.2`: `crates/medulla-catalog/src/emit.rs::tests::existing_records_emit_with_owner_term_and_lineage_without_copies`.
- `MEDULLA-TYPED-PACKS-R108.3`: `tests/typed_packs/test_catalog_assets_served.py::test_publish_register_and_create_reach_the_one_emitter`.
- `MEDULLA-TYPED-PACKS-R108.4`: `tests/typed_packs/test_catalog_assets_served.py::test_orphan_class_missing_owner_and_marked_field_are_handled`.
- `MEDULLA-TYPED-PACKS-R108.5`: `tests/typed_packs/test_catalog_assets_served.py::test_node_carries_source_owner_steward_revision_and_history`.
- `MEDULLA-TYPED-PACKS-R108.6`: `tests/typed_packs/test_catalog_assets_served.py::test_one_query_returns_agents_business_workflows_graphs_and_connector_assets`.

- `MEDULLA-TYPED-PACKS-R108.7`: `tests/typed_packs/test_catalog_capabilities_served.py::test_contract_harvest_stores_schema_keys_views_indexes_and_no_rows`
- `MEDULLA-TYPED-PACKS-R108.8`: `tests/typed_packs/test_catalog_capabilities_served.py::test_tool_call_and_query_provenance_feed_usage_frequency_and_top_queries`
- `MEDULLA-TYPED-PACKS-R108.9`: `tests/typed_packs/test_catalog_capabilities_served.py::test_sensitivity_propagates_downstream_as_proposal_and_asset_names_its_policy`
- `MEDULLA-TYPED-PACKS-R108.10`: `tests/typed_packs/test_catalog_capabilities_served.py::test_lineage_walks_source_operation_workflow_dashboard_agent_and_model`
- `MEDULLA-TYPED-PACKS-R108.11`: `tests/typed_packs/test_catalog_capabilities_served.py::test_metric_formula_binds_terms_and_unknown_term_refuses`
- `MEDULLA-TYPED-PACKS-R108.12`: `tests/typed_packs/test_catalog_capabilities_served.py::test_annotations_attach_to_any_asset_and_trust_flags_reuse_the_steward_path`
- `MEDULLA-TYPED-PACKS-R108.13`: `tests/typed_packs/test_catalog_capabilities_served.py::test_article_attaches_to_asset_and_domain_and_is_found_by_text_and_by_asset`
- `MEDULLA-TYPED-PACKS-R108.14`: `tests/typed_packs/test_catalog_capabilities_served.py::test_dashboard_report_saved_query_and_template_register_with_reads_and_usage`
- `MEDULLA-TYPED-PACKS-R108.15`: `tests/typed_packs/test_catalog_capabilities_served.py::test_model_asset_links_registry_ref_schemas_and_training_lineage_without_weights`
- `MEDULLA-TYPED-PACKS-R108.16`: `tests/typed_packs/test_catalog_capabilities_served.py::test_one_search_ranks_all_families_with_usage_and_endorsement_boost`

- `MEDULLA-TYPED-PACKS-R109`: See each child.
- `MEDULLA-TYPED-PACKS-R109.1`: `crates/medulla-types/tests/dashboard.rs::dashboard_asset_round_trips_and_shape_validates`.
- `MEDULLA-TYPED-PACKS-R109.2`: `crates/medulla-core/tests/dashboard_asset.rs::save_bumps_revision_rebuilds_links_and_delete_keeps_then_purges`.
- `MEDULLA-TYPED-PACKS-R109.3`: `tests/typed_packs/test_dashboard_asset_served.py::test_save_share_delete_restore_reach_the_handler_with_grant_checks`.
- `MEDULLA-TYPED-PACKS-R109.4`: `tests/typed_packs/test_dashboard_asset_served.py::test_stale_revision_missing_grant_invalid_body_and_purged_asset_refuse`.
- `MEDULLA-TYPED-PACKS-R109.5`: `tests/typed_packs/test_dashboard_asset_served.py::test_revision_grant_and_delete_history_and_usage_are_served`.
- `MEDULLA-TYPED-PACKS-R109.6`: `tests/typed_packs/test_dashboard_asset_served.py::test_dashboard_save_conflict_share_lineage_search_and_soft_delete`.

## R085.7 original session reference inspection

`original_reference_inspection_is_owner_bound_and_does_not_touch` checks exact stored reference, stamped principal/tenant, expiry and revocation refusals. The durable R085.1 reopen fixture also inspects the original record through `SessionOp::InspectReference`. Neither fixture proves served admission. The unit fixture passed in the 2026-10-20 run recorded below. Durable execution remains pending; canonical generation passed.

## R085.4 global authority revision renewal

`renewal_uses_global_revision_and_survives_reopen_without_resurrection` exercises the durable owner. The expected revision is global, so unrelated authority mutations also require a fresh read. Renewal never extends absolute authentication lifetime. This durable fixture remains unexecuted. Canonical generation passed; see the evidence below.

`renewal_preserves_original_scope_ceiling_and_refuses_legacy_evidence` checks that the captured ceiling survives serialization. Missing legacy evidence and expanded scopes refuse. Narrowed scopes can renew without changing the ceiling. Existing credential resolution behavior is unchanged.

## R085.5 approval key lifecycle

Source fixtures cover create/read/persistence, rotation/revocation CAS and exact subject/tenant/purpose. They also cover expired or pending sessions, disabled/service subjects and absent broker authority. Canonical proof bytes are deterministic; native tests do not verify broker proof or served enrollment. The pure store tests passed in the run below. Durable, server and served qualification remain pending.

## R085.6 non-touch credential resolution

The existing Resolve operation accepts optional `refresh`. False preserves the current credential authority checks and returns the existing resolution without touching session state. Native test sources cover no mutation, exact expiry, revoke despite false, missing authority and omitted-field compatibility. The durable session fixture checks unchanged revision and policy image. The unit tests and canonical generation passed. Durable execution, codec checks and served qualification remain pending.

## R085 acceptance qualification layers

R085.4–R085.7 preserve their parent obligations as rollups. Their .1 children cover pure store transitions; .2 children require durable owner execution. R085.5.3 covers request stamping only. R085.7.3, R085.4.3, R085.5.4 and R085.6.3 retain generated/served qualification. A passing unit filter is not evidence for unexecuted durable or served acceptance. Source markers bind only their matching layer.

## Bound tests

- `MEDULLA-TYPED-PACKS-R105.3`: `crates/medulla-rdf/tests/connector_parent_classes.rs::recurring_connector_concepts_sit_under_their_upper_classes`.
- `MEDULLA-TYPED-PACKS-R105.3.1`: `crates/medulla-rdf/tests/connector_parent_classes.rs::source_record_classes_have_narrow_parents`.
- `MEDULLA-TYPED-PACKS-R105.4`: `crates/medulla-rdf/tests/service_operations_classes.rs::service_operations_classes_exist_with_their_parents`, `crates/medulla-rdf/tests/service_operations_classes.rs::competency_question_edges_declare_domain_and_range`.
- `MEDULLA-TYPED-PACKS-R105.5`: `crates/medulla-rdf/tests/service_operations_classes.rs::self_contained_modules_align_to_the_core`.
- `MEDULLA-TYPED-PACKS-R105.6`: `crates/medulla-rdf/tests/domain_modules_classes.rs::it4it_delivery_classes_exist_with_their_parents`, `crates/medulla-rdf/tests/domain_modules_classes.rs::it4it_delivery_edges_declare_domain_and_range`.
- `MEDULLA-TYPED-PACKS-R105.7`: `crates/medulla-rdf/tests/domain_modules_classes.rs::dataplatform_classes_exist_with_their_parents`, `crates/medulla-rdf/tests/domain_modules_classes.rs::dataplatform_edges_declare_domain_and_range`.
- `MEDULLA-TYPED-PACKS-R105.8`: `crates/medulla-rdf/tests/domain_modules_classes.rs::media_classes_exist_with_their_parents`, `crates/medulla-rdf/tests/domain_modules_classes.rs::media_edges_declare_domain_and_range`.
- `MEDULLA-TYPED-PACKS-R105.9`: `crates/medulla-rdf/tests/domain_modules_classes.rs::retrieval_classes_exist_with_their_parents`, `crates/medulla-rdf/tests/domain_modules_classes.rs::retrieval_edges_declare_domain_and_range`.
- `MEDULLA-TYPED-PACKS-R700.7`: `crates/medulla-h-admin/src/admin/connector_pack/import/front_matter.rs::the_skill_type_is_closed`
- `MEDULLA-TYPED-PACKS-R700.8`: `tests/packs/test_skill_graph_pack_served.py::test_graph_files_import_and_non_graph_file_refuses`
- `MEDULLA-TYPED-PACKS-R701.1`: `crates/medulla-catalog-types/tests/skill_graph.rs::layer_round_trip_and_reject_foreign_iri`
- `MEDULLA-TYPED-PACKS-R701.2`: `crates/medulla-h-admin/src/admin/connector_pack/import/skill_graph.rs::tests::layer_iri_and_rollup_checked_in_one_pass`
- `MEDULLA-TYPED-PACKS-R701.3`: `tests/packs/test_skill_graph_pack_served.py::test_layer_classes_read_through_core_class`
- `MEDULLA-TYPED-PACKS-R701.4`: `tests/packs/test_skill_graph_pack_served.py::test_layer_refusals_leave_state_unchanged`
- `MEDULLA-TYPED-PACKS-R701.5`: `tests/packs/test_skill_graph_pack_served.py::test_layered_graph_imports_and_orphan_layer_refuses`
- `MEDULLA-TYPED-PACKS-R700.1`: `crates/medulla-catalog-types/tests/skill_graph.rs::manifest_report_candidate_round_trip_and_reject_bad_digest`
- `MEDULLA-TYPED-PACKS-R700.2`: `crates/medulla-h-admin/src/admin/connector_pack/import/skill_graph.rs::tests::single_pass_rules_and_violation_cap_on_10_1000_100000_entries`
- `MEDULLA-TYPED-PACKS-R700.3`: `tests/packs/test_skill_graph_pack_served.py::test_bundle_validated_in_import_and_candidate_renders_on_demand`
- `MEDULLA-TYPED-PACKS-R700.4`: `tests/packs/test_skill_graph_pack_served.py::test_hash_index_and_entry_refusals_leave_state_unchanged`
- `MEDULLA-TYPED-PACKS-R700.9`: `tests/packs/test_skill_graph_distill_served.py::test_distilled_bundle_validates_and_is_not_published`.

## Planned bindings

Each line binds one row to its proving test. A test marked planned does not exist yet; its path is the exact post-split location where it will be created and tagged with `// spec: <ID>`.

- `MEDULLA-TYPED-PACKS-R001`: `digest.rs::pack_digest` (planned); `tests/test_connector_pack_contract.py::test_pack_index_entry_contract` (planned)
- `MEDULLA-TYPED-PACKS-R001.1`: `crates/medulla-catalog-types/src/connector_pack/index.rs::tests::index_entry_round_trip_and_refusals` (planned)
- `MEDULLA-TYPED-PACKS-R001.2`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::check_annotations` (existing); `crates/medulla-catalog-types/src/connector_pack/annotations.rs::tests::new_fields_roundtrip` (existing); `crates/medulla-h-admin/src/admin/connector_pack/import.rs::tests::section_ranges_are_checked_without_wrapping` (existing)
- `MEDULLA-TYPED-PACKS-R001.3`: `crates/medulla-capabilities/tests/contract_registry.rs::every_wire_variant_has_exactly_one_descriptor_and_vice_versa` (existing); `crates/medulla-catalog-types/src/connector_pack/ops.rs::tests::operation_and_result_shapes_are_stable` (planned)
- `MEDULLA-TYPED-PACKS-R001.4`: `crates/medulla-catalog-types/src/connector_pack/digest.rs::tests::framed_digest_vectors_match` (existing)
- `MEDULLA-TYPED-PACKS-R001.5`: `tests/test_connector_pack_client.py::test_pack_digest_matches_the_rust_golden_vector` (existing); `tests/test_connector_pack_contract.py::test_python_client_matches_pack_vectors` (planned)
- `MEDULLA-TYPED-PACKS-R001.6`: `crates/medulla-agent-types/src/agent_component.rs::tests::prompt_blueprint_identity` (planned)
- `MEDULLA-TYPED-PACKS-R001.7`: `crates/medulla-agent-types/src/agent_component/facts.rs::tests::prompt_blueprint_schema_refusals` (planned)
- `MEDULLA-TYPED-PACKS-R001.8`: `crates/medulla-compute/src/assemble/prompt.rs::tests::prompt_render_vectors` (planned)
- `MEDULLA-TYPED-PACKS-R001.9`: `tests/test_prompt_blueprint_contract.py::test_prompt_wire_round_trip` (planned)
- `MEDULLA-TYPED-PACKS-R002.1`: `crates/medulla-h-admin/src/admin/connector_pack/tests/storage_acceptance.rs::body_staging` (planned)
- `MEDULLA-TYPED-PACKS-R002.2`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::holder_refcounts_transition_across_unchanged_revised_and_withdrawn_entries` (existing)
- `MEDULLA-TYPED-PACKS-R002.3`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::holder_refcounts_transition_across_unchanged_revised_and_withdrawn_entries` (existing)
- `MEDULLA-TYPED-PACKS-R002.4`: `crates/medulla-h-admin/src/admin/connector_pack/tests/withdrawal.rs::withdrawn_tool_leaves_search_but_keeps_historical_content` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::holder_refcounts_transition_across_unchanged_revised_and_withdrawn_entries` (existing)
- `MEDULLA-TYPED-PACKS-R002.5`: `crates/medulla-h-admin/src/admin/connector_pack/reconcile.rs::serve` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::the_reconciler_releases_an_orphan_body_and_keeps_every_held_one` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/storage_acceptance.rs::orphan_grace_fault` (planned)
- `MEDULLA-TYPED-PACKS-R002.6`: `crates/medulla-h-admin/src/admin/component_content.rs::handle_component_content` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/storage_acceptance.rs::verified_body_read` (planned)
- `MEDULLA-TYPED-PACKS-R002.7`: `tests/test_prompt_blueprints_served.py::test_prompt_owner_commit` (planned)
- `MEDULLA-TYPED-PACKS-R002.8`: `crates/medulla-h-admin/src/connector_pack_projection.rs::project` (existing); `tests/test_prompt_blueprints_served.py::test_prompt_graph_projection` (planned)
- `MEDULLA-TYPED-PACKS-R002.9`: `tests/test_prompt_blueprints_served.py::test_prompt_render_route` (planned)
- `MEDULLA-TYPED-PACKS-R002.10`: `tests/test_prompt_blueprints_served.py::test_receipt_verdicts` (planned)
- `MEDULLA-TYPED-PACKS-R002.11`: `tests/test_prompt_blueprints_served.py::test_prompt_restart_readback` (planned)
- `MEDULLA-TYPED-PACKS-R003`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::a_rejected_import_mutates_nothing_in_the_catalog` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::only_the_bound_importer_may_import` (existing)
- `MEDULLA-TYPED-PACKS-R004`: `tests/test_connector_pack_client.py::test_transport_loss_retry_reuses_the_exact_import_operation_key` (existing); `tests/test_connector_pack_client.py::test_typed_status_content_and_import_use_only_generated_wire_shapes` (existing)
- `MEDULLA-TYPED-PACKS-R005`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::an_end_to_end_connector_fixture_imports_through_to_a_served_read` (existing); `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R006`: `crates/medulla-h-admin/src/admin/connector_pack/tests/outbox.rs::an_admin_forced_reproject_redrives_a_stale_projection_without_losing_later_work` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/outbox.rs::failed_projection_records_durable_failed_status` (existing)
- `MEDULLA-TYPED-PACKS-R007`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::bind_unbind_and_retire_require_admin_connector_pack_and_the_head_survives_a_retired_reimport` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::a_retired_entry_can_never_return` (existing)
- `MEDULLA-TYPED-PACKS-R008`: `crates/medulla-server-blob/src/blob/store/tests.rs::a_referenced_then_released_blob_survives_grace_and_increfs_stay_idempotent_on_count` (existing); `crates/medulla-server-blob/src/blob/store/tests.rs::gc_reclaims_orphan_keeps_referenced_shared` (existing)
- `MEDULLA-TYPED-PACKS-R009`: `crates/medulla-h-admin/src/admin/connector_pack/tests/outbox.rs::a_pack_without_schema_is_visible_at_once_and_one_with_schema_after_projection` (existing)
- `MEDULLA-TYPED-PACKS-R010`: `crates/medulla-catalog-types/src/connector_pack/ops.rs::tests::pack_admission_exposes_only_import_with_no_staged_protocol_variant` (existing)
- `MEDULLA-TYPED-PACKS-R011`: `crates/medulla-persistence-ops/src/store_upgrade/tests.rs::the_server_refuses_an_upgradable_store_and_names_the_command` (existing)
- `MEDULLA-TYPED-PACKS-R012`: `crates/medulla-server-mutation/src/mutation.rs::pack_and_agent_layer_write_methods_are_the_local_only_five` (existing)
- `MEDULLA-TYPED-PACKS-R013`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::tests::the_split_rule_rejects_eg_typos_and_keeps_foreign_claims` (existing)
- `MEDULLA-TYPED-PACKS-R014`: `crates/medulla-h-admin/src/admin/connector_pack/tests/content_model.rs::an_absent_entry_is_withdrawn_and_returns_under_the_same_id` (existing)
- `MEDULLA-TYPED-PACKS-R015`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::an_environment_importer_is_a_bootstrap_default_superseded_by_an_admin_bind` (existing)
- `MEDULLA-TYPED-PACKS-R016`: `crates/medulla-agent-types/src/agent_component.rs::package_version_leaves_the_v4_digest_and_v3_rows_still_validate` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/content_model.rs::a_server_contract_version_change_revises_its_pin` (existing)
- `MEDULLA-TYPED-PACKS-R017`: `crates/medulla-storage/src/owner/agent_library_upgrade/tests.rs::earlier_connector_packless_layout_is_not_upgradeable` (existing)
- `MEDULLA-TYPED-PACKS-R018`: `crates/medulla-catalog-types/src/connector_pack/ops.rs::tests::pack_op_exposes_no_body_read_variant` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::holder_refcounts_transition_across_unchanged_revised_and_withdrawn_entries` (existing)
- `MEDULLA-TYPED-PACKS-R020`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::only_the_bound_importer_may_import` (existing)
- `MEDULLA-TYPED-PACKS-R021`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R022`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R023`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R024`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R025`: `crates/medulla-h-admin/src/admin/connector_pack/import.rs::tests::section_ranges_are_checked_without_wrapping` (existing); `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R026`: `crates/medulla-catalog-types/src/connector_pack/digest.rs::tests::framed_digest_vectors_match` (existing); `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R027`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R028`: `crates/medulla-h-admin/src/admin/connector_pack/import.rs::tests::json_budget_rejects_excess_depth` (existing); `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R029`: `crates/medulla-h-admin/src/admin/connector_pack/import.rs::tests::skill_frontmatter_refuses_yaml_expansion_features` (existing); `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R030`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R031`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R032`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::tests::cost_latency_and_model_facts_are_bounded` (existing); `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R033`: `crates/medulla-rdf/src/pack/tests.rs::triple_budget_boundary_is_inclusive` (existing); `crates/medulla-h-admin/src/admin/connector_pack/import.rs::tests::ontology_imports_are_closed_over_pack_entries` (existing); `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R034`: `crates/medulla-h-admin/src/admin/connector_pack/import.rs::tests::duplicate_named_shapes_can_be_detected_across_files` (existing); `crates/medulla-h-admin/src/admin/connector_pack/import/validation.rs::tests::only_a_constraint_query_service_clause_is_refused` (existing); `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R035`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R036`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing)
- `MEDULLA-TYPED-PACKS-R037`: `crates/medulla-agent-types/src/agent_component.rs::tests::agent_component_draft_validate_accepts_valid_and_refuses_invalid_fields` (existing)
- `MEDULLA-TYPED-PACKS-R038`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::only_the_bound_importer_may_import` (existing)
- `MEDULLA-TYPED-PACKS-R039`: `crates/medulla-h-admin/src/admin/connector_pack/tests/content_model.rs::withdrawing_most_of_a_connector_needs_the_admin_override` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/withdrawal.rs::an_approved_mass_withdrawal_that_moves_every_uri_imports` (existing)
- `MEDULLA-TYPED-PACKS-R040`: `tests/test_connector_pack_planted.py::test_planted_defect_is_refused_by_its_rule` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::a_rejected_import_mutates_nothing_in_the_catalog` (existing)
- `MEDULLA-TYPED-PACKS-R041`: `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::pack_import_refusals_leave_state_unchanged` (existing)
- `MEDULLA-TYPED-PACKS-R041.1`: `crates/medulla-persistence-records/src/connector_pack/undecodable.rs::tests::an_undecodable_stored_pack_row_refuses_with_its_typed_code_and_reason`; `crates/medulla-h-admin/src/admin/connector_pack/tests/undecodable.rs::an_undecodable_stored_pack_refuses_typed_and_a_reimport_replaces_it`
- `MEDULLA-TYPED-PACKS-R041.2`: `crates/medulla-h-admin/src/admin/connector_pack/tests/undecodable.rs::an_undecodable_stored_pack_refuses_typed_and_a_reimport_replaces_it`
- `MEDULLA-TYPED-PACKS-R042.1`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::check_annotations` (existing); `crates/medulla-h-admin/src/admin/connector_pack/import/desired.rs::DesiredBuilder::draft` (existing); `crates/medulla-h-admin/src/admin/connector_pack/import/capability_tests.rs::both_capability_fields_refuse_relative_and_invalid_scheme_claims` (existing)
- `MEDULLA-TYPED-PACKS-R042.2`: `crates/medulla-h-admin/src/admin/connector_pack/import.rs::run_import` (existing); `crates/medulla-h-admin/src/admin/connector_pack/import/desired.rs::DesiredBuilder::draft` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/admin.rs::a_rejected_import_mutates_nothing_in_the_catalog` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/content_model.rs::capability_annotations_remain_declared_after_import` (existing)
- `MEDULLA-TYPED-PACKS-R043`: `crates/medulla-h-admin/src/admin/connector_pack/import/modality_tests.rs::modality_digest_replay_is_canonical_and_direction_sensitive` (existing)
- `MEDULLA-TYPED-PACKS-R044`: `crates/medulla-h-admin/src/admin/connector_pack/import/facts.rs::tests::tool_facts_carry_both_section_digests` (existing); `crates/medulla-catalog-types/src/connector_pack/digest.rs::tests::framed_digest_vectors_match` (existing)
- `MEDULLA-TYPED-PACKS-R045`: `tests/harness_config_tool_facts_served.rs::two_tool_pack_subset_respects_budget` (planned)
- `MEDULLA-TYPED-PACKS-R045.1`: `crates/medulla-catalog-types/src/connector_pack/annotations.rs::tests::new_fields_roundtrip` (existing)
- `MEDULLA-TYPED-PACKS-R045.2`: `crates/medulla-h-admin/src/admin/connector_pack/import/facts.rs::facts` (existing); `crates/medulla-catalog-types/src/connector_pack/annotations.rs::tests::absent_facts_take_safe_defaults` (existing)
- `MEDULLA-TYPED-PACKS-R045.3`: `crates/medulla-catalog-types/src/connector_pack/digest.rs::tests::absent_new_fields_keep_golden_vectors` (existing)
- `MEDULLA-TYPED-PACKS-R045.4`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::tests::unknown_side_effect_is_refused` (existing)
- `MEDULLA-TYPED-PACKS-R045.5`: `crates/medulla-h-admin/src/admin/connector_pack/import/facts.rs::tests::mcp_hint_read_only_is_unproven` (existing)
- `MEDULLA-TYPED-PACKS-R045.6`: `crates/medulla-h-decide/src/decide/stat_tool_subset.rs::request_body` (existing); `crates/medulla-h-decide/src/decide/stat_tests/tool_subset_tests.rs::missing_schema_tokens_abstains` (existing)
- `MEDULLA-TYPED-PACKS-R045.7`: `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::mcp_hint_read_only_premise_is_unproven` (planned)
- `MEDULLA-TYPED-PACKS-R045.8`: `tests/test_connector_pack_client.py::test_harness_tool_facts_bind_the_annotation_digest_like_rust` (existing)
- `MEDULLA-TYPED-PACKS-R045.9`: `tests/harness_config_tool_facts_served.rs::two_tool_pack_subset_respects_budget` (planned)
- `MEDULLA-TYPED-PACKS-R046`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::tests::cost_latency_and_model_facts_are_bounded` (existing)
- `MEDULLA-TYPED-PACKS-R047`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::tests::cost_latency_and_model_facts_are_bounded` (existing)
- `MEDULLA-TYPED-PACKS-R048`: `tests/typed_packs/test_model_harness_facts.py::test_model_profile_facts_round_trip` (planned)
- `MEDULLA-TYPED-PACKS-R048.1`: `crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs::tests::context_window_zero_is_refused_by_the_pack_validator` (existing); `crates/medulla-core/tests/harness_shapes.rs::model_fact_shapes_reject_non_positive_context_window` (existing)
- `MEDULLA-TYPED-PACKS-R048.2`: `crates/medulla-agent-types/src/agent_component/facts.rs::tests::measured_fact_requires_run_ref` (existing)
- `MEDULLA-TYPED-PACKS-R048.3`: `crates/medulla-agent-types/src/agent_component.rs::tests::absent_shell_proficiency_keeps_digest` (existing)
- `MEDULLA-TYPED-PACKS-R048.4`: `crates/medulla-agent-types/src/agent_component.rs::tests::measured_model_facts_change_digest` (existing)
- `MEDULLA-TYPED-PACKS-R048.5`: `tests/typed_packs/test_model_harness_facts.py::test_model_profile_round_trip` (existing)
- `MEDULLA-TYPED-PACKS-R049`: `crates/medulla-catalog-types/src/connector_pack/digest.rs::contract_pin_tests::server_pin_digest_carries_the_declared_sdk_contract_pin_opaquely` (existing)
- `MEDULLA-TYPED-PACKS-R050`: `crates/medulla-h-admin/src/admin/connector_pack/tests/content_model.rs::a_server_contract_version_change_revises_its_pin` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/content_model.rs::a_package_release_with_identical_content_is_unchanged` (existing)
- `MEDULLA-TYPED-PACKS-R051`: `crates/medulla-h-admin/src/graph_schema.rs::handle_gateway` (existing); `crates/medulla-graph-core/src/graph/schema_sources.rs::tests::composed_digest_is_ordered_and_excludes_provenance_time` (existing); `tests/packs/test_schema_attach_served.py::test_attach_detach_and_conflict` (planned)
- `MEDULLA-TYPED-PACKS-R052`: `crates/medulla-h-admin/src/admin/connector_pack/tests/outbox.rs::the_worker_consumes_an_import_and_the_operator_can_rewind_it` (existing)
- `MEDULLA-TYPED-PACKS-R053`: `crates/medulla-h-admin/src/graph_schema/tests.rs::attach_pack_into_a_spare_graph_copies_a_static_snapshot` (existing); `tests/packs/test_schema_attach_served.py::test_static_attach_reports_staleness` (planned)
- `MEDULLA-TYPED-PACKS-R054`: `crates/medulla-transaction/src/tests/outbox.rs::a_consumer_may_not_change_the_topic_its_cursor_already_names` (existing)
- `MEDULLA-TYPED-PACKS-R055`: `crates/medulla-transaction/src/tests/outbox.rs::two_topics_retry_exhaustion_preserves_other_topic_attempts_and_progress` (existing)
- `MEDULLA-TYPED-PACKS-R056`: `crates/medulla-persistence-tests/src/shard_migrate_tests/layout.rs::migration_discovery_rejects_mixed_and_sparse_layouts` (existing); `crates/medulla-persistence-tests/src/redb_backend_tests.rs::normal_startup_rejects_retired_single_file_layout` (existing)
- `MEDULLA-TYPED-PACKS-R057`: `crates/medulla-server-api/src/schema_compose/compiled_tests.rs::attach_detach_and_replace_each_change_the_identity` (existing); `crates/medulla-h-admin/src/graph_schema/tests.rs::attach_list_and_detach_share_one_authoritative_source_set` (existing)
- `MEDULLA-TYPED-PACKS-R058`: `crates/medulla-reasoner/src/owl/budget.rs::tests::exact_budget_completes_and_one_less_is_exhausted` (existing); `tests/packs/test_schema_attach_served.py::test_budget_exhaustion_refuses` (planned)
- `MEDULLA-TYPED-PACKS-R059`: `crates/medulla-h-admin/src/graph_schema/tests.rs::attach_validates_its_own_writes_and_never_scans_existing_data` (existing); `tests/packs/test_schema_attach_served.py::test_attach_does_not_scan_existing_data` (planned)
- `MEDULLA-TYPED-PACKS-R061`: `crates/medulla-graph-core/src/graph/schema_sources.rs::tests::low_level_source_seam_enforces_the_document_bound` (existing)
- `MEDULLA-TYPED-PACKS-R062`: `crates/medulla-raft/src/raft/command/native/catalog.rs::tests::graph_schema_records_replicate_as_graph_state` (existing); `tests/packs/test_schema_attach_served.py::test_schema_replicates` (planned)
- `MEDULLA-TYPED-PACKS-R063`: `crates/medulla-capabilities/tests/contract_registry.rs::schema_bump_enumerates_agent_layer_types` (existing)
- `MEDULLA-TYPED-PACKS-R064`: `crates/medulla-h-admin/src/admin/connector_pack/import/facts.rs::tests::tool_facts_carry_both_section_digests` (existing); `crates/medulla-catalog-types/src/connector_pack/digest.rs::tests::framed_digest_vectors_match` (existing)
- `MEDULLA-TYPED-PACKS-R065`: `crates/medulla-catalog-types/src/connector_pack/digest.rs::contract_pin_tests::server_pin_digest_carries_the_declared_sdk_contract_pin_opaquely` (existing)
- `MEDULLA-TYPED-PACKS-R066.1`: `crates/medulla-core/tests/harness_shapes.rs::model_fact_shapes_reject_missing_run` (existing); `tests/typed_packs/test_model_harness_facts.py::test_shapes_reject_missing_run` (planned)
- `MEDULLA-TYPED-PACKS-R066.2`: `crates/medulla-shacl/tests/swarm_topology_shapes.rs::shell_centric_must_be_boolean` (existing)
- `MEDULLA-TYPED-PACKS-R066.3`: `crates/medulla-agent-types/src/agent_ontology.rs::broader_chain` (existing); `crates/medulla-agent-types/src/agent_ontology.rs::tests::shell_centric_prefers_own_fact_then_inherits_from_task_ancestor` (existing); `crates/medulla-shacl/tests/swarm_topology_shapes.rs::shell_centric_is_single_valued_on_task_classes` (existing)
- `MEDULLA-TYPED-PACKS-R067`: `crates/medulla-capabilities/tests/contract_registry.rs::every_wire_variant_has_exactly_one_descriptor_and_vice_versa` (existing)
- `MEDULLA-TYPED-PACKS-R068`: `crates/medulla-capabilities/src/catalog.rs::tests::the_catalog_digest_is_a_stable_sha256` (existing); `crates/medulla-capabilities/tests/contract_registry.rs::every_wire_variant_has_exactly_one_descriptor_and_vice_versa` (existing)
- `MEDULLA-TYPED-PACKS-R070`: `crates/medulla-catalog-types/src/connector_pack/digest.rs::tests::framed_digest_vectors_match` (existing)
- `MEDULLA-TYPED-PACKS-R071`: `crates/medulla-core/tests/ontology_corpus_parity.rs::generated_modules_match_committed_artifacts` (planned)
- `MEDULLA-TYPED-PACKS-R071.1`: `tests/typed_packs/test_ontology_crosswalk_served.py::test_core_metadata_crosswalks` (planned)
- `MEDULLA-TYPED-PACKS-R071.1.1`: `crates/medulla-catalog-types/src/domain_pack.rs::tests::crosswalk_round_trips_every_field` (planned); `tests/typed_packs/test_ontology_crosswalk_served.py::test_core_metadata_crosswalks` (planned)
- `MEDULLA-TYPED-PACKS-R071.1.2`: `crates/medulla-catalog-types/src/domain_pack/crosswalk.rs::tests::golden_person_event_document_tuples` (planned)
- `MEDULLA-TYPED-PACKS-R071.1.3`: `crates/medulla-h-sparql/src/rdf/inspect_tests.rs::inspection_returns_composed_crosswalks` (planned); the pack carries `crosswalk` and `concept_path` entries covered by `pack_digest`, and the revision is the composed schema digest
- `MEDULLA-TYPED-PACKS-R071.1.4`: `tests/typed_packs/test_ontology_crosswalk_served.py::test_crosswalk_refusals_leave_state_unchanged` (planned)
- `MEDULLA-TYPED-PACKS-R071.1.5`: `tests/typed_packs/test_ontology_crosswalk_served.py::test_crosswalk_evidence_matches_committed_records` (planned)
- `MEDULLA-TYPED-PACKS-R071.1.6`: `tests/typed_packs/test_ontology_crosswalk_served.py::test_core_metadata_crosswalks` (planned)
- `MEDULLA-TYPED-PACKS-R072`: `tests/typed_packs/test_catalog_competency_queries.py::test_fleet_catalog_joins_distinct_authorities` (planned)
- `MEDULLA-TYPED-PACKS-R073`: `crates/medulla-h-admin/src/graph_schema/world_model_tests.rs::world_model_modules_are_core_modules_within_the_catalog_bound` (existing); `crates/medulla-h-admin/src/graph_schema/world_model_tests.rs::every_world_model_class_is_mapped_and_nothing_external_is_imported` (existing)
- `MEDULLA-TYPED-PACKS-R074`: `crates/medulla-h-admin/src/connector_pack_projection/grant.rs::tests::the_projection_actor_writes_only_the_pack_graph_it_was_granted` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests/outbox.rs::failed_projection_records_durable_failed_status` (existing)
- `MEDULLA-TYPED-PACKS-R077`: `crates/medulla-server-ops/src/api/admin/terms.rs::tests::a_paged_walk_returns_every_term_once_in_order_at_one_digest` (existing)
- `MEDULLA-TYPED-PACKS-R078`: `crates/medulla-h-decide/src/decide/stat_tests/retrieval_plan_tests.rs::templates_are_keyed_by_task_class_and_schema_digest` (existing)
- `MEDULLA-TYPED-PACKS-R081`: `tests/test_ontology_package.py::test_api_returns_every_source_file` (existing)
- `MEDULLA-TYPED-PACKS-R082`: `tests/test_rdf_client.py::test_validate_shacl_sends_both_inline_graphs` (existing); `tests/test_graph_schema_vectors.py::test_client_models_match_rust_vectors` (planned)
- `MEDULLA-TYPED-PACKS-R083`: `tests/packs/test_schema_drift_served.py::test_breaking_drift_quarantines_and_activation_needs_approval` (existing)
- `MEDULLA-TYPED-PACKS-R084`: `crates/medulla-h-admin/src/admin/connector_pack/tests/ontology.rs::a_cancelled_import_does_not_block_the_next_bind` (existing); `crates/medulla-h-admin/src/admin/connector_pack/tests.rs::a_cancelled_waiter_leaves_the_pack_lock_queue` (existing)

- **MEDULLA-TYPED-PACKS-R093.1:** `crates/medulla-rdf/src/mapping/parsing.rs::tests::independent_parses_do_not_collide_on_repeated_blank_labels` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R093.2:** `tests/typed_packs/test_reproject_blank_nodes_served.py::test_reused_member_labels_stay_isolated_and_digest_repeats` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R094.2:** `tests/test_prompt_blueprints_served.py::test_lossless_blueprint_export` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R094.3:** `tests/test_prompt_blueprints_served.py::test_modified_blueprint_import` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R094.4:** `tests/test_prompt_blueprints_served.py::test_blueprint_retry_identity` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R094.5:** `tests/test_prompt_blueprints_served.py::test_blueprint_roundtrip_restart` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R094.6:** `tests/test_prompt_blueprints_served.py::test_prompt_proposal_has_no_authority` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R094.7:** `tests/test_prompt_blueprints_served.py::test_approved_source_effect_dependency` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R096:** `crates/medulla-h-admin/src/admin/connector_pack/tests/outbox.rs::restart_replays_projection_once` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R096.1:** `crates/medulla-h-admin/src/admin/connector_pack/tests/outbox.rs::rejected_projection_is_visible_and_retried` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097:** `tests/typed_packs/test_domain_pack_import_served.py::test_each_domain_pack_imports_and_reads_back` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.1:** `crates/medulla-catalog-types/src/domain_pack.rs::tests` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.1:** `tests/typed_packs/test_domain_pack_import_served.py::test_hr_pack_import_matches_source` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.2:** `tests/typed_packs/test_ontology_crosswalk_served.py::test_domain_coverage_and_refusals` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.2.1:** `crates/medulla-catalog-types/src/domain_pack.rs::tests::coverage_round_trips_three_domains` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.2.2:** `crates/medulla-catalog-types/src/domain_pack/crosswalk.rs::tests::coverage_is_order_independent_for_medical_technology_labor` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.2.3:** `crates/medulla-h-sparql/src/rdf/inspect_tests.rs::inspection_reports_pack_revision_coverage` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.2.4:** `tests/typed_packs/test_ontology_crosswalk_served.py::test_refused_domain_import_preserves_heads_and_digests` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.2.5:** `tests/typed_packs/test_ontology_crosswalk_served.py::test_coverage_matches_pack_status_and_schema_reads` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.2.6:** `tests/typed_packs/test_ontology_crosswalk_served.py::test_domain_coverage_and_refusals` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100:** `crates/medulla-capabilities/tests/skill_package_regeneration.rs::list_after_reingest_returns_new_digest` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100.2:** `crates/medulla-capabilities/tests/skill_package_regeneration.rs::render_reflects_latest_ingest` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100.3:** `crates/medulla-capabilities/tests/skill_package_regeneration.rs::catalog_listing_renders_current_content` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100.4:** `crates/medulla-capabilities/tests/skill_package_regeneration.rs::dangling_reference_is_refused` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100.5:** `crates/medulla-capabilities/tests/skill_package_regeneration.rs::render_carries_revision_and_digest` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100.6:** `crates/medulla-capabilities/tests/skill_package_regeneration.rs::list_after_reingest_returns_new_digest` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R097.2.7:** `tests/typed_packs/test_external_schema_import_served.py::test_mapped_and_unmapped_terms_of_every_kind` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100.100:** `tests/typed_packs/test_skill_lossless_edit_served.py::test_frontmatter_and_step_edits_preserve_unmodified_bytes` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100.101:** `tests/typed_packs/test_skill_compatibility_admission_served.py::test_breaking_upgrade_requires_exact_consumer_migration` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R001.100:** `tests/typed_packs/test_connector_nested_schema_served.py::test_nested_schema_evolution_preserves_digest` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R002.12:** `crates/medulla-h-admin/src/admin/connector_pack/tests/outbox.rs::native_component_event_projects` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R002.13:** `crates/medulla-h-admin/src/admin/prompt_receipt.rs::tests::receipt_verification` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R002.13:** `tests/test_prompt_blueprints_served.py::test_receipt_verdicts` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R071.2:** `crates/medulla-core/tests/ontology_modules_extend_core.rs::no_module_imports_core_and_every_class_reaches_core` (planned). Proves the row at its served entry point; no receipt is asserted. It also asserts every module class has an `rdfs:subClassOf` path to a core class (the 208 formerly unrooted classes included).
- **MEDULLA-TYPED-PACKS-R071.2:** `tests/typed_packs/test_ontology_corpus_served.py::test_modules_extend_core_through_subclass` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R071.100:** `tests/typed_packs/test_concept_path_admission_served.py::test_duplicate_or_conflicting_paths_refuse_without_iri_changes` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R101:** `crates/medulla-shacl/tests/connector_pack_shapes.rs::each_contract_shape_accepts_valid_and_refuses_invalid` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R101.2:** `crates/medulla-shacl/tests/connector_pack_shapes.rs::each_contract_shape_accepts_valid_and_refuses_invalid` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R102:** `tests/typed_packs/test_connector_surface_rdf_served.py::test_surface_query_follows_pack_head` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R102.2:** `crates/medulla-h-admin/src/connector_pack_projection/surface_tests.rs::surface_nodes_follow_pack_head` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R102.3:** `crates/medulla-h-admin/src/admin/connector_pack/import/contract_tests.rs::core_class_redeclaration_is_refused` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R102.4:** `crates/medulla-h-admin/src/connector_pack_projection/surface_tests.rs::outbox_consumer_writes_surface_nodes` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R102.5:** `crates/medulla-h-admin/src/admin/connector_pack/import/contract_tests.rs::surface_refusals_leave_state_unchanged` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R102.6:** `crates/medulla-h-admin/src/connector_pack_projection/surface_tests.rs::surface_nodes_carry_pack_digest` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R102.7:** `tests/typed_packs/test_connector_surface_rdf_served.py::test_surface_query_follows_pack_head` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R103:** `tests/typed_packs/test_runtime_ontology_import_served.py::test_import_revise_withdraw_end_to_end` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R103.2:** `crates/medulla-h-admin/src/graph_schema/ontology_import_tests.rs::import_is_idempotent_by_digest_and_revises` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R103.3:** `crates/medulla-h-admin/src/graph_schema/ontology_import_tests.rs::gateway_routes_ontology_import` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R103.3:** `tests/test_graph_schema_vectors.py::test_import_ontology_vector` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R103.4:** `crates/medulla-h-admin/src/graph_schema/ontology_import_tests.rs::ontology_import_refusals_leave_schema_unchanged` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R103.5:** `crates/medulla-h-admin/src/graph_schema/ontology_import_tests.rs::revision_records_digest_importer_and_classes` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R103.6:** `tests/typed_packs/test_runtime_ontology_import_served.py::test_import_revise_withdraw_end_to_end` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R104:** `tests/connector_pack/test_seed_graph_served.py::test_seed_idempotent_and_partial_reimport` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R104.3:** `crates/medulla-h-admin/src/admin/connector_pack/seed_tests.rs::served_import_projects_seed` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R104.4:** `crates/medulla-h-admin/src/admin/connector_pack/seed_tests.rs::seed_refusals_write_nothing` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R104.5:** `crates/medulla-h-admin/src/admin/connector_pack/seed_tests.rs::receipt_names_part_outcomes` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R104.6:** `tests/connector_pack/test_seed_graph_served.py::test_seed_idempotent_and_partial_reimport` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R105:** `tests/typed_packs/test_connector_ontology_core_link_served.py::test_linked_pack_reads_through_core_class_and_orphan_refuses` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R105.1:** `tests/typed_packs/test_connector_ontology_core_link_served.py::test_linked_pack_reads_through_core_class_and_orphan_refuses` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R650:** `tests/typed_packs/test_object_model_served.py::test_pack_value_link_interface_declarations_are_enforced_on_write` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R650.1:** `crates/medulla-core/tests/object_model.rs::declarations_round_trip_and_reject_zero_max_card` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R650.2:** `crates/medulla-core/tests/object_model.rs::compile_is_deterministic_and_maps_each_constraint` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R650.3:** `tests/typed_packs/test_object_model_served.py::test_import_compiles_shapes_and_graph_schema_classes_reports_interfaces` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R650.4:** `tests/typed_packs/test_object_model_served.py::test_value_link_and_interface_violations_refuse_with_unchanged_state` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R650.5:** `tests/typed_packs/test_object_model_served.py::test_refusal_carries_rule_path_and_value_digest` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R650.6:** `tests/typed_packs/test_object_model_served.py::test_pack_value_link_interface_declarations_are_enforced_on_write` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R651:** `tests/typed_packs/test_ontology_edits_served.py::test_import_edit_chain_revert_and_sealed_refusal` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R651.1:** `crates/medulla-types/tests/ontology_edit.rs::edit_and_finding_round_trip` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R651.2:** `crates/medulla-core/tests/ontology_edits.rs::chain_links_invert_is_exact_and_revert_replays_newest_first` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R651.3:** `tests/typed_packs/test_ontology_edits_served.py::test_schema_ops_append_edits_and_revert_edit_inverts` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R651.4:** `tests/typed_packs/test_ontology_edits_served.py::test_broken_chain_sealed_revert_and_naming_refuse` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R651.5:** `tests/typed_packs/test_ontology_edits_served.py::test_head_inversions_and_findings_read_back` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R651.6:** `tests/typed_packs/test_ontology_edits_served.py::test_import_edit_chain_revert_and_sealed_refusal` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R652:** `tests/typed_packs/test_classification_served.py::test_claim_propose_propagate_promote_and_redacted_read` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R652.1:** `crates/medulla-types/tests/classification.rs::claim_round_trips_and_rejects_empty_categories` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R652.2:** `crates/medulla-isolation/tests/marking_propagation.rs::closure_is_monotone_bounded_and_deterministic` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R652.3:** `tests/typed_packs/test_classification_served.py::test_methods_reach_handler_and_promotion_stamps_marking` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R652.4:** `tests/typed_packs/test_classification_served.py::test_missing_evidence_illegal_state_and_unknown_marking_refuse` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R652.5:** `tests/typed_packs/test_classification_served.py::test_marking_sets_and_promoter_read_back` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R652.6:** `tests/typed_packs/test_classification_served.py::test_claim_propose_propagate_promote_and_redacted_read` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R700:** `tests/packs/test_skill_graph_pack_served.py::test_bad_hash_refused_and_conformant_pack_imports` (planned). Proves the row at its served entry point; no receipt is asserted.
- **MEDULLA-TYPED-PACKS-R100.1:** `crates/medulla-capabilities/tests/skill_package_regeneration.rs::rendered_package_round_trip` (existing test; add the `// spec: MEDULLA-TYPED-PACKS-R100.1` tag above it, and extend it to assert the new `schema_version` field is 1).

- `MEDULLA-TYPED-PACKS-R700.5`: `tests/packs/test_skill_graph_pack_served.py::test_receipt_lists_every_checked_rule`
- `MEDULLA-TYPED-PACKS-R700.6`: `tests/packs/test_skill_graph_pack_served.py::test_bad_hash_refused_and_conformant_pack_imports`
- `MEDULLA-TYPED-PACKS-R097.700`: `crates/medulla-core/src/domain_pack/mapping.rs::tests::mapping_cases_evaluate_to_expected_entities`
- `MEDULLA-TYPED-PACKS-R701`: `tests/packs/test_skill_graph_pack_served.py::test_layered_graph_imports_and_orphan_layer_refuses`
- `MEDULLA-TYPED-PACKS-R097.701`: `tests/packs/test_domain_pack_run_served.py::test_preview_then_run_promotes_only_through_envelope`
- `MEDULLA-TYPED-PACKS-R710`: `crates/medulla-catalog-types/src/graph_schema/drift.rs::tests::a_missing_required_field_or_a_retyped_field_is_breaking`, `::a_quarantined_drift_becomes_the_repair_proposal_the_approval_binds`
- `MEDULLA-TYPED-PACKS-R712`: `crates/medulla-catalog-types/src/graph_schema/drift.rs::tests::an_undeclared_policy_is_quarantine_and_quarantine_never_applies`, `tests/packs/test_schema_drift_served.py::test_breaking_drift_quarantines_and_activation_needs_approval`
- `MEDULLA-TYPED-PACKS-R714`: `tests/packs/test_schema_drift_served.py::test_delivery_reports_drift_class`
- `MEDULLA-TYPED-PACKS-R715`: `tests/packs/test_schema_drift_served.py::test_breaking_delivery_is_quarantined`
- `MEDULLA-TYPED-PACKS-R801`: `tests/typed_packs/test_glossary_served.py::test_import_scheme_bind_term_and_review_lifecycle`
- `MEDULLA-TYPED-PACKS-R802`: `tests/typed_packs/test_stewardship_served.py::test_change_request_flow`
- `MEDULLA-TYPED-PACKS-R803`: `crates/medulla-h-jobs/src/jobs/sensitive_scan_served_tests.rs::profile_job_proposes_claim_without_values`
- `MEDULLA-TYPED-PACKS-R804`: `crates/medulla-h-admin/src/graph_schema/ontology_diff_tests.rs::import_reports_impact_and_opens_change_request`
- `MEDULLA-TYPED-PACKS-R805`: `crates/medulla-h-admin/src/catalog/data_product_served_tests.rs::request_grants_access_after_steward_approval`
- `MEDULLA-TYPED-PACKS-R106`: `tests/typed_packs/test_connector_catalog_served.py::test_catalog_search_finds_assets_with_owner_term_and_lineage`
- `MEDULLA-TYPED-PACKS-R107`: `tests/typed_packs/test_pack_contract_served.py::test_breaking_release_waits_for_approval_then_serves`
- `MEDULLA-TYPED-PACKS-R108`: `tests/typed_packs/test_catalog_assets_served.py::test_one_query_returns_agents_business_workflows_graphs_and_connector_assets`
- `MEDULLA-TYPED-PACKS-R109`: `tests/typed_packs/test_dashboard_asset_served.py::test_dashboard_save_conflict_share_lineage_search_and_soft_delete`
- `MEDULLA-TYPED-PACKS-R085.7.1`: `crates/medulla-identity-types/src/identity/tests/session_evidence.rs::original_reference_inspection_is_owner_bound_and_does_not_touch`, `::reference_inspection_refuses_pending_disabled_and_ambiguous_records`
- `MEDULLA-TYPED-PACKS-R085.4.1`: `crates/medulla-identity-types/src/identity/tests/session_evidence.rs::renewal_preserves_original_scope_ceiling_and_refuses_legacy_evidence`, `::renewal_clamps_idle_to_original_absolute_expiry_and_preserves_subject_mfa`, `::renewal_refuses_invalid_authority_without_mutation`
- `MEDULLA-TYPED-PACKS-R085.4.3`: `crates/medulla-server-router/src/dispatch/dispatch_e2e_tests/identity/resolution_context.rs::reference_renewal_rechecks_boundary_and_global_revision`
- `MEDULLA-TYPED-PACKS-R085.5.1`: `crates/medulla-identity-types/src/identity/tests/approval_keys.rs::lifecycle_persists_public_facts_and_enforces_rotation_and_revocation_cas`, `::registration_message_has_exact_domain_lengths_and_revision_binding`
- `MEDULLA-TYPED-PACKS-R085.6.1`: `crates/medulla-identity-types/src/identity/tests/session_evidence.rs::credential_observation_preserves_state_and_existing_refusals`, `::omitted_refresh_preserves_legacy_touch_and_authority`
- `MEDULLA-TYPED-PACKS-R094.8`: `tests/test_prompt_blueprints_served.py::test_source_effect_receipt_is_recorded_once`
- `MEDULLA-TYPED-PACKS-R097.2.7.1`: `crates/medulla-types/src/external_schema.rs::tests::mapping_and_coverage_round_trip`
- `MEDULLA-TYPED-PACKS-R097.2.7.2`: `crates/medulla-types/src/external_schema.rs::tests::reordered_inputs_give_identical_digest`
- `MEDULLA-TYPED-PACKS-R097.2.7.3`: `crates/medulla-h-admin/src/admin/connector_pack/tests/external_schema.rs::refusals_leave_head_and_schema_unchanged`
- `MEDULLA-TYPED-PACKS-R097.2.7.4`: `tests/typed_packs/test_external_schema_import_served.py::test_mapped_and_unmapped_terms_of_every_kind`
- `MEDULLA-TYPED-PACKS-R803.7`: `crates/medulla-server-graph/src/dispatch/graph_pipeline/web_ingest_tests.rs::enrich_proposes_claim_for_sensitive_document`
- `MEDULLA-TYPED-PACKS-R806`: `tests/packs/test_schema_drift_methods_served.py::test_propose_then_approve_activates_once`
- `MEDULLA-TYPED-PACKS-R807`: `tests/packs/test_conformance_evidence_served.py::test_evidence_written_read_revoked_and_tenant_scoped`
- `MEDULLA-TYPED-PACKS-R808`: `tests/graph_schema_digest_served.rs::mismatch_and_drift_are_refused_and_digest_reads_back`
- `MEDULLA-TYPED-PACKS-R809`: `tests/ontology/test_canonical_hash_served.py::test_reordered_serializations_hash_equal`
- `MEDULLA-TYPED-PACKS-R110`: `tests/open_metadata_sync_served.rs::metadata_sync_round_trips_terms_assets_and_lineage`
- `MEDULLA-TYPED-PACKS-R110.1`: `crates/medulla-rdf/tests/open_metadata.rs::mapped_types_resolve_one_class`
- `MEDULLA-TYPED-PACKS-R110.2`: `crates/medulla-catalog-types/src/open_metadata.rs::tests::record_round_trips_and_refuses_unknown_fields`
- `MEDULLA-TYPED-PACKS-R110.3`: `crates/medulla-catalog-types/src/open_metadata/tests.rs::inbound_maps_term_asset_and_lineage`
- `MEDULLA-TYPED-PACKS-R110.4`: `crates/medulla-catalog-types/src/open_metadata/tests.rs::outbound_carries_expected_version`
- `MEDULLA-TYPED-PACKS-R110.5`: `tests/open_metadata_sync_served.rs::sync_reads_terms_and_writes_catalog_nodes`
- `MEDULLA-TYPED-PACKS-R110.6`: `tests/open_metadata_sync_served.rs::unmapped_types_are_listed_and_skipped`
- `MEDULLA-TYPED-PACKS-R110.7`: `tests/open_metadata_sync_served.rs::nodes_keep_guid_version_and_source`
- `MEDULLA-TYPED-PACKS-R110.8`: `tests/open_metadata_sync_served.rs::concurrent_term_edit_is_a_conflict_and_catalog_wins`
- `MEDULLA-TYPED-PACKS-R110.9`: `tests/competency/test_open_metadata_sync_served.py::test_metadata_sync_then_write_back`
