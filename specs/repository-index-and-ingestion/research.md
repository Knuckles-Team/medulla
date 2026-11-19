# Research: repository index and ingestion

## R002.651.2 supersession: how it is composed and verified

- Question: how is a supersession composed from existing methods and verified by the engine?
- Decision: the envelope operation `Supersede{old_fact, new_fact}` rides the existing `Method::ApplyChangeEnvelope` path. The apply path of `apply_one_change_envelope` writes the `supersedes` edge and marks the old fact `superseded` in one transaction, so the edge and the mark never part. No new served method exists.
- Verification: the engine checks that the old fact exists before it writes and refuses a missing one as `SUPERSESSION_UNKNOWN_FACT`. A read at an earlier time still returns the old fact through the bitemporal stamps of `MEDULLA-DURABLE-KERNEL-R078`.
- Alternative refused: a second writer for supersession. One writer commits every observation through the mutation gateway.

## R002.651.3 dead letter: where a redrive requeues its items

- Question: given which queue methods are built, where does a dead-letter redrive requeue its items?
- Decision: the ingest worker ack path of `MEDULLA-INGEST-WORKER-R001.9` parks an item after `max_attempts` as a `:DeadLetterItem` node, written through the mutation gateway. `DeadLetterRedrive{item_ids}` requeues the named items to the queue each item records and deletes the nodes in one transaction. A named item that is not parked refuses as `DEAD_LETTER_UNKNOWN_ITEM`.
- Bound: `SchedulerConfig.dead_letter_max_items` caps the parked set, default 100000, range [1000, 10000000].
- Alternative refused: a separate dead-letter store. The parked item is graph state and needs no second store.
