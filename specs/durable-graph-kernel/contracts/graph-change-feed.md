# Seam: committed graph change feed

Provider: medulla MEDULLA-DURABLE-KERNEL-R043.100
Consumer: arbora ARBORA-OPS-R062

## Wire shape

Arbora calls `OpenGraphSnapshot` once and then pages `ReadGraphChanges` with the opaque cursor. The snapshot carries `incarnation`, `committed_revision`, `snapshot_digest`, `visibility_revision` and the exact `high_water_cursor`. Each event carries `commit_id`, `event_ordinal`, the commit and visible timestamps and the change. Event identity is `(commit_id, event_ordinal)`; order is `(graph_incarnation, committed_revision, event_ordinal)`. Page size is 1 to 256 and retention is 24 hours. Delivery is at-least-once with stable ids. Arbora supplies the verified principal and tenant. The cursor binds tenant, graph, incarnation and authorization revision.

## Refusal codes

`CURSOR_EXPIRED` (with a safe current watermark), `AUTHORIZATION_CHANGED`, `GRAPH_INCARNATION_STALE` and `ACCESS_DENIED`. The feed is read-only and downstream of `ApplyChangeEnvelope`; it never writes.

## Authoritative types

`crates/medulla-types/src/graph_change_feed.rs` (`MEDULLA-DURABLE-KERNEL-R043.100.1`). The methods are variants of `crates/medulla-protocol-types/src/protocol/method/`. The Python client methods are `open_graph_snapshot` and `read_graph_changes` (`MEDULLA-DURABLE-KERNEL-R043.100.4`).

## Test vectors

`tests/graph_change_feed_served.rs` holds the served vectors for ordering, deduplication and recovery. `tests/test_graph_change_feed_client.py` holds the client vectors that arbora replays.
