//! Plan-backed materialized-view MANAGER (CONCEPT:MEDULLA-KG.storage.plan-backed-matview).
//!
//! Holds the in-RAM registry of named, plan-backed materialized views + their freshness
//! state. A subsystem-local process singleton ([`manager`]) so BOTH the CDC hub
//! (`CdcHub::emit` → [`note_change`]) and the dispatch handler reach it WITHOUT threading
//! a new field through the shared `ServerState` (owned by another lane).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use medulla_core::view_status;
use medulla_exec::incremental::Circuit;
use medulla_server_api::knob_config;
use medulla_types::contract::ServerErrorCode;
use medulla_types::delta::Delta;
use parking_lot::{Mutex, RwLock};

use super::incremental::{decode_operator_state, encode_operator_state};
use super::spill::{settle_target, ViewStateSpill};

/// The durable DEFINITION of a plan-backed materialized view (CONCEPT:MEDULLA-KG.storage.plan-backed-matview):
/// a named `wire::Plan` over ONE `graph`. Serialized (MessagePack) into the disjoint
/// `plan_matviews` redb table. Carries NO result rows — the materialized RESULT rides the
/// version-keyed, RLS-aware result cache on the graph's `GraphCore`, so a write that bumps
/// the graph version retires it for free.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PlanMatView {
    pub name: String,
    pub graph: String,
    pub plan: medulla_types::wire::Plan,
}

/// Which maintenance mode a tracked view runs in
/// (MEDULLA-INCREMENTAL-VIEWS-R001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Its plan compiled to a DBSP circuit: `apply_delta` folds each CDC change into the
    /// circuit's maintained state in O(delta); `Get` projects the result directly from
    /// that state (no recompute, no cache round-trip).
    Incremental,
    /// Its plan has an op with no incremental form (Traverse/Reason/…): today's
    /// recompute-on-`Get` path, unchanged. A CDC change flags it stale.
    Recompute,
    /// Incremental, with its circuit spilled out of memory (MEDULLA-INCREMENTAL-VIEWS-R007.3).
    Spilled,
}

/// Where an incremental view's circuit lives (MEDULLA-INCREMENTAL-VIEWS-R007.3).
enum CircuitSlot {
    Resident(Circuit),
    /// Spilled to the store; `pending` holds the bytes until the store acknowledges them.
    Spilled {
        pending: Option<Arc<Vec<u8>>>,
    },
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// In-RAM per-view tracking: the definition + freshness flag + (for `Incremental` views)
/// the compiled circuit + its CDC watermark, or (for `Recompute` views) the typed reason
/// its plan could not be incrementalized.
struct Tracked {
    def: PlanMatView,
    /// Set by [`PlanMatViewManager::note_change`] when a committed write to `def.graph`
    /// lands — so the next `Get` (or an explicit `Refresh`) recomputes even if a stale
    /// cache entry somehow lingered. A freshly-materialized view clears it. Governs
    /// `Mode::Recompute` views only; an `Incremental` view is fresh by construction.
    stale: bool,
    /// The maintenance mode (MEDULLA-INCREMENTAL-VIEWS-R001).
    mode: Mode,
    /// `Some` ⇒ `Mode::Incremental`: the compiled DBSP circuit `apply_delta` folds
    /// deltas into. Its maintained STATE (membership map / bucket accumulators) is the
    /// authoritative result; the RowSet is projected lazily by `current()` at read, so
    /// per-delta maintenance stays O(delta), not O(view). `None` ⇒ `Mode::Recompute`.
    circuit: Option<CircuitSlot>,
    /// The typed reason this view fell back to `Mode::Recompute` (the first unsupported
    /// op) — first-class + queryable (MEDULLA-INCREMENTAL-VIEWS-R001), never a
    /// silent drop. `None` for an `Incremental` view or a boot-reloaded view.
    fallback_reason: Option<String>,
    /// CDC watermark — the next `seq` this view expects (mirrors
    /// `cdc::ContinuousQuery::through_seq`). Deltas with `seq < through_seq` are already
    /// folded and skipped.
    through_seq: u64,
    /// Unix milliseconds of the last change folded in (or of install); orders spills.
    last_change_ms: u64,
    /// CDC changes folded into this view's circuit — advanced ONCE per
    /// `apply_delta` call, not per row (MEDULLA-INCREMENTAL-VIEWS-R005.2).
    applied_changes: u64,
}

impl Tracked {
    /// Drop the circuit and fall back to recompute with `reason`; the next read recomputes.
    fn demote(&mut self, reason: String) {
        self.mode = Mode::Recompute;
        self.circuit = None;
        self.fallback_reason = Some(reason);
        self.through_seq = 0;
        self.stale = true;
        // The mode flip is a state change: the board must not keep advertising the
        // old mode (MEDULLA-INCREMENTAL-VIEWS-R005.2).
        view_status::publish(status_of(self));
    }

