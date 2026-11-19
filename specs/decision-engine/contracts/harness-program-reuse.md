# Seam: harness program reuse

Provider: medulla MEDULLA-DECISION-ENGINE-R135.16, MEDULLA-DECISION-ENGINE-R135.17
Consumer: meristem MERISTEM-HARNESS-CYCLE-R022 (children .4 to .8)

## Wire shape

Before its first model request, the harness sends a `program_lookup` question through arbora. The question carries `task_signature`, `request_text` and `program_graph`. Medulla answers one option `program_match:<json>` or abstains. The JSON is a `ProgramMatch`: task signature, program digest, match kind, score and threshold in millionths, matched node, bindings and bound tool steps. Each bound step names a server, a tool and its scalar arguments. The harness runs the steps in order and never re-binds or reranks them. After the run, the harness reports one `program_replay` run record: `hit`, `fallback` or `abstain`.

## Refusal codes

Abstentions: `NO_PROGRAM`, `PROGRAM_BELOW_THRESHOLD`, `PROGRAM_SLOTS_UNBOUND`, `PROGRAM_NOT_EXECUTABLE`, `PROGRAM_EMBEDDING_UNAVAILABLE` and `PROGRAM_DEMOTED`. Request refusals: `REQUEST_TEXT_TOO_LONG`, `PROGRAM_GRAPH_REQUIRED`, `PROGRAM_CANDIDATES_RESERVED` and `REPLAY_RECORD_INVALID`.

## Authoritative types

`ProgramMatch` in `crates/medulla-decision-types/src/decision/program_store/semantic.rs` and `ProgramReplay` in `crates/medulla-decision-types/src/decision/program_store/replay.rs`.

## Test vectors

The served tests named by R135.16.6 and R135.17.3 are the vectors. The consumer replays their recorded answers.
