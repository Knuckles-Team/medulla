# MEDULLA-INCREMENTAL-VIEWS — Quickstart

Governing spec: [spec.md](spec.md).

## Plan view, served

1. Define a plan view with `PlanMatViewDefine { name: "recent_docs", graph: "__commons__", plan }`. The plan is `Scan Doc` then `Filter year > 2000`.
2. Add, update and remove `Doc` nodes.
3. Read with `PlanMatViewGet { name: "recent_docs" }`. The answer equals a full recompute.
4. Check maintainability first with UQL: `EXPLAIN MATCH (:Doc) WHERE year > 2000`. The answer carries `incremental: true`.

Proof command: `cargo test -p medulla-server-served-tests --features matview,redb plan_view_served_define_write_get_equals_recompute`.

## SQL view, over pgwire

```sql
CREATE TABLE orders (id bigint PRIMARY KEY, region text, amount numeric(12,2));
CREATE MATERIALIZED VIEW by_region AS
  SELECT region, count(*), sum(amount), avg(amount) FROM orders WHERE amount > 0 GROUP BY region;
INSERT INTO orders VALUES (1, 'west', 10.00), (2, 'west', 5.50);
SELECT * FROM by_region;
SELECT name, mode, reason_code FROM medulla_catalog.matviews;
```

The view is current after each commit. Its mode reads `incremental`.

Proof command: `cargo test --test pgwire_roundtrip matview_incremental_read_after_commit`.

## Watch a view change

Read `CdcRead { graph: "view:recent_docs", from_seq: 0, limit: 256 }`. Each event is a row insert or delete of the view result. On `gap: true`, read the view, then resume from the `head_seq` read before it.

Proof command: `cargo test -p medulla-server-served-tests --features matview,redb view_delta_feed_matches_result_difference`.

## Observe

- `SELECT * FROM medulla_catalog.matviews` lists mode, reason, state size and checks.
- The metrics endpoint serves `medulla_view_changes_applied_total`, `medulla_view_demotions_total` and `medulla_view_shadow_checks_total`.