    fn resident_bytes(&self) -> u64 {
        match &self.circuit {
            Some(CircuitSlot::Resident(circuit)) => circuit.state_bytes(),
            _ => 0,
        }
    }

    /// The view's maintained state in bytes for the status board
    /// (MEDULLA-INCREMENTAL-VIEWS-R005.2): the resident circuit's incrementally
    /// maintained total, the spilled blob's size while out of memory, zero for a
    /// recompute view.
    fn maintained_state_bytes(&self) -> u64 {
        match &self.circuit {
            Some(CircuitSlot::Resident(circuit)) => circuit.state_bytes(),
            Some(CircuitSlot::Spilled { pending }) => {
                pending.as_ref().map_or(0, |b| b.len() as u64)
            }
            None => 0,
        }
    }
}

/// The mode the status board reports for `t` — a spilled circuit reads as `Spilled`,
/// mirroring [`PlanMatViewManager::mode`].
fn view_mode_of(t: &Tracked) -> view_status::ViewMode {
    match &t.circuit {
        Some(CircuitSlot::Spilled { .. }) => view_status::ViewMode::Spilled,
        _ if t.mode == Mode::Incremental => view_status::ViewMode::Incremental,
        _ => view_status::ViewMode::Recompute,
    }
}

/// The published status-board entry for `t` (MEDULLA-INCREMENTAL-VIEWS-R005.2).
fn status_of(t: &Tracked) -> view_status::ViewStatus {
    view_status::ViewStatus {
        name: t.def.name.clone(),
        family: view_status::ViewFamily::Plan,
        graph: t.def.graph.clone(),
        mode: view_mode_of(t),
        reason_code: t.fallback_reason.clone(),
        reason: None,
        state_bytes: t.maintained_state_bytes(),
        applied_changes: t.applied_changes,
        shadow_checks: 0,
        shadow_mismatches: 0,
    }
}

/// A view over its per-view state ceiling falls back to recompute with a typed reason
/// (MEDULLA-INCREMENTAL-VIEWS-R007.2).
fn enforce_view_ceiling(t: &mut Tracked, ceiling: u64) {
    if t.resident_bytes() <= ceiling {
        return;
    }
    let reason = ServerErrorCode::StateBudgetExceeded.as_str();
    medulla_kernel::metrics::process().view_demoted("plan", reason);
    tracing::warn!(
        target: "medulla::matview",
        view = %t.def.name,
        ceiling,
        reason,
        "incremental matview exceeded its state ceiling and falls back to recompute",
    );
    t.demote(reason.to_string());
}

/// The process-global plan-backed matview registry.
#[derive(Default)]
pub struct PlanMatViewManager {
    views: Mutex<HashMap<String, Tracked>>,
    spill: RwLock<Option<Arc<dyn ViewStateSpill>>>,
}

static MANAGER: OnceLock<PlanMatViewManager> = OnceLock::new();

/// The process-global plan-backed matview manager (CONCEPT:MEDULLA-KG.storage.plan-backed-matview).
pub fn manager() -> &'static PlanMatViewManager {
    MANAGER.get_or_init(PlanMatViewManager::default)
}

impl PlanMatViewManager {
    /// Insert (or replace) a view definition in `Mode::Recompute`, marking it freshly
    /// materialized (no fallback reason — boot reload / re-materializes lazily on `Get`).
    pub fn define(&self, def: PlanMatView) {
        self.insert_recompute(def, None);
    }

    /// Insert (or replace) a view in `Mode::Recompute` because its plan carried an op the
    /// circuit compiler can't incrementalize, recording the TYPED `reason` so the fallback
    /// is queryable (MEDULLA-INCREMENTAL-VIEWS-R001) — never a silent drop.
    pub fn note_fallback(&self, def: PlanMatView, reason: String) {
        self.insert_recompute(def, Some(reason));
    }

    fn insert_recompute(&self, def: PlanMatView, fallback_reason: Option<String>) {
        let name = def.name.clone();
        let status = {
            let mut views = self.views.lock();
            views.insert(
                name.clone(),
                Tracked {
                    def,
                    stale: false,
                    mode: Mode::Recompute,
                    circuit: None,
                    fallback_reason,
                    through_seq: 0,
                    last_change_ms: 0,
                    applied_changes: 0,
                },
            );
            status_of(views.get(&name).expect("just-inserted view present"))
        };
        // MEDULLA-INCREMENTAL-VIEWS-R005.2: the board learns of the recompute view.
        view_status::publish(status);
    }

