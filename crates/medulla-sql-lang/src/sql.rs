//! Modules moved here from `medulla_sql::sql` (hard cutover, no re-exports).

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod classify;
/// SQL stored-function (`CREATE FUNCTION … LANGUAGE sql`) expansion (CONCEPT:MEDULLA-KG.query.create-drop-function) —
/// inline a scalar function body as a scalar subquery, expand a table function as a
/// parameterized-view subquery in `FROM`; reuses the SQL exec path (no new evaluator).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod funcs;
/// Postgres-family extension parity: pgvector ANN index pushdown, TimescaleDB hypertables/continuous-aggregates, and
/// ParadeDB `@@@` BM25 — the pure parse/plan/project layer.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pgfamily;
/// SQL:2023 SQL/PGQ property-graph definitions and bounded `GRAPH_TABLE`
/// relational lowering. Execution stays on this module's sole DataFusion path.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pgq;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pgvector_ddl;
/// PL/pgSQL procedural interpreter (CONCEPT:MEDULLA-KG.query.medulla-validate-procedural-body/MEDULLA-341): parse + execute a
/// `LANGUAGE plpgsql` body (DECLARE vars, BEGIN..END, `:=`, IF/ELSIF/ELSE, LOOP/WHILE/
/// FOR, RETURN, RAISE, `SELECT … INTO`) against a variable environment, running embedded
/// SQL back through the same read path. Pure Rust — no new deps (folds into `sql`).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod plpgsql;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod typed_result;
