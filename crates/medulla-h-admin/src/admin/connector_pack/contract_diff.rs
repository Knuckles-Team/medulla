//! Pack release contract diff and the release history it drives
//! (MEDULLA-TYPED-PACKS-R107.2).
//!
//! `diff_release` is a pure function over two release contracts: it reuses the
//! axiom diff of `GraphSchema.DiffOntology` (`OntologyDiff`, R804) for classes
//! and properties, then compares shape constraints, declared field types and
//! the mapping set. A release with no `ContractBreak` applies and supersedes
//! the active one. A release with any break is stored pending behind a change
//! request (`medulla_core::stewardship::open_change_request`, R802.1) and the
//! active version keeps serving. Every status change goes through the
//! explicit `PackReleaseMachine` (`ReleaseStatus::apply`); `StoredRelease::apply`
//! is the only writer of `StoredRelease.status`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::connector_pack::contract_version::{
    ContractBreak, ContractBreakKind, PackContractError, PackContractVersion,
};
use medulla_core::stewardship::{open_change_request, StewardshipError, TargetScopes};
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};
use medulla_catalog_types::ontology_diff::OntologyDiff;
use medulla_catalog_types::stewardship::{ChangeRequest, StewardshipAssignment};
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use crate::graph_schema::ontology_diff::{diff_axioms, SchemaSide};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const SH: &str = "http://www.w3.org/ns/shacl#";
const CLASS_TYPES: [&str; 2] = [
    "http://www.w3.org/2002/07/owl#Class",
    "http://www.w3.org/2000/01/rdf-schema#Class",
];
const PROPERTY_TYPES: [&str; 4] = [
    "http://www.w3.org/2002/07/owl#ObjectProperty",
    "http://www.w3.org/2002/07/owl#DatatypeProperty",
    "http://www.w3.org/2002/07/owl#AnnotationProperty",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property",
];
/// Longest `sh:in` list the shape comparison walks.
const MAX_LIST_ITEMS: usize = 1_000;

/// Everything of one pack release that the contract covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseContract {
    pub version: PackContractVersion,
    pub ontology_ttl: String,
    pub shapes_ttl: Option<String>,
    /// Declared tool and prompt field path to its declared type.
    pub fields: BTreeMap<String, String>,
    /// Ids of the pack's mappings.
    pub mappings: BTreeSet<String>,
}

/// Why a diff or a history operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractDiffError {
    /// A pack document did not parse.
    Document(String),
    /// A release record failed validation.
    Record(PackContractError),
    /// No owner or steward could review the change request.
    Stewardship(StewardshipError),
    /// An event is not legal for the release's status.
    Transition(TransitionRefused),
    /// The version named is not in the history.
    UnknownVersion(String),
    /// The version is already in the history.
    DuplicateVersion(String),
}

impl std::fmt::Display for ContractDiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Document(error) => f.write_str(error),
            Self::Record(error) => error.fmt(f),
            Self::Stewardship(error) => error.fmt(f),
            Self::Transition(error) => error.fmt(f),
            Self::UnknownVersion(v) => write!(f, "UNKNOWN_VERSION: {v}"),
            Self::DuplicateVersion(v) => write!(f, "DUPLICATE_VERSION: {v}"),
        }
    }
}

impl std::error::Error for ContractDiffError {}

/// The closed set of release statuses: the states of `PackReleaseMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReleaseStatus {
    /// Stored behind a change request; not served.
    Pending,
    /// The version queries resolve through.
    Active,
    /// Replaced by a later active version; readable by revision.
    Superseded,
    /// The change request was rejected; never served.
    Rejected,
}

/// The closed set of events that move a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReleaseEvent {
    Activate,
    Reject,
    Supersede,
}

impl MachineState for ReleaseStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Rejected => "rejected",
        }
    }
}

impl MachineEvent for ReleaseEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Activate => "Activate",
            Self::Reject => "Reject",
            Self::Supersede => "Supersede",
        }
    }
}