    /// Install (or replace) a view in `Mode::Incremental` with its compiled + seeded
    /// `circuit` and the CDC `through_seq` watermark captured at definition time
    /// (MEDULLA-INCREMENTAL-VIEWS-R001). The circuit's maintained state is the
    /// authoritative result; `Get` projects it lazily.
    pub fn install_incremental(&self, def: PlanMatView, circuit: Circuit, through_seq: u64) {
        let name = def.name.clone();
        let status = {
            let mut views = self.views.lock();
            views.insert(
                name.clone(),
                Tracked {
                    def,
                    stale: false,
                    mode: Mode::Incremental,
                    circuit: Some(CircuitSlot::Resident(circuit)),
                    fallback_reason: None,
                    through_seq,
                    last_change_ms: now_ms(),
                    applied_changes: 0,
                },
            );
            status_of(views.get(&name).expect("just-inserted view present"))
        };
        // MEDULLA-INCREMENTAL-VIEWS-R005.2: the board learns of the incremental view.
        view_status::publish(status);
    }

    /// The maintenance mode of `name`, if tracked.
    pub fn mode(&self, name: &str) -> Option<Mode> {
        self.views.lock().get(name).map(|t| match t.circuit {
            Some(CircuitSlot::Spilled { .. }) => Mode::Spilled,
            _ => t.mode,
        })
    }

    /// Install the store that backs spilled view state (MEDULLA-INCREMENTAL-VIEWS-R007.3).
    pub fn install_spill(&self, spill: Arc<dyn ViewStateSpill>) {
        *self.spill.write() = Some(spill);
    }

    /// The store acknowledged the write of `bytes` for `name`: the pending copy is released.
    pub fn spill_acknowledged(&self, name: &str, bytes: &Arc<Vec<u8>>) {
        let mut views = self.views.lock();
        if let Some(CircuitSlot::Spilled { pending }) =
            views.get_mut(name).and_then(|t| t.circuit.as_mut())
        {
            if pending
                .as_ref()
                .is_some_and(|held| Arc::ptr_eq(held, bytes))
            {
                *pending = None;
            }
        }
    }

    /// Bring a spilled circuit back into memory from `pending` or the store. A view whose
    /// state cannot be read falls back to recompute, so no answer rests on lost state.
    fn ensure_resident(&self, t: &mut Tracked) -> bool {
        let Some(CircuitSlot::Spilled { pending }) = &t.circuit else {
            return t.circuit.is_some();
        };
        let blob = match pending {
            Some(bytes) => Some(bytes.as_ref().clone()),
            None => self.spill.read().as_ref().and_then(|s| s.get(&t.def.name)),
        };
        match blob.map(|b| decode_operator_state(&b)) {
            Some(Ok(circuit)) => {
                t.circuit = Some(CircuitSlot::Resident(circuit));
                true
            }
            other => {
                let why = other
                    .and_then(Result::err)
                    .unwrap_or_else(|| "missing".into());
                t.demote(format!("spilled view state unreadable: {why}"));
                false
            }
        }
    }

    /// Spill the least recently changed resident views (never one of `keep`) until the
    /// resident total is under 90 percent of `budget`. Returns the spilled blobs, to be
    /// handed to the store after the lock is released.
    fn spill_over_budget(
        &self,
        views: &mut HashMap<String, Tracked>,
        keep: &[String],
        budget: u64,
    ) -> Vec<(String, Arc<Vec<u8>>)> {
        let mut out = Vec::new();
        if self.spill.read().is_none() {
            return out;
        }
        let mut total: u64 = views.values().map(Tracked::resident_bytes).sum();
        if total <= budget {
            return out;
        }
        let mut order: Vec<(u64, String)> = views
            .values()
            .filter(|t| t.resident_bytes() > 0 && !keep.contains(&t.def.name))
            .map(|t| (t.last_change_ms, t.def.name.clone()))
            .collect();
        order.sort();
        for (_, name) in order {
            if total <= settle_target(budget) {
                break;
            }
            let Some(t) = views.get_mut(&name) else {
                continue;
            };
            let Some(CircuitSlot::Resident(circuit)) = &t.circuit else {
                continue;
            };
            let Ok(blob) = encode_operator_state(circuit) else {
                continue;
            };
            total = total.saturating_sub(circuit.state_bytes());
            let bytes = Arc::new(blob);
            t.circuit = Some(CircuitSlot::Spilled {
                pending: Some(bytes.clone()),
            });
            out.push((name, bytes));
        }
        out
    }

