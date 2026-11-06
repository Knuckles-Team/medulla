//! Plan-backed materialized-view MANAGER (CONCEPT:MEDULLA-KG.storage.plan-backed-matview).
//!
//! Holds the in-RAM registry of named, plan-backed materialized views + their freshness
//! state. A subsystem-local process singleton ([`manager`]) so BOTH the CDC hub
//! (`CdcHub::emit` → [`note_change`]) and the dispatch handler reach it WITHOUT threading
//! a new field through the shared `ServerState` (owned by another lane).

use std::collections::HashMap;
use std::sync::OnceLock;

use medulla_exec::incremental::Circuit;
use medulla_types::delta::Delta;
use parking_lot::Mutex;

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
    circuit: Option<Circuit>,
    /// The typed reason this view fell back to `Mode::Recompute` (the first unsupported
    /// op) — first-class + queryable (MEDULLA-INCREMENTAL-VIEWS-R001), never a
    /// silent drop. `None` for an `Incremental` view or a boot-reloaded view.
    fallback_reason: Option<String>,
    /// CDC watermark — the next `seq` this view expects (mirrors
    /// `cdc::ContinuousQuery::through_seq`). Deltas with `seq < through_seq` are already
    /// folded and skipped.
    through_seq: u64,
    /// Changes applied since the last shadow check (MEDULLA-INCREMENTAL-VIEWS-R008.1).
    since_check: u64,
    /// Set once `since_check` reaches `view.shadow_every`; cleared when the check is taken.
    check_due: bool,
}

impl Tracked {
    /// Count one applied change; the check is due once `every` have accumulated.
    fn note_applied(&mut self, every: u64) {
        self.since_check += 1;
        if self.since_check >= every {
            self.check_due = true;
        }
    }
}

