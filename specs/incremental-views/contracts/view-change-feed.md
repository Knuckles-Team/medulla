# Seam contract: view change feed

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-INCREMENTAL-VIEWS-R010.4
Consumer: arbora ARBORA-OPS-R018

## Provider

Repository: medulla. Rows: `MEDULLA-INCREMENTAL-VIEWS-R010.1`, `MEDULLA-INCREMENTAL-VIEWS-R010.3`, `MEDULLA-INCREMENTAL-VIEWS-R010.4`, and `MEDULLA-INCREMENTAL-VIEWS-R005.3` for the status table.

## Consumer

Repository: arbora, row `ARBORA-OPS-R018`. It serves every engine method that has no curated operation as the generated operation `eg.<service>.<Method>`. It serves `CdcRead` that way and `Sql` as `query.sql`. Arbora adds no handler for this seam.

## Wire shape

| Method | Request | Answer |
|---|---|---|
| `CdcRead` | `{graph: "view:<name>", from_seq, limit}` | `CdcReadResult {events, gap, watermark, head_seq, epoch}` |
| `Sql` | `SELECT … FROM medulla_catalog.matviews` | rows of the ten status columns |

Each event of a view feed has `kind` `InsertRow` or `DeleteRow` (MEDULLA-ENGINE-PARITY-R015.1), `table` set to the view name, `node_id` set to the result row key and `after` or `before` set to the row as a MessagePack map. Folding inserts as `+1` and deletes as `-1` gives the view result.

## Refusal codes

- `MATVIEW_UNKNOWN`: the view name has no definition.
- `ACCESS_DENIED`: the caller may not read the graph of the view.
- A cursor below the retained ring answers `gap: true` with the current `watermark`; the consumer reads a snapshot and resumes.

## Authoritative types

- `CdcEvent` and `CdcKind`: `crates/medulla-types/src/wire_streaming.rs` in medulla.
- `ViewStatus`: `crates/medulla-core/src/view_status.rs` in medulla.

## Test vectors

- `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::view_delta_feed_matches_result_difference` in medulla.
- `crates/medulla-server-served-tests/src/split_tests/incremental_views.rs::view_feed_gap_then_snapshot_resume` in medulla.