    fn flush_spills(&self, puts: Vec<(String, Arc<Vec<u8>>)>) {
        let Some(spill) = self.spill.read().clone() else {
            return;
        };
        for (name, bytes) in puts {
            spill.put(&name, bytes);
        }
    }

    /// The typed fallback reason for a `Mode::Recompute` view (why its plan was not
    /// incrementalizable) — the queryable half of the first-class fallback. `None` for an
    /// `Incremental` view, a boot-reloaded view, or an unknown one.
    pub fn fallback_reason(&self, name: &str) -> Option<String> {
        self.views
            .lock()
            .get(name)
            .and_then(|t| t.fallback_reason.clone())
    }

    /// The live maintained result rows `[id, score?]` of an `Incremental` view, PROJECTED
    /// from the circuit's maintained state at read (`None` for an unknown or
    /// `Recompute`-mode view — the caller then takes the recompute path). This projection
    /// is O(view); the per-delta maintenance in `apply_delta` is O(delta).
    pub fn incremental_rows(&self, name: &str) -> Option<Vec<(String, Option<f32>)>> {
        let mut views = self.views.lock();
        let t = views.get_mut(name)?;
        if t.mode != Mode::Incremental || !self.ensure_resident(t) {
            return None;
        }
        let Some(CircuitSlot::Resident(circuit)) = &t.circuit else {
            return None;
        };
        let rows = circuit
            .current()
            .rows()
            .iter()
            .map(|r| (r.id.clone(), r.score))
            .collect();
        let puts = self.spill_over_budget(
            &mut views,
            &[name.to_string()],
            knob_config::knobs().views_memory_bytes,
        );
        drop(views);
        self.flush_spills(puts);
        Some(rows)
    }

    /// A snapshot of an `Incremental` view's compiled circuit (for durable operator-state
    /// persistence). `None` for unknown/`Recompute` views.
    pub fn circuit_snapshot(&self, name: &str) -> Option<Circuit> {
        match self.views.lock().get(name)?.circuit.as_ref()? {
            CircuitSlot::Resident(circuit) => Some(circuit.clone()),
            CircuitSlot::Spilled { .. } => None,
        }
    }

    /// INCREMENTAL CDC maintenance (MEDULLA-INCREMENTAL-VIEWS-R001): fold one
    /// change into every `Mode::Incremental` view over `graph`. For each such view, only a
    /// change at/after its watermark is applied (mirroring `cdc::maintain`); the circuit's
    /// output updates `current` and the watermark advances. Returns how many views it
    /// maintained. `Mode::Recompute` views are UNTOUCHED here (they ride `note_change`).
    pub fn apply_delta(&self, graph: &str, event: &medulla_types::wire::CdcEvent) -> usize {
        let delta = super::incremental::event_to_delta(event);
        self.apply_signed(graph, event.seq, &delta)
    }

    /// Fold one signed `delta` at change `seq` into every `Mode::Incremental` view over
    /// `graph` (the core of [`Self::apply_delta`]). A view whose circuit refuses the delta
    /// (MEDULLA-INCREMENTAL-VIEWS-R002.3) is demoted to `Mode::Recompute` with the typed
    /// reason code (`DELTA_WEIGHT_OVERFLOW`) and marked stale, so the next read
    /// recomputes it.
    fn apply_signed(&self, graph: &str, seq: u64, delta: &Delta) -> usize {
        let knobs = knob_config::knobs();
        self.apply_limited(
            graph,
            seq,
            delta,
            (knobs.view_state_bytes, knobs.views_memory_bytes),
        )
    }

    /// [`Self::apply_signed`] under explicit `(per-view ceiling, resident budget)` limits.
    fn apply_limited(&self, graph: &str, seq: u64, delta: &Delta, limits: (u64, u64)) -> usize {
        let (ceiling, budget) = limits;
        let mut changed = Vec::new();
        let puts = {
            let mut views = self.views.lock();
            for t in views.values_mut() {
                if t.mode != Mode::Incremental || t.def.graph != graph || seq < t.through_seq {
                    continue;
                }
                if self.fold_one(t, seq, delta, ceiling) {
                    // MEDULLA-INCREMENTAL-VIEWS-R005.2: one increment per call, not per row.
                    t.applied_changes = t.applied_changes.saturating_add(1);
                    changed.push(t.def.name.clone());
                }
            }
            let puts = self.spill_over_budget(&mut views, &changed, budget);
            // The board sees each maintained view's updated counters + state size
            // (post-spill mode), once per call (MEDULLA-INCREMENTAL-VIEWS-R005.2).
            for name in &changed {
                if let Some(t) = views.get(name) {
                    view_status::publish(status_of(t));
                }
            }
            puts
        };
        self.flush_spills(puts);
        changed.len()
    }