/// `PackReleaseMachine`: the closed edge table. Every other pair is refused.
pub static PACK_RELEASE_MACHINE: MachineDef<ReleaseStatus, ReleaseEvent, &'static str> =
    MachineDef {
        name: "PackReleaseMachine",
        states: &[
            ReleaseStatus::Pending,
            ReleaseStatus::Active,
            ReleaseStatus::Superseded,
            ReleaseStatus::Rejected,
        ],
        initial: ReleaseStatus::Pending,
        terminals: &[ReleaseStatus::Superseded, ReleaseStatus::Rejected],
        events: &[
            ReleaseEvent::Activate,
            ReleaseEvent::Reject,
            ReleaseEvent::Supersede,
        ],
        guards: &[],
        edges: &[
            Edge {
                id: "P01",
                from: ReleaseStatus::Pending,
                event: ReleaseEvent::Activate,
                guard: None,
                to: ReleaseStatus::Active,
            },
            Edge {
                id: "P02",
                from: ReleaseStatus::Pending,
                event: ReleaseEvent::Reject,
                guard: None,
                to: ReleaseStatus::Rejected,
            },
            Edge {
                id: "P03",
                from: ReleaseStatus::Active,
                event: ReleaseEvent::Supersede,
                guard: None,
                to: ReleaseStatus::Superseded,
            },
        ],
    };

