# Incremental views

A materialized view is kept up to date as its source changes, so a read of the view is a read of a
stored result and never a recompute. Two families exist: **plan views**, defined over a graph by a
plan, and **SQL views**, defined by `CREATE MATERIALIZED VIEW ... AS SELECT ...` over user tables.
A view that its shape cannot be maintained for stays a **recompute** view: it is marked stale on
every change to its inputs and recomputed when it is next read. The `mode` column of
`medulla_catalog.matviews` says which one a view is, and the `reason_code` column says why a
recompute view is not incremental.

## Quickstart

```sql
CREATE TABLE orders (id BIGINT PRIMARY KEY, region TEXT, amount NUMERIC(12,2));

CREATE MATERIALIZED VIEW by_region AS
  SELECT region, count(*), sum(amount), avg(amount)
  FROM orders WHERE amount > 0 GROUP BY region;

-- Fresh after every commit, with no REFRESH:
SELECT * FROM by_region;

SELECT name, mode, reason_code FROM medulla_catalog.matviews;
```

A view is updated in the same transaction that changes its source, so a rolled-back commit leaves
the view unchanged. After the commit is durable, the rows the view gained and lost are published to
the change feed `view:<name>`, a delete of the old row and an insert of the new one for a grouped
view; a `CdcRead` of that feed resumes from a cursor like any graph feed.

## Supported shapes

| Family | Shape | Notes |
|--------|-------|-------|
| plan | `Scan` of one label | the base of every plan view |
| plan | `Filter` | numeric and equality predicates on node properties |
| plan | `AsOf` | a fixed instant on either time axis |
| plan | `Limit` | a window over the ordered result |
| plan | `WindowAgg` | a count over a time window |
| SQL | filter | a `WHERE` over one table |
| SQL | projection | scalar expressions, including exact arithmetic |
| SQL | grouped `count`, `sum`, `avg` | over exact numbers, with `GROUP BY` |
| SQL | grouped `min`, `max` | a retracted extreme recomputes only its group |
| SQL | `SELECT DISTINCT` | a group count per distinct row |
| SQL | one inner equi-join | one equality between a column of each table, both keys indexed |

Everything else keeps the view in recompute mode: a second table without an indexed equality join,
an outer, semi or anti join, a subquery, a window function, `ORDER BY`, `LIMIT`, `HAVING`, a
volatile function, a float column under `sum` or `avg`, and `SELECT *`. The first construct found
is the one the `reason` column names.

## Knobs

| Knob | Environment | Default | Range | Unit |
|------|-------------|---------|-------|------|
| `view.state_bytes` | `MEDULLA_VIEW_STATE_BYTES` | 268435456 | [1048576, 17179869184] | bytes |
| `views.memory_bytes` | `MEDULLA_VIEWS_MEMORY_BYTES` | 1073741824 | [16777216, 68719476736] | bytes |
| `view.shadow_every` | `MEDULLA_VIEW_SHADOW_EVERY` | 1000 | [10, 1000000] | changes |
| `view.shadow_tick_ms` | `MEDULLA_VIEW_SHADOW_TICK_MS` | 1000 | [100, 60000] | ms |
| `view.join_fanout` | `MEDULLA_VIEW_JOIN_FANOUT` | 10000 | [1, 1000000] | rows |

`view.state_bytes` is the largest maintained state of one view: past it the view demotes to recompute
with `STATE_BUDGET_EXCEEDED`. `views.memory_bytes` bounds the resident state of all views together:
past it the view that changed least recently spills to the store and reloads on its next change or
read. `view.shadow_every` is the number of applied changes between two shadow checks of a view, and
has no off value; `view.shadow_tick_ms` is the period of the task that runs the checks. A shadow check
recomputes the view and compares it with the maintained rows. `view.join_fanout` is the most rows one
join probe may read per key: a larger fan-out refuses the commit with `STATE_BUDGET_EXCEEDED`; an
out-of-range or unparsable value falls back to the default with one warning.

## Reason codes

| Code | Meaning |
|------|---------|
| `MATVIEW_NOT_INCREMENTAL` | the defining query has a construct no incremental form maintains |
| `STATE_BUDGET_EXCEEDED` | a view's state, or a join probe, went past its budget |
| `SHADOW_MISMATCH` | a shadow check found the maintained rows different from a recompute |
| `MATVIEW_UNKNOWN` | the named view does not exist |
| `MATVIEW_EXISTS` | a view of that name already exists |

A demotion keeps the view correct: it is recomputed from its source on the next read. It is counted by
`medulla_view_demotions_total` with its reason code.

## Metrics

| Metric | Labels | Meaning |
|--------|--------|---------|
| `medulla_view_changes_applied_total` | `family` | changes folded into incremental views |
| `medulla_view_demotions_total` | `family`, `reason_code` | views demoted out of incremental mode |
| `medulla_view_shadow_checks_total` | `family`, `outcome` | shadow checks, `match` or `mismatch` |
| `medulla_view_state_bytes` | `family` | maintained state of the views of one family |
| `medulla_views_resident_state_bytes` | none | maintained state of every resident view |

## The `medulla_catalog.matviews` columns

| Column | Meaning |
|--------|---------|
| `name` | the view name |
| `family` | `plan` or `sql` |
| `graph` | the graph the view reads |
| `mode` | `incremental`, `recompute` or `spilled` |
| `reason_code` | why the view is not incremental, or null |
| `reason` | the first construct or condition behind the code, or null |
| `state_bytes` | the maintained state of the view |
| `applied_changes` | changes folded into the view |
| `shadow_checks` | shadow checks run |
| `shadow_mismatches` | shadow checks that differed |
| `stale` | true when a committed change to an input is not yet in the view's rows; always false for an incremental view |