    /// Fold `delta` into one view's circuit. A refused delta or an over-ceiling state
    /// demotes the view. Returns whether the view was maintained.
    fn fold_one(&self, t: &mut Tracked, seq: u64, delta: &Delta, ceiling: u64) -> bool {
        if !self.ensure_resident(t) {
            return false;
        }
        let Some(CircuitSlot::Resident(circuit)) = t.circuit.as_mut() else {
            return false;
        };
        // O(delta) maintenance: fold the signed delta into the circuit's state ONLY (no
        // O(view) result projection here: `Get` projects `current()` lazily). `touched` is
        // the rows-touched instrument (bounded by the delta size, independent of view size).
        let touched = if delta.is_empty() {
            0
        } else {
            match circuit.apply(delta) {
                Ok(touched) => touched,
                Err(err) => {
                    t.demote(err.code().as_str().to_string());
                    return false;
                }
            }
        };
        t.through_seq = seq + 1;
        t.last_change_ms = now_ms();
        tracing::trace!(
            target: "medulla::matview",
            view = %t.def.name,
            graph = %t.def.graph,
            seq,
            delta = delta.len(),
            touched,
            "incremental matview maintained one CDC delta",
        );
        enforce_view_ceiling(t, ceiling);
        true
    }

    /// The stored definition for `name`, if any.
    pub fn get(&self, name: &str) -> Option<PlanMatView> {
        self.views.lock().get(name).map(|t| t.def.clone())
    }

    /// Whether `name` needs recompute: a CDC change since the last materialize (or an
    /// unknown view) is stale. Reuses the (hash, version) result-cache discipline — this
    /// is the belt-and-braces signal ON TOP of the version key.
    pub fn is_stale(&self, name: &str) -> bool {
        self.views.lock().get(name).map(|t| t.stale).unwrap_or(true)
    }

    /// Mark `name` freshly materialized (clears the CDC stale flag).
    pub fn mark_fresh(&self, name: &str) {
        if let Some(t) = self.views.lock().get_mut(name) {
            t.stale = false;
        }
    }

    /// Drop a view's tracking (and its status-board entry); returns whether it existed.
    pub fn drop_view(&self, name: &str) -> bool {
        let existed = self.views.lock().remove(name).is_some();
        view_status::remove(name);
        existed
    }

