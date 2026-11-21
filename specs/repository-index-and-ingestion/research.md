# Research: repository index and ingestion

## R002.651.2 supersession: how it is composed and verified

- Question: how is a supersession composed from existing methods and verified by the engine?
- Decision: no new operation. The producer composes a supersession with `supersession_methods` as an `AddEdge` from new to old fact (`relationship: "supersedes"`, `envelope_id`) paired with a `CompareAndSetNodeFields` on the old fact (`superseded`, `superseded_by`, `superseded_in`) inside one `ApplyChangeEnvelope` batch, so the edge and the mark commit in one transaction.
- Verification: `check_supersessions` runs in `apply_one_change_envelope_inner` under the write lock, refuses a missing old fact as `SUPERSESSION_UNKNOWN_FACT` and an edge or mark without its pair as `SUPERSESSION_UNPAIRED`. A read at an earlier time still returns the old fact through the bitemporal stamps of `MEDULLA-DURABLE-KERNEL-R078`.
- Alternative refused: a second writer for supersession. One writer commits every observation through the mutation gateway.

## R002.651.3 dead letter: where a redrive requeues its items

- Question: given which queue methods are built, where does a dead-letter redrive requeue its items?
- Decision: dead letters are rows of the jobs-store table `TASK_DEAD_LETTERS`, written by the ingest ack path of `MEDULLA-INGEST-WORKER-R001.9` in the same commit as the failed transition. `DeadLetterRedrive{item_ids}` resets each named job to `Submitted` and deletes its row in one jobs.redb transaction; a named item that is not parked refuses the whole call as `DEAD_LETTER_UNKNOWN_ITEM`.
- Alternative refused: a separate dead-letter store. The parked item is graph state and needs no second store.
