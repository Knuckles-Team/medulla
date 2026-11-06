//! Retraction by delete and re-derive (DRed): removing axioms from a classified
//! [`Reasoner`] without a fresh run.
//!
//! Phase one over-deletes, through `dependents`, every derived subsumption and role
//! pair whose justification cites a removed axiom or an over-deleted premise. Phase
//! two re-runs the completion seeded with the sources of the over-deleted entries
//! only, so whatever still follows from the remaining axioms is derived again.

use std::collections::{BTreeSet, HashSet};

use super::{
    cause_key, iri, parse_ontology, Classification, Justification, Ontology, Reasoner, Triple,
    OWL_NOTHING,
};

type Entry = (String, String);

impl Reasoner {
    /// Remove the axioms stated by `removed` and return the classification of the
    /// remaining axioms (equal to [`Reasoner::from_triples`] over them, then
    /// `classify`). Cost follows the over-deleted region, except when the reasoner has
    /// not classified yet, has introduced refined fillers, or loses a role axiom:
    /// those rebuild.
    pub fn retract_axioms(&mut self, removed: &[Triple]) -> Classification {
        let gone = parse_ontology(removed);
        // Role pairs propagate up the role hierarchy and along chains from
        // pairs the over-delete never revisits, so removing a role axiom
        // rebuilds rather than re-deriving the touched region only.
        let role_axiom = !gone.sub_roles.is_empty() || !gone.chains.is_empty();
        let labels = self.remove_axioms(gone);
        if self.s.is_empty() || !self.refined.is_empty() || role_axiom {
            return self.rebuild();
        }
        let overdeleted = self.overdelete(labels);
        let mut sources = self.forget(&overdeleted);
        self.drop_unnamed_classes(&mut sources);
        self.rederive(&sources);
        self.snapshot()
    }

    /// Drop `gone` from the stored ontology; the labels of the removed axioms.
    fn remove_axioms(&mut self, gone: Ontology) -> BTreeSet<String> {
        let mut labels: BTreeSet<String> = BTreeSet::new();
        labels.extend(gone.gcis.iter().map(|g| g.label.clone()));
        labels.extend(gone.sub_roles.iter().map(|r| r.2.clone()));
        labels.extend(gone.chains.iter().map(|c| c.label.clone()));
        labels.extend(gone.disjoint.iter().map(|d| d.2.clone()));
        labels.extend(gone.all_values.iter().map(|v| v.3.clone()));
        let ont = &mut self.ont;
        ont.gcis.retain(|g| !labels.contains(&g.label));
        ont.sub_roles.retain(|r| !labels.contains(&r.2));
        ont.chains.retain(|c| !labels.contains(&c.label));
        ont.disjoint.retain(|d| !labels.contains(&d.2));
        ont.all_values.retain(|v| !labels.contains(&v.3));
        ont.inverses.retain(|p| !gone.inverses.contains(p));
        ont.domains.retain(|p| !gone.domains.contains(p));
        ont.ranges.retain(|p| !gone.ranges.contains(p));
        ont.same_as.retain(|p| !gone.same_as.contains(p));
        ont.different_from
            .retain(|p| !gone.different_from.contains(p));
        ont.symmetric.retain(|r| !gone.symmetric.contains(r));
        ont.functional.retain(|r| !gone.functional.contains(r));
        ont.inverse_functional
            .retain(|r| !gone.inverse_functional.contains(r));
        ont.classes.retain(|c| !gone.classes.contains(c));
        labels
    }

    /// Recompute everything from the remaining axioms.
    fn rebuild(&mut self) -> Classification {
        let weighted = self.weighted;
        *self = Reasoner::new(std::mem::take(&mut self.ont));
        if weighted {
            self.classify_weighted()
        } else {
            self.classify()
        }
    }

    /// Phase one: every entry transitively citing one of `labels`, in discovery order.
    fn overdelete(&mut self, labels: BTreeSet<String>) -> Vec<Entry> {
        let mut seen: HashSet<Entry> = HashSet::new();
        let mut found: Vec<Entry> = Vec::new();
        let mut work: Vec<String> = labels.into_iter().collect();
        while let Some(cause) = work.pop() {
            for derived in self.dependents.remove(&cause).unwrap_or_default() {
                if seen.insert(derived.clone()) {
                    work.push(cause_key(&derived));
                    found.push(derived);
                }
            }
        }
        found
    }

    /// Delete the over-deleted entries from the closure; the classes that were their
    /// source (`A` of `A ⊑ B` or of `(A, B) ∈ R(r)`).
    fn forget(&mut self, entries: &[Entry]) -> BTreeSet<String> {
        let mut sources = BTreeSet::new();
        for entry in entries {
            match entry.0.strip_prefix("R:") {
                Some(role) => self.forget_role_pair(role, &entry.1, &mut sources),
                None => {
                    if let Some(set) = self.s.get_mut(&entry.0) {
                        set.remove(&entry.1);
                    }
                    self.conf.remove(entry);
                    sources.insert(entry.0.clone());
                }
            }
            if let Some(old) = self.just.remove(entry) {
                self.unindex(entry, &old);
            }
        }
        sources
    }

    /// Delete the role pair rendered `a->b` from `R(role)`.
    fn forget_role_pair(&mut self, role: &str, pair: &str, sources: &mut BTreeSet<String>) {
        let Some((a, b)) = pair.split_once("->") else {
            return;
        };
        if let Some(set) = self.r.get_mut(role) {
            set.remove(&(a.to_string(), b.to_string()));
        }
        self.rconf
            .remove(&(role.to_string(), a.to_string(), b.to_string()));
        sources.insert(a.to_string());
    }

    /// Remove `entry` from the lists of the axioms and premises its old justification cited.
    fn unindex(&mut self, entry: &Entry, old: &Justification) {
        let causes = old
            .axioms
            .iter()
            .cloned()
            .chain(old.premises.iter().map(cause_key));
        for cause in causes {
            if let Some(list) = self.dependents.get_mut(&cause) {
                list.retain(|d| d != entry);
            }
        }
    }

    /// Forget classes the remaining axioms no longer mention (never ⊥ or ⊤ seeds).
    fn drop_unnamed_classes(&mut self, sources: &mut BTreeSet<String>) {
        let signature = self.signature();
        let nothing = iri(OWL_NOTHING);
        let gone: Vec<String> = self
            .s
            .keys()
            .filter(|c| !signature.contains(*c) && **c != nothing)
            .cloned()
            .collect();
        for class in &gone {
            self.s.remove(class);
            sources.remove(class);
            self.conf.retain(|k, _| k.0 != *class);
        }
    }

    /// Phase two: run the completion with only the sources' subsumers fresh.
    fn rederive(&mut self, sources: &BTreeSet<String>) {
        self.seed();
        self.fresh = sources
            .iter()
            .filter_map(|a| self.s.get(a).map(|members| (a.clone(), members.clone())))
            .collect();
        self.run_completion();
    }
}
