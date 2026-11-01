# MEDULLA-INCREMENTAL-VIEWS — Data model

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Delta (R002)

File: `crates/medulla-types/src/delta.rs`.

| Type | Field | Type | Rule |
|---|---|---|---|
| `ZRow` | `id` | `String` | Row key: node id or SQL primary key as text |
| `ZRow` | `score` | `Option<f32>` | Carried score, unchanged |
| `ZRow` | `props` | `serde_json::Map<String, Value>` | Row image |
| `ZRow` | `weight` | `i64` | Never zero after `consolidate` |
| `Delta` | `rows` | `Vec<ZRow>` | Order is first-seen order |
| `DeltaError` | `WeightOverflow { id }` | enum | Maps to `DELTA_WEIGHT_OVERFLOW` |

## Circuit additions (R007.1, R010.1)

File: `crates/medulla-exec/src/incremental/circuit.rs`.

| Type | Field | Type | Rule |
|---|---|---|---|
| `Circuit` | `state_bytes` | `u64` | `#[serde(default)]`; maintained in `apply` |
| `ApplyOutcome` | `touched` | `usize` | State entries touched |
| `ApplyOutcome` | `output` | `Delta` | Consolidated change of the result |

## Manager additions (R007.3, R008.1)

File: `crates/medulla-server-cdc/src/matview/manager.rs`.

| Type | Field | Type | Rule |
|---|---|---|---|
| `CircuitSlot` | `Resident(Circuit)` | variant | Folds changes |
| `CircuitSlot` | `Spilled { pending }` | `Option<Arc<Vec<u8>>>` | Bytes not yet acknowledged by the store |
| `Tracked` | `last_change_ms` | `u64` | Spill order |
| `Tracked` | `since_check` | `u64` | Changes since the last shadow check |
| `Tracked` | `check_due` | `bool` | Set at `MEDULLA_VIEW_SHADOW_EVERY` |

## View status (R005.1)

File: `crates/medulla-core/src/view_status.rs`.

| Field | Type | Values |
|---|---|---|
| `name` | `String` | View name |
| `family` | `ViewFamily` | `Plan`, `Sql` |
| `graph` | `String` | Graph or SQL scope |
| `mode` | `ViewMode` | `Incremental`, `Recompute`, `Spilled` |
| `reason_code` | `Option<String>` | A `ServerErrorCode` string |
| `reason` | `Option<String>` | Human text |
| `state_bytes` | `u64` | From `Circuit::state_bytes`; 0 for SQL views |
| `applied_changes` | `u64` | Changes folded since install |
| `shadow_checks` | `u64` | Completed checks |
| `shadow_mismatches` | `u64` | Mismatches found |

The system table `medulla_catalog.matviews` has one column per field, in this order. Text columns render enum values in lower case.

## SQL view program (R009.1, R012.1, R013.1)

File: `crates/medulla-table-store/src/tables/matview_incremental.rs`.

| Type | Shape |
|---|---|
| `IncrementalViewProgram` | `{ view, table, join: Option<EquiJoin>, filter: Option<RowExpr>, output: ViewOutput }` |
| `ViewOutput` | `Rows(Vec<(String, RowExpr)>)`, `Groups { keys, aggs }`, `Distinct(Vec<(String, RowExpr)>)` |
| `ExactAgg` | `Count`, `Sum(RowExpr)`, `Avg(RowExpr)`, `Min(RowExpr)`, `Max(RowExpr)` |
| `RowExpr` | column, literal, `+ - *`, comparison, `AND`, `OR`, `NOT`, `IS NULL`, `IS NOT NULL` |
| `EquiJoin` | `{ right, left_key, right_key }` |
| `RowChange` | old and new image of one row, from MEDULLA-ENGINE-PARITY-R015.1 |
| `ViewRowChanges` | `{ view, deleted: Vec<Row>, inserted: Vec<Row> }` |
| `NotIncremental` | `{ construct: String }`, code `MATVIEW_NOT_INCREMENTAL` |

Grouped and distinct result rows add the hidden column `__count` of type bigint.

## Codes added to `ServerErrorCode`

| Variant | Code | Row |
|---|---|---|
| `DeltaWeightOverflow` | `DELTA_WEIGHT_OVERFLOW` | R002.2 |
| `MatviewNotIncremental` | `MATVIEW_NOT_INCREMENTAL` | R005.5 |
| `StateBudgetExceeded` | `STATE_BUDGET_EXCEEDED` | R007.2 |
| `ShadowMismatch` | `SHADOW_MISMATCH` | R008.2 |
| `MatviewUnknown` | `MATVIEW_UNKNOWN` | R010.3, shared with MEDULLA-ENGINE-PARITY-R014.1 |

## Knobs

| Knob | Default | Range | Unit | Row |
|---|---|---|---|---|
| `MEDULLA_VIEW_STATE_BYTES` | 268435456 | [1048576, 17179869184] | bytes | R007.2 |
| `MEDULLA_VIEWS_MEMORY_BYTES` | 1073741824 | [16777216, 68719476736] | bytes | R007.3 |
| `MEDULLA_VIEW_SHADOW_EVERY` | 1000 | [10, 1000000] | changes | R008.1 |
| `MEDULLA_VIEW_SHADOW_TICK_MS` | 1000 | [100, 60000] | ms | R008.1 |
| `MEDULLA_VIEW_JOIN_FANOUT` | 10000 | [1, 1000000] | rows | R013.2 |

## Metrics (R005.4)

| Metric | Kind | Labels |
|---|---|---|
| `medulla_view_changes_applied_total` | counter | `family` |
| `medulla_view_demotions_total` | counter | `family`, `reason_code` |
| `medulla_view_shadow_checks_total` | counter | `family`, `outcome` |
| `medulla_view_state_bytes` | gauge | `family` |
| `medulla_views_resident_state_bytes` | gauge | none |