impl ReleaseStatus {
    /// The one transition function of `PackReleaseMachine`.
    pub fn apply(self, event: ReleaseEvent) -> Result<Self, TransitionRefused> {
        transition(&PACK_RELEASE_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

fn parse(label: &str, document: &str) -> Result<Vec<Triple>, ContractDiffError> {
    medulla_rdf::mapping::parse_turtle_bounded(
        document,
        medulla_graph_core::graph::MAX_SCHEMA_DOCUMENT_TRIPLES,
    )
    .map_err(|error| ContractDiffError::Document(format!("ONTOLOGY_INVALID: {label}: {error}")))
}

fn subject_axioms(triples: &[Triple]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for triple in triples {
        if let NamedOrBlankNode::NamedNode(subject) = &triple.subject {
            let object = match &triple.object {
                Term::BlankNode(_) => "_:".to_string(),
                other => other.to_string(),
            };
            out.entry(subject.as_str().to_string())
                .or_default()
                .insert(format!("{} {}", triple.predicate.as_str(), object));
        }
    }
    out
}

fn has_type(axioms: &BTreeSet<String>, types: &[&str]) -> bool {
    types
        .iter()
        .any(|t| axioms.contains(&format!("{RDF_TYPE} <{t}>")))
}

fn with_predicate<'a>(axioms: &'a BTreeSet<String>, predicate: &str) -> BTreeSet<&'a String> {
    let prefix = format!("{predicate} ");
    axioms.iter().filter(|a| a.starts_with(&prefix)).collect()
}

fn brk(kind: ContractBreakKind, item: &str) -> ContractBreak {
    ContractBreak {
        kind,
        item: item.to_string(),
    }
}

fn ontology_breaks(
    prior: &ReleaseContract,
    next: &ReleaseContract,
) -> Result<Vec<ContractBreak>, ContractDiffError> {
    let side = |c: &ReleaseContract| SchemaSide {
        revision: c.version.version.clone(),
        ontology_ttl: c.ontology_ttl.clone(),
        shapes_ttl: c.shapes_ttl.clone(),
    };
    let diff: OntologyDiff =
        diff_axioms(&side(prior), &side(next)).map_err(ContractDiffError::Document)?;
    let before = subject_axioms(&parse("prior ontology", &prior.ontology_ttl)?);
    let after = subject_axioms(&parse("next ontology", &next.ontology_ttl)?);
    let mut out = Vec::new();
    let empty = BTreeSet::new();
    for iri in &diff.removed {
        let axioms = before.get(iri).unwrap_or(&empty);
        if has_type(axioms, &CLASS_TYPES) {
            out.push(brk(ContractBreakKind::RemovedClass, iri));
        } else if has_type(axioms, &PROPERTY_TYPES) {
            out.push(brk(ContractBreakKind::RemovedProperty, iri));
        }
    }
    for iri in &diff.changed {
        let (old, new) = (
            before.get(iri).unwrap_or(&empty),
            after.get(iri).unwrap_or(&empty),
        );
        let retyped = with_predicate(old, RDF_TYPE) != with_predicate(new, RDF_TYPE);
        if has_type(old, &CLASS_TYPES) && retyped {
            out.push(brk(ContractBreakKind::RetypedClass, iri));
        } else if has_type(old, &PROPERTY_TYPES)
            && (retyped || with_predicate(old, RDFS_RANGE) != with_predicate(new, RDFS_RANGE))
        {
            out.push(brk(ContractBreakKind::RetypedProperty, iri));
        }
    }
    Ok(out)
}

fn term_text(term: &Term) -> String {
    match term {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        other => other.to_string(),
    }
}

fn term_key(term: &Term) -> Option<String> {
    match term {
        Term::NamedNode(n) => Some(format!("<{}>", n.as_str())),
        Term::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        // A literal or a quoted triple is never an RDF list cell.
        _ => None,
    }
}

type Edges = BTreeMap<String, Vec<(String, Term)>>;

fn list_items(by_subject: &Edges, head: &Term) -> Vec<String> {
    let mut items = Vec::new();
    let mut cursor = term_key(head);
    while let Some(key) = cursor.take() {
        if items.len() >= MAX_LIST_ITEMS || key == format!("<{RDF_NIL}>") {
            break;
        }
        let Some(edges) = by_subject.get(&key) else {
            break;
        };
        for (predicate, object) in edges {
            if predicate == RDF_FIRST {
                items.push(term_text(object));
            } else if predicate == RDF_REST {
                cursor = term_key(object);
            }
        }
    }
    items
}

/// `(shape iri, property path)` to constraint local name to values.
type ShapeConstraints = BTreeMap<(String, String), BTreeMap<String, BTreeSet<String>>>;

fn shape_constraints(ttl: Option<&str>) -> Result<ShapeConstraints, ContractDiffError> {
    let Some(ttl) = ttl else {
        return Ok(BTreeMap::new());
    };
    let triples = parse("shapes", ttl)?;
    let mut by_subject: Edges = BTreeMap::new();
    for triple in &triples {
        by_subject
            .entry(triple.subject.to_string())
            .or_default()
            .push((triple.predicate.as_str().to_string(), triple.object.clone()));
    }
    let mut out = ShapeConstraints::new();
    let sh_property = format!("{SH}property");
    let sh_path = format!("{SH}path");
    for (subject, edges) in &by_subject {
        let Some(shape) = subject
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .map(str::to_string)
        else {
            continue;
        };
        let mut blocks: Vec<(String, &Vec<(String, Term)>)> = vec![(String::new(), edges)];
        for (predicate, object) in edges {
            if *predicate != sh_property {
                continue;
            }
            let Some(block) = term_key(object).and_then(|k| by_subject.get(&k)) else {
                continue;
            };
            let path = block
                .iter()
                .find(|(p, _)| *p == sh_path)
                .map(|(_, o)| term_text(o))
                .unwrap_or_default();
            blocks.push((path, block));
        }
        for (path, block) in blocks {
            let entry = out.entry((shape.clone(), path)).or_default();
            for (predicate, object) in block {
                let Some(local) = predicate.strip_prefix(SH) else {
                    continue;
                };
                if matches!(
                    local,
                    "property" | "path" | "name" | "description" | "message"
                ) {
                    continue;
                }
                let values = entry.entry(local.to_string()).or_default();
                if local == "in" {
                    values.extend(list_items(&by_subject, object));
                } else {
                    values.insert(term_text(object));
                }
            }
        }
    }
    Ok(out)
}

fn extreme(values: &BTreeSet<String>, want_max: bool) -> Option<f64> {
    let nums = values.iter().filter_map(|v| v.parse::<f64>().ok());
    if want_max {
        nums.fold(None, |a, v| Some(a.map_or(v, |a: f64| a.max(v))))
    } else {
        nums.fold(None, |a, v| Some(a.map_or(v, |a: f64| a.min(v))))
    }
}

/// Whether `next` admits fewer values than `prior` for one constraint kind.
fn narrows(local: &str, prior: &BTreeSet<String>, next: &BTreeSet<String>) -> bool {
    match local {
        "minCount" | "minLength" => {
            extreme(next, true).is_some_and(|n| n > extreme(prior, true).unwrap_or(0.0))
        }
        "minInclusive" | "minExclusive" => extreme(next, true)
            .is_some_and(|n| n > extreme(prior, true).unwrap_or(f64::NEG_INFINITY)),
        "maxCount" | "maxLength" | "maxInclusive" | "maxExclusive" => {
            extreme(next, false).is_some_and(|n| n < extreme(prior, false).unwrap_or(f64::INFINITY))
        }
        "in" => {
            if prior.is_empty() {
                !next.is_empty()
            } else {
                prior.difference(next).next().is_some()
            }
        }
        _ => next.difference(prior).next().is_some(),
    }
}

fn shape_breaks(
    prior: &ReleaseContract,
    next: &ReleaseContract,
) -> Result<Vec<ContractBreak>, ContractDiffError> {
    let before = shape_constraints(prior.shapes_ttl.as_deref())?;
    let after = shape_constraints(next.shapes_ttl.as_deref())?;
    let none = BTreeMap::new();
    let none_set = BTreeSet::new();
    let mut out = Vec::new();
    for ((shape, path), constraints) in &after {
        let old = before.get(&(shape.clone(), path.clone())).unwrap_or(&none);
        let narrowed = constraints
            .iter()
            .any(|(local, values)| narrows(local, old.get(local).unwrap_or(&none_set), values));
        if narrowed {
            out.push(brk(
                ContractBreakKind::NarrowedShape,
                &format!("{shape}#{path}"),
            ));
        }
    }
    Ok(out)
}

fn field_breaks(prior: &ReleaseContract, next: &ReleaseContract) -> Vec<ContractBreak> {
    let mut out = Vec::new();
    for (path, declared) in &prior.fields {
        match next.fields.get(path) {
            None => out.push(brk(ContractBreakKind::RemovedField, path)),
            Some(now) if now != declared => out.push(brk(ContractBreakKind::RetypedField, path)),
            Some(_) => {}
        }
    }
    out
}

/// Every breaking change `next` makes to `prior`, sorted and without repeats.
/// No break means the release is additive and may apply at once.
pub fn diff_release(
    prior: &ReleaseContract,
    next: &ReleaseContract,
) -> Result<Vec<ContractBreak>, ContractDiffError> {
    let mut out = ontology_breaks(prior, next)?;
    out.extend(shape_breaks(prior, next)?);
    out.extend(field_breaks(prior, next));
    out.extend(
        prior
            .mappings
            .difference(&next.mappings)
            .map(|id| brk(ContractBreakKind::RemovedMapping, id)),
    );
    out.sort_by(|a, b| (a.kind as u8, &a.item).cmp(&(b.kind as u8, &b.item)));
    out.dedup();
    Ok(out)
}

/// One release as stored, with the status only [`StoredRelease::apply`] writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRelease {
    pub contract: ReleaseContract,
    status: ReleaseStatus,
    /// Every break found at submission.
    pub breaks: Vec<ContractBreak>,
    /// The change request holding a breaking release.
    pub change_request: Option<ChangeRequest>,
}