/// The process-global plan-backed matview registry.
#[derive(Default)]
pub struct PlanMatViewManager {
    views: Mutex<HashMap<String, Tracked>>,
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
        self.views.lock().insert(
            def.name.clone(),
            Tracked {
                def,
                stale: false,
                mode: Mode::Recompute,
                circuit: None,
                fallback_reason,
                through_seq: 0,
                since_check: 0,
                check_due: false,
            },
        );
    }

    /// Install (or replace) a view in `Mode::Incremental` with its compiled + seeded
    /// `circuit` and the CDC `through_seq` watermark captured at definition time
    /// (MEDULLA-INCREMENTAL-VIEWS-R001). The circuit's maintained state is the
    /// authoritative result; `Get` projects it lazily.
    pub fn install_incremental(&self, def: PlanMatView, circuit: Circuit, through_seq: u64) {
        self.views.lock().insert(
            def.name.clone(),
            Tracked {
                def,
                stale: false,
                mode: Mode::Incremental,
                circuit: Some(circuit),
                fallback_reason: None,
                through_seq,
                since_check: 0,
                check_due: false,
            },
        );
    }

    /// The maintenance mode of `name`, if tracked.
    pub fn mode(&self, name: &str) -> Option<Mode> {
        self.views.lock().get(name).map(|t| t.mode)
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
        let views = self.views.lock();
        let t = views.get(name)?;
        if t.mode != Mode::Incremental {
            return None;
        }
        let circuit = t.circuit.as_ref()?;
        Some(
            circuit
                .current()
                .rows()
                .iter()
                .map(|r| (r.id.clone(), r.score))
                .collect(),
        )
    }

    /// A snapshot of an `Incremental` view's compiled circuit (for durable operator-state
    /// persistence). `None` for unknown/`Recompute` views.
    pub fn circuit_snapshot(&self, name: &str) -> Option<Circuit> {
        self.views.lock().get(name).and_then(|t| t.circuit.clone())
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
        let mut demoted: Vec<(PlanMatView, String)> = Vec::new();
        let shadow_every = medulla_server_api::knob_config::knobs().view_shadow_every;
        let mut n = 0;
        {
            let mut views = self.views.lock();
            for t in views.values_mut() {
                if t.mode != Mode::Incremental || t.def.graph != graph {
                    continue;
                }
                if seq < t.through_seq {
                    continue;
                }
                if let Some(circuit) = t.circuit.as_mut() {
                    // O(delta) maintenance: fold the signed delta into the circuit's state
                    // ONLY (no O(view) result projection here — `Get` projects `current()`
                    // lazily). `touched` is the rows-touched instrument (bounded by the
                    // delta size, independent of view size — the O(delta) cost proof).
                    let touched = if delta.is_empty() {
                        0
                    } else {
                        match circuit.apply(delta) {
                            Ok(touched) => touched,
                            Err(err) => {
                                demoted.push((t.def.clone(), err.code().as_str().to_string()));
                                continue;
                            }
                        }
                    };
                    t.through_seq = seq + 1;
                    t.note_applied(shadow_every);
                    n += 1;
                    tracing::trace!(
                        target: "medulla::matview",
                        view = %t.def.name,
                        graph = %graph,
                        seq,
                        delta = delta.len(),
                        touched,
                        "incremental matview maintained one CDC delta",
                    );
                }
            }
        }
        for (def, reason) in demoted {
            self.demote(def, reason);
        }
        n
    }

    /// Install `def` in `Mode::Recompute` with the typed `reason`, marked stale so the next
    /// read recomputes it.
    fn demote(&self, def: PlanMatView, reason: String) {
        let name = def.name.clone();
        self.insert_recompute(def, Some(reason));
        if let Some(t) = self.views.lock().get_mut(&name) {
            t.stale = true;
        }
    }

    /// Demote the incremental view `name` to recompute mode with the typed `code`
    /// (MEDULLA-INCREMENTAL-VIEWS-R008.2). `false` when it is not an incremental view.
    pub fn demote_incremental(&self, name: &str, code: &str) -> bool {
        let Some(def) = self
            .get(name)
            .filter(|_| self.mode(name) == Some(Mode::Incremental))
        else {
            return false;
        };
        self.demote(def, code.to_string());
        true
    }

    /// Take up to `limit` incremental views whose shadow check is due, clearing their due
    /// flag and counter (MEDULLA-INCREMENTAL-VIEWS-R008.1).
    pub fn take_due_checks(&self, limit: usize) -> Vec<PlanMatView> {
        let mut views = self.views.lock();
        let mut due: Vec<&mut Tracked> = views
            .values_mut()
            .filter(|t| t.mode == Mode::Incremental && t.check_due)
            .collect();
        due.sort_by(|a, b| a.def.name.cmp(&b.def.name));
        due.into_iter()
            .take(limit)
            .map(|t| {
                t.check_due = false;
                t.since_check = 0;
                t.def.clone()
            })
            .collect()
    }

    /// Mark the check of `name` due again (a skipped check is retried on the next tick).
    pub fn rearm_check(&self, name: &str) {
        if let Some(t) = self.views.lock().get_mut(name) {
            t.check_due = true;
        }
    }

    /// Drop one member of the maintained state of `name` so a shadow check sees a divergence.
    #[cfg(feature = "test-support")]
    pub fn corrupt_for_test(&self, name: &str) -> bool {
        let mut views = self.views.lock();
        views
            .get_mut(name)
            .and_then(|t| t.circuit.as_mut())
            .is_some_and(Circuit::drop_one_member_for_test)
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

    /// Drop a view's tracking; returns whether it existed.
    pub fn drop_view(&self, name: &str) -> bool {
        self.views.lock().remove(name).is_some()
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

    // spec: MEDULLA-INCREMENTAL-VIEWS-R008.1
    #[test]
    fn check_becomes_due_after_knob_changes() {
        use medulla_types::wire::{Op, Plan};
        let every = medulla_server_api::knob_config::knobs().view_shadow_every;
        let plan = Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]);
        let m = PlanMatViewManager::default();
        for name in ["a", "b"] {
            let circuit = Circuit::compile(&plan).unwrap();
            let def = PlanMatView {
                name: name.into(),
                graph: "g".into(),
                plan: plan.clone(),
            };
            m.install_incremental(def, circuit, 0);
        }
        let empty = Delta::from(Vec::new());
        for seq in 0..every - 1 {
            m.apply_signed("g", seq, &empty);
        }
        assert!(m.take_due_checks(4).is_empty(), "not due before the knob");
        m.apply_signed("g", every - 1, &empty);
        let first = m.take_due_checks(1);
        assert_eq!(first.len(), 1, "the limit bounds one take");
        assert_eq!(m.take_due_checks(4).len(), 1, "the other view is still due");
        assert!(m.take_due_checks(4).is_empty(), "a taken check is cleared");
        m.rearm_check(&first[0].name);
        assert_eq!(m.take_due_checks(4).len(), 1, "a skipped check is re-armed");
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
}
