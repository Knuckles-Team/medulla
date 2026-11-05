# MEDULLA-MEDIA-VOCAB — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Unit | load media-v1.ttl and content-v1.ttl and their shapes with the native parser | classes resolve, `:Transcript` is not redefined, SHACL pass and fail fixtures behave | `crates/medulla-core/tests/media_vocab_a.rs` |
| T-002 | FR-002 | Contract | each connector mapping fixture and recorded response | every row names an existing class or property and `vg:sourceKind` is mcp | `crates/medulla-core/tests/media_vocab_a.rs` |
| T-003 | FR-003 | Integration | recorded arr, qbittorrent and jellyfin responses joined in one graph | films downloaded but not in the library, provenance per leg | `crates/medulla-rdf/tests/media_vocab_cross_source.rs` |
| T-004 | FR-004 | Negative | connector fixture lacking the operation | typed abstention naming the operation and its owner | `crates/medulla-core/tests/media_vocab_a.rs` |
| T-005 | FR-005 | Served | transcript revisions, summary context and summary commit through the served methods | sealed digest read back, bounded context, receipt and citation checks | PENDING |
| T-006 | FR-006 | Served | bookmark ingest, search and relations on one snapshot | one binding, no denied edge or count leaks | PENDING |
| T-007 | FR-007 | Served | media tags and paper records through `SourceIngest`, tag search through `SemanticSearch` | authorized tags only, receipt-bound paper facts, refusals leave the store unchanged | PENDING |
| T-008 | FR-008 | Served | media memory attachments, vocabulary terms and food-pack import | blob stored once, recall by caption, terms declared | PENDING |

## Planned tests per row

Every row below names a test that does not exist yet. The test file and function are fixed by the row; no case has run.