impl StoredRelease {
    /// A release in the machine's initial `pending` state.
    fn new(
        contract: ReleaseContract,
        breaks: Vec<ContractBreak>,
        change_request: Option<ChangeRequest>,
    ) -> Self {
        Self {
            contract,
            status: PACK_RELEASE_MACHINE.initial,
            breaks,
            change_request,
        }
    }

    pub fn status(&self) -> ReleaseStatus {
        self.status
    }

    /// Applies one event; the only writer of the status. An illegal event is
    /// the typed refusal naming the machine, the current state and the event.
    fn apply(&mut self, event: ReleaseEvent) -> Result<(), TransitionRefused> {
        self.status = self.status.apply(event)?;
        Ok(())
    }
}

/// What a submitted release did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseDisposition {
    /// Applied at once; `supersedes` is the version it replaced.
    Applied { supersedes: Option<String> },
    /// Held behind a change request; the active version keeps serving. The
    /// breaks are the first `catalog.contract_break_report_max`.
    Pending {
        breaks: Vec<ContractBreak>,
        total_breaks: usize,
        change_request: ChangeRequest,
    },
}

/// The releases of one connector pack, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContractHistory {
    releases: Vec<StoredRelease>,
}

impl ContractHistory {
    /// The release queries resolve through.
    pub fn active(&self) -> Option<&StoredRelease> {
        self.releases
            .iter()
            .find(|r| r.status == ReleaseStatus::Active)
    }