    /// The sorted names of every tracked view (observability/tests).
    pub fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.views.lock().keys().cloned().collect();
        v.sort();
        v
    }

    /// CDC INVALIDATION (CONCEPT:MEDULLA-KG.storage.matview-cdc-invalidation): mark EVERY view
    /// over `graph` stale. Called from `CdcHub::emit` per committed change. A committed
    /// write already bumped the graph's OCC version (retiring the cached result under the
    /// (query_hash, version) key); this additionally flags the view so a subsequent `Get`
    /// definitely recomputes. Returns how many views it retired (tests read this).
    pub fn note_change(&self, graph: &str) -> usize {
        let mut views = self.views.lock();
        let mut n = 0;
        for t in views.values_mut() {
            // Incremental views are maintained by `apply_delta`, not invalidated — the
            // stale flag governs only the recompute path.
            if t.mode == Mode::Incremental {
                continue;
            }
            if t.def.graph == graph && !t.stale {
                t.stale = true;
                n += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str, graph: &str) -> PlanMatView {
        PlanMatView {
            name: name.into(),
            graph: graph.into(),
            plan: medulla_types::wire::Plan::new(vec![]),
        }
    }

    /// A node-add CdcEvent carrying a `type`/`year` msgpack property blob.
    fn add_node_event(
        seq: u64,
        graph: &str,
        id: &str,
        ty: &str,
        year: i64,
    ) -> medulla_types::wire::CdcEvent {
        let after =
            rmp_serde::to_vec_named(&serde_json::json!({"type": ty, "year": year})).unwrap();
        medulla_types::wire::CdcEvent {
            seq,
            graph: graph.into(),
            kind: medulla_types::wire::CdcKind::AddNode,
            node_id: id.into(),
            target_id: String::new(),
            table: String::new(),
            committed_at_ms: 0,
            label: ty.into(),
            before: Vec::new(),
            after,
            had_before: false,
            had_after: true,
        }
    }

    fn remove_node_event(
        seq: u64,
        graph: &str,
        id: &str,
        ty: &str,
        year: i64,
    ) -> medulla_types::wire::CdcEvent {
        let before =
            rmp_serde::to_vec_named(&serde_json::json!({"type": ty, "year": year})).unwrap();
        medulla_types::wire::CdcEvent {
            seq,
            graph: graph.into(),
            kind: medulla_types::wire::CdcKind::RemoveNode,
            node_id: id.into(),
            target_id: String::new(),
            table: String::new(),
            committed_at_ms: 0,
            label: ty.into(),
            before,
            after: Vec::new(),
            had_before: true,
            had_after: false,
        }
    }

    /// MEDULLA-INCREMENTAL-VIEWS-R002.3: a delta the circuit refuses demotes the view to
    /// recompute mode with the typed reason code, and the view reads as stale.
    // spec: MEDULLA-INCREMENTAL-VIEWS-R002.3
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R002.3: Wiring: the circuit consolidates before it folds.
    #[test]
    fn apply_failure_demotes_with_typed_reason() {
        use medulla_types::delta::ZRow;
        use medulla_types::wire::{Op, Plan};
        let plan = Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]);
        let circuit = Circuit::compile(&plan).unwrap();
        let m = PlanMatViewManager::default();
        m.install_incremental(
            PlanMatView {
                name: "v".into(),
                graph: "g".into(),
                plan,
            },
            circuit,
            0,
        );
        let props = serde_json::json!({"type": "Doc"})
            .as_object()
            .cloned()
            .unwrap();
        let mut big = ZRow::insert("a", props.clone());
        big.weight = i64::MAX;
        let delta = Delta::from(vec![big, ZRow::insert("a", props)]);
        assert_eq!(m.apply_signed("g", 0, &delta), 0);
        assert_eq!(m.mode("v"), Some(Mode::Recompute));
        assert_eq!(
            m.fallback_reason("v").as_deref(),
            Some("DELTA_WEIGHT_OVERFLOW")
        );
        assert!(m.is_stale("v"));
        assert!(m.incremental_rows("v").is_none());
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R001.6
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R001.6: Define, change and read are wired
    #[test]
    fn apply_delta_maintains_incremental_view_end_to_end() {
        use medulla_types::wire::{Op, Plan, Pred};
        // Scan{Doc} |> Filter year > 2000 — an Incremental-mode view.
        let plan = Plan::new(vec![
            Op::Scan {
                label: "Doc".into(),
            },
            Op::Filter {
                preds: vec![Pred::GtNum {
                    prop: "year".into(),
                    n: 2000.0,
                }],
            },
        ]);
        let circuit = Circuit::compile(&plan).unwrap();
        let d = PlanMatView {
            name: "v".into(),
            graph: "g".into(),
            plan,
        };

        let m = PlanMatViewManager::default();
        m.install_incremental(d, circuit, 0);
        assert_eq!(m.mode("v"), Some(Mode::Incremental));

        // add a passing Doc (2005), a filtered-out Doc (1999), a non-Doc — via CDC events.
        assert_eq!(
            m.apply_delta("g", &add_node_event(0, "g", "a", "Doc", 2005)),
            1
        );
        assert_eq!(
            m.apply_delta("g", &add_node_event(1, "g", "b", "Doc", 1999)),
            1
        );
        assert_eq!(
            m.apply_delta("g", &add_node_event(2, "g", "x", "Other", 2010)),
            1
        );
        let rows = m.incremental_rows("v").unwrap();
        assert_eq!(
            rows.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["a"]
        );

        // a change to a DIFFERENT graph maintains nothing.
        assert_eq!(
            m.apply_delta("other", &add_node_event(3, "other", "c", "Doc", 2005)),
            0
        );

        // remove the passing node → view empties.
        assert_eq!(
            m.apply_delta("g", &remove_node_event(3, "g", "a", "Doc", 2005)),
            1
        );
        assert!(m.incremental_rows("v").unwrap().is_empty());

        // note_change does NOT flag an Incremental view stale (it's maintained, not retired).
        assert_eq!(m.note_change("g"), 0);

        // a delta below the watermark is skipped (idempotent replay guard): not maintained.
        assert_eq!(
            m.apply_delta("g", &add_node_event(0, "g", "z", "Doc", 2005)),
            0
        );
        assert!(
            m.incremental_rows("v").unwrap().is_empty(),
            "stale seq is not folded"
        );
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R001.7
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R001.7: A fallback keeps a typed reason
    #[test]
    fn note_fallback_records_queryable_reason() {
        // The first-class fallback: an unsupported-op plan is Recompute mode AND exposes
        // its typed reason via `fallback_reason` (queryable), never a silent drop.
        let m = PlanMatViewManager::default();
        m.note_fallback(
            def("t", "g"),
            "op #1 is not incrementally maintainable: Traverse".into(),
        );
        assert_eq!(m.mode("t"), Some(Mode::Recompute));
        assert_eq!(
            m.fallback_reason("t").as_deref(),
            Some("op #1 is not incrementally maintainable: Traverse")
        );
        // A plain define (boot reload) has no fallback reason; an Incremental view none.
        m.define(def("d", "g"));
        assert_eq!(m.fallback_reason("d"), None);
        assert_eq!(m.fallback_reason("unknown"), None);
    }

    #[test]
    fn recompute_mode_view_is_untouched_by_apply_delta() {
        let m = PlanMatViewManager::default();
        m.define(def("r", "g")); // Recompute mode (empty plan)
        assert_eq!(m.mode("r"), Some(Mode::Recompute));
        assert_eq!(
            m.apply_delta("g", &add_node_event(0, "g", "a", "Doc", 2005)),
            0
        );
        assert!(
            m.incremental_rows("r").is_none(),
            "recompute views expose no incremental rows"
        );
        assert_eq!(
            m.note_change("g"),
            1,
            "recompute views still flag stale on CDC"
        );
    }

    #[test]
    fn cdc_change_marks_only_views_over_that_graph_stale() {
        // A LOCAL manager (not the global singleton) so the test is isolated.
        let m = PlanMatViewManager::default();
        m.define(def("v_a", "g1"));
        m.define(def("v_b", "g1"));
        m.define(def("v_c", "g2"));

        // Freshly defined ⇒ not stale.
        assert!(!m.is_stale("v_a"));
        assert!(!m.is_stale("v_c"));

        // A committed change to g1 retires BOTH views over g1, none over g2.
        assert_eq!(m.note_change("g1"), 2);
        assert!(m.is_stale("v_a"));
        assert!(m.is_stale("v_b"));
        assert!(!m.is_stale("v_c"), "a g2 view is untouched by a g1 change");

        // A second change to g1 retires nothing new (already stale).
        assert_eq!(m.note_change("g1"), 0);

        // Re-materialize clears the flag; a later change retires it again.
        m.mark_fresh("v_a");
        assert!(!m.is_stale("v_a"));
        assert_eq!(m.note_change("g1"), 1);
        assert!(m.is_stale("v_a"));
    }

    #[test]
    fn define_get_drop_roundtrip() {
        let m = PlanMatViewManager::default();
        assert!(m.get("v").is_none());
        assert!(m.is_stale("v"), "an unknown view is stale (needs compute)");
        m.define(def("v", "g1"));
        assert_eq!(m.get("v").unwrap().graph, "g1");
        assert_eq!(m.list(), vec!["v".to_string()]);
        assert!(m.drop_view("v"));
        assert!(!m.drop_view("v"), "second drop is a no-op");
        assert!(m.get("v").is_none());
    }

    /// `n` Doc inserts with ids `<prefix>0..`.
    fn docs(prefix: &str, n: usize) -> Delta {
        use medulla_types::delta::ZRow;
        let props = serde_json::json!({"type": "Doc"})
            .as_object()
            .cloned()
            .unwrap();
        Delta::from(
            (0..n)
                .map(|i| ZRow::insert(format!("{prefix}{i}"), props.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn scan_view(m: &PlanMatViewManager, name: &str, graph: &str, label: &str) {
        use medulla_types::wire::{Op, Plan};
        let plan = Plan::new(vec![Op::Scan {
            label: label.into(),
        }]);
        let circuit = Circuit::compile(&plan).unwrap();
        let def = PlanMatView {
            name: name.into(),
            graph: graph.into(),
            plan,
        };
        m.install_incremental(def, circuit, 0);
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R007.2
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R007.2: Per-view ceiling demotes
    #[test]
    fn view_over_ceiling_demotes_with_typed_reason() {
        let m = PlanMatViewManager::default();
        scan_view(&m, "big", "g", "Doc");
        scan_view(&m, "small", "g", "Other");
        m.apply_limited("g", 0, &docs("d", 100), (1_000, u64::MAX));
        assert_eq!(m.mode("big"), Some(Mode::Recompute));
        assert_eq!(
            m.fallback_reason("big").as_deref(),
            Some("STATE_BUDGET_EXCEEDED")
        );
        assert!(m.is_stale("big"));
        assert!(m.incremental_rows("big").is_none());
        assert_eq!(m.mode("small"), Some(Mode::Incremental));
    }

    /// A spill sink that remembers the last blob it was given per view.
    #[derive(Default)]
    struct MemorySpill {
        store: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    }

    impl ViewStateSpill for MemorySpill {
        fn put(&self, name: &str, bytes: Arc<Vec<u8>>) {
            self.store.lock().insert(name.to_string(), bytes);
        }
        fn get(&self, name: &str) -> Option<Vec<u8>> {
            self.store.lock().get(name).map(|b| b.as_ref().clone())
        }
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R007.3
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R007.3: Spill and reload by least recent change
    #[test]
    fn least_recent_view_spills_and_reloads_equal() {
        let m = PlanMatViewManager::default();
        let sink = Arc::new(MemorySpill::default());
        m.install_spill(sink.clone());
        scan_view(&m, "va", "ga", "Doc");
        scan_view(&m, "vb", "gb", "Doc");
        let limits = (u64::MAX, 700);
        m.apply_limited("ga", 0, &docs("a", 10), limits);
        let before = m.incremental_rows("va").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        m.apply_limited("gb", 0, &docs("b", 10), limits);
        assert_eq!(m.mode("va"), Some(Mode::Spilled), "the older view spills");
        assert_eq!(
            m.mode("vb"),
            Some(Mode::Incremental),
            "the changed view stays"
        );
        assert!(sink.get("va").is_some());
        // A read reloads it from `pending`, equal to what it held.
        assert_eq!(m.incremental_rows("va").unwrap(), before);
        assert_eq!(m.mode("va"), Some(Mode::Incremental));
        // After a change to `va`, `vb` is the least recent and spills; once acknowledged,
        // the reload comes from the store alone.
        std::thread::sleep(std::time::Duration::from_millis(3));
        m.apply_limited("ga", 1, &docs("c", 1), limits);
        assert_eq!(m.mode("vb"), Some(Mode::Spilled));
        let put = sink.store.lock().get("vb").cloned().unwrap();
        m.spill_acknowledged("vb", &put);
        drop(put);
        assert_eq!(m.incremental_rows("vb").unwrap().len(), 10);
    }
    // spec: MEDULLA-INCREMENTAL-VIEWS-R005.2
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R005.2: The manager publishes every state change
    #[test]
    fn status_board_tracks_mode_and_changes() {
        use medulla_core::view_status::{ViewFamily, ViewMode};
        use medulla_types::wire::{Op, Plan};
        let plan = Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]);
        let circuit = Circuit::compile(&plan).unwrap();
        let m = PlanMatViewManager::default();
        let name = "iv_r005_2_status_view";
        m.install_incremental(
            PlanMatView {
                name: name.into(),
                graph: "g".into(),
                plan,
            },
            circuit,
            0,
        );
        let find = || view_status::snapshot().into_iter().find(|s| s.name == name);
        let s = find().expect("install publishes the view");
        assert_eq!(s.family, ViewFamily::Plan);
        assert_eq!(s.graph, "g");
        assert_eq!(s.mode, ViewMode::Incremental);
        assert_eq!(s.applied_changes, 0);
        assert_eq!(s.reason_code, None);

        // one folded CDC change advances applied_changes once; state grows.
        assert_eq!(
            m.apply_delta("g", &add_node_event(0, "g", "a", "Doc", 2005)),
            1
        );
        let s = find().expect("apply_delta re-publishes the view");
        assert_eq!(s.applied_changes, 1);
        assert!(s.state_bytes > 0, "state_bytes tracks the folded state");

        // a fallback view publishes recompute mode with the typed reason code.
        let f = "iv_r005_2_fallback_view";
        m.note_fallback(def(f, "g"), "MATVIEW_NOT_INCREMENTAL".into());
        let s = view_status::snapshot()
            .into_iter()
            .find(|s| s.name == f)
            .expect("note_fallback publishes the view");
        assert_eq!(s.mode, ViewMode::Recompute);
        assert_eq!(s.reason_code.as_deref(), Some("MATVIEW_NOT_INCREMENTAL"));

        // drop removes the board entry.
        assert!(m.drop_view(name));
        assert!(find().is_none());
        view_status::remove(f);
    }
}