| Test ID | Row | Level | Planned test | Expected observation |
|---|---|---|---|---|
| T-009 | MEDULLA-MEDIA-VOCAB-R001.100.1 | L2 | `crates/medulla-core/tests/media_vocab_a.rs::media_tag_terms_and_shape` | Tag vocabulary proves its row as stated in requirements.md |
| T-010 | MEDULLA-MEDIA-VOCAB-R001.100.2 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_media_tag_tests.rs::tag_ingest_refusals_and_replay` | Tag ingest mapping proves its row as stated in requirements.md |
| T-011 | MEDULLA-MEDIA-VOCAB-R001.100.3 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_media_tag_tests.rs::authorized_search_returns_only_visible_tags` | Authorized search and served proof proves its row as stated in requirements.md |
| T-012 | MEDULLA-MEDIA-VOCAB-R001.101.1 | L1 | `crates/medulla-types/tests/meeting_transcript.rs::records_round_trip` | Model proves its row as stated in requirements.md |
| T-013 | MEDULLA-MEDIA-VOCAB-R001.101.2 | L2 | `crates/medulla-core/tests/meeting_transcript.rs::append_correct_finalize_and_conflicts` | Revision behavior proves its row as stated in requirements.md |
| T-014 | MEDULLA-MEDIA-VOCAB-R001.101.3 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_meeting_transcript_tests.rs::served_append_correct_finalize` | Revision wiring proves its row as stated in requirements.md |
| T-015 | MEDULLA-MEDIA-VOCAB-R001.101.4 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_meeting_transcript_tests.rs::context_is_bounded_and_refuses_before_write` | Summary context proves its row as stated in requirements.md |
| T-016 | MEDULLA-MEDIA-VOCAB-R001.101.5 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_meeting_transcript_tests.rs::commit_validates_receipt_and_citations` | Summary commit proves its row as stated in requirements.md |
| T-017 | MEDULLA-MEDIA-VOCAB-R001.101.6 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_meeting_transcript_tests.rs::transcript_finalize_context_commit_round_trip` | Refusals, evidence and served proof proves its row as stated in requirements.md |
| T-018 | MEDULLA-MEDIA-VOCAB-R001.102.1 | L1 | `crates/medulla-types/tests/bookmark_query.rs::records_round_trip_and_reject_bad_limits` | Model proves its row as stated in requirements.md |
| T-019 | MEDULLA-MEDIA-VOCAB-R001.102.2 | L3 | `crates/medulla-server-served-tests/src/split_tests/bookmark_served_tests.rs::search_pages_with_bound_cursor_and_unknown_metadata` | Search proves its row as stated in requirements.md |
| T-020 | MEDULLA-MEDIA-VOCAB-R001.102.3 | L3 | `crates/medulla-server-served-tests/src/split_tests/bookmark_served_tests.rs::relations_hide_denied_edges_and_counts` | Relations proves its row as stated in requirements.md |
| T-021 | MEDULLA-MEDIA-VOCAB-R001.102.4 | L3 | `crates/medulla-server-served-tests/src/split_tests/bookmark_served_tests.rs::ingest_search_and_relations_share_one_snapshot` | Refusals, evidence and served proof proves its row as stated in requirements.md |
| T-022 | MEDULLA-MEDIA-VOCAB-R002.5.1 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_paper_tests.rs::mixed_result_refuses_only_the_unidentified_paper` | Identity and receipt checks proves its row as stated in requirements.md |
| T-023 | MEDULLA-MEDIA-VOCAB-R002.5.2 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_paper_tests.rs::receipt_bound_paper_facts_round_trip` | Mapping and provenance proves its row as stated in requirements.md |
| T-024 | MEDULLA-MEDIA-VOCAB-R002.5.3 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_paper_tests.rs::count_only_record_creates_no_edge` | Citation edges proves its row as stated in requirements.md |
| T-025 | MEDULLA-MEDIA-VOCAB-R002.5.4 | L2 | `crates/medulla-core/tests/media_vocab_a.rs::paper_classes_and_cites_reconciled` | Vocabulary reconciliation proves its row as stated in requirements.md |
| T-026 | MEDULLA-MEDIA-VOCAB-R002.5.5 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_paper_tests.rs::paper_refusals_leave_store_unchanged` | Refusals and served proof proves its row as stated in requirements.md |
| T-027 | MEDULLA-MEDIA-VOCAB-R028 | L2 | `crates/medulla-core/tests/media_vocab_a.rs::media_vocab_audio_transcriber_single` | Cross-repo, owner audio-transcriber: transcript list/get operation by recording id and date proves its row as stated in requirements.md |
| T-028 | MEDULLA-MEDIA-VOCAB-R029 | L3 | `tests/test_kg_ingest.py::test_segment_carries_avg_logprob` | Cross-repo, owner audio-transcriber: expose avg_logprob (confidence) per segment proves its row as stated in requirements.md |
| T-029 | MEDULLA-MEDIA-VOCAB-R030 | L3 | `tests/test_sx_search_citations.py::test_citations_returns_citing_papers` | Cross-repo, owner scholarx: citing-papers operation: add sx_search action 'citations' returning papers that cite a given id proves its row as stated in requirements.md |
| T-030 | MEDULLA-MEDIA-VOCAB-R031 | L3 | `tests/test_kg_ingest.py::test_unresponsive_engines_pass_through` | Cross-repo, owner searxng-mcp: pass through per-engine result counts and unresponsive_engines from the SearXNG response proves its row as stated in requirements.md |
| T-031 | MEDULLA-MEDIA-VOCAB-R032 | L3 | `tests/test_ontology.py::test_no_duplicate_iri_with_core` | Cross-repo, owner arr-mcp: rename connector :Episode to mv:TelevisionEpisode subclass (collision with core :Episode) proves its row as stated in requirements.md |
| T-032 | MEDULLA-MEDIA-VOCAB-R033 | L3 | `tests/test_ontology.py::test_no_duplicate_iri_with_core` | Cross-repo, owner jellyfin-mcp and audiobookshelf-mcp: stop declaring :Author in the shared namespace; use mv:contributor to :Person proves its row as stated in requirements.md |
| T-033 | MEDULLA-MEDIA-VOCAB-R650.1 | L1 | `crates/medulla-work-types/src/memory.rs::tests::media_attachment_round_trips` | Model proves its row as stated in requirements.md |
| T-034 | MEDULLA-MEDIA-VOCAB-R650.2 | L2 | `crates/medulla-modality/tests/media_memory.rs::blob_written_once_and_recall_is_deterministic` | Behavior proves its row as stated in requirements.md |
| T-035 | MEDULLA-MEDIA-VOCAB-R650.3 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_graph_memory_media_tests.rs::media_attachment_remember_and_recall` | Wiring proves its row as stated in requirements.md |
| T-036 | MEDULLA-MEDIA-VOCAB-R650.4 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_graph_memory_media_tests.rs::media_refusals` | Refusal proves its row as stated in requirements.md |
| T-037 | MEDULLA-MEDIA-VOCAB-R650.5 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_graph_memory_media_tests.rs::media_evidence_reads_back` | Evidence proves its row as stated in requirements.md |
| T-038 | MEDULLA-MEDIA-VOCAB-R650.6 | nightly | `crates/medulla-modality/benches/media_memory_write.rs::media_memory_write` | Hot-path proof proves its row as stated in requirements.md |
| T-039 | MEDULLA-MEDIA-VOCAB-R651 | L1 | `crates/medulla-core/tests/media_vocab_b.rs::engagement_metric_value_declared` | Engagement metric name and value proves its row as stated in requirements.md |
| T-040 | MEDULLA-MEDIA-VOCAB-R652 | L1 | `crates/medulla-core/tests/media_vocab_b.rs::announces_stream_declared` | announcesStream link proves its row as stated in requirements.md |
| T-041 | MEDULLA-MEDIA-VOCAB-R653 | L1 | `crates/medulla-core/tests/connector_competency_ontology_terms.rs::ontology_audit_personal_v1::totalAmount_covers_purchase_invoice` | totalAmount covers purchase invoices proves its row as stated in requirements.md |
| T-042 | MEDULLA-MEDIA-VOCAB-R654 | L3 | `crates/medulla-server-served-tests/src/split_tests/medulla_h_ingest_transcript_clock_tests.rs::segments_carry_three_typed_clocks` | Transcript segments carry storage, event and narrative time proves its row as stated in requirements.md |
| T-043 | MEDULLA-MEDIA-VOCAB-R655 | L2 | `crates/medulla-core/tests/nutrition_pack.rs::nutrient_ids_map_and_records_conform` | Food composition and nutrient reference types proves its row as stated in requirements.md |

## Negative and boundary cases

Missing leg (a source unconnected): typed abstention naming the operation and its owner. Raw email address in a subscriber: SHACL violation. Hash case mismatch: normalised to lower case. Distinct queries returning the same URL keep distinct search result identities. Two posts linking the same article remain separate. Repeated same-source records stay idempotent. Equal numeric provider ids for a film and a television series do not merge. Cross-tenant requests to the transcript, bookmark, tag, paper and memory surfaces refuse before any write or count. A refusal never changes the store.

## Quality and release proof

Focused: `cargo test -p medulla-core --test media_vocab_a`, `cargo test -p medulla-core --test media_vocab_b` and `cargo test -p medulla-rdf --test media_vocab_shapes --test media_vocab_cross_source`. Gates: the repository complexity, duplication, dead-code and KISS gates and the spec gate set. Record revision, environment, output link and pass or fail in the acceptance record; a source test is not served acceptance.