    /// A release by revision, whatever its status (removed surfaces stay
    /// readable by revision).
    pub fn by_version(&self, version: &str) -> Option<&StoredRelease> {
        self.releases
            .iter()
            .find(|r| r.contract.version.version == version)
    }

    pub fn releases(&self) -> &[StoredRelease] {
        &self.releases
    }

    fn index_of(&self, version: &str) -> Result<usize, ContractDiffError> {
        self.releases
            .iter()
            .position(|r| r.contract.version.version == version)
            .ok_or_else(|| ContractDiffError::UnknownVersion(version.to_string()))
    }

    fn retire_active(&mut self) -> Result<(), ContractDiffError> {
        for release in &mut self.releases {
            if release.status == ReleaseStatus::Active {
                release
                    .apply(ReleaseEvent::Supersede)
                    .map_err(ContractDiffError::Transition)?;
            }
        }
        Ok(())
    }

    /// Diff `next` against the active release and apply or hold it.
    pub fn submit(
        &mut self,
        mut next: ReleaseContract,
        assignments: &[StewardshipAssignment],
        scopes: &TargetScopes,
    ) -> Result<ReleaseDisposition, ContractDiffError> {
        if self.by_version(&next.version.version).is_some() {
            return Err(ContractDiffError::DuplicateVersion(next.version.version));
        }
        let active = self.active().map(|r| r.contract.clone());
        let breaks = match &active {
            Some(prior) => diff_release(prior, &next)?,
            None => Vec::new(),
        };
        next.version.supersedes = active.as_ref().map(|p| p.version.version.clone());
        next.version.validate().map_err(ContractDiffError::Record)?;
        for found in &breaks {
            found.validate().map_err(ContractDiffError::Record)?;
        }
        if breaks.is_empty() {
            let supersedes = next.version.supersedes.clone();
            let mut stored = StoredRelease::new(next, breaks, None);
            stored
                .apply(ReleaseEvent::Activate)
                .map_err(ContractDiffError::Transition)?;
            self.retire_active()?;
            self.releases.push(stored);
            return Ok(ReleaseDisposition::Applied { supersedes });
        }
        let target = format!("pack:{}", next.version.connector);
        let change_request =
            open_change_request(&target, &next.version.contract_digest, assignments, scopes)
                .map_err(ContractDiffError::Stewardship)?;
        let cap = usize::try_from(
            medulla_server_api::knob_config::knobs().catalog_contract_break_report_max,
        )
        .unwrap_or(usize::MAX);
        let total_breaks = breaks.len();
        let reported = breaks.iter().take(cap).cloned().collect();
        self.releases.push(StoredRelease::new(
            next,
            breaks,
            Some(change_request.clone()),
        ));
        Ok(ReleaseDisposition::Pending {
            breaks: reported,
            total_breaks,
            change_request,
        })
    }

    /// A pending release whose change request was approved becomes active and
    /// supersedes the one that was serving.
    pub fn approve(&mut self, version: &str) -> Result<(), ContractDiffError> {
        let index = self.index_of(version)?;
        let mut probe = self.releases[index].clone();
        probe
            .apply(ReleaseEvent::Activate)
            .map_err(ContractDiffError::Transition)?;
        probe.contract.version.supersedes =
            self.active().map(|r| r.contract.version.version.clone());
        probe
            .contract
            .version
            .validate()
            .map_err(ContractDiffError::Record)?;
        self.retire_active()?;
        self.releases[index] = probe;
        Ok(())
    }

    /// A pending release whose change request was rejected is never served.
    pub fn reject(&mut self, version: &str) -> Result<(), ContractDiffError> {
        let index = self.index_of(version)?;
        self.releases[index]
            .apply(ReleaseEvent::Reject)
            .map_err(ContractDiffError::Transition)
    }
}

#[cfg(test)]
mod tests;
