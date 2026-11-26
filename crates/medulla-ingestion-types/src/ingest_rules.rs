//! Pure admission and refusal rules for the ingest worker's typed records.
//!
//! Every function here decides before any write: a refusal is a value, so a
//! refused item can never leave partial state behind. The served wiring that
//! calls these rules is carried by the rows that own the wire methods.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::extracted_fact::{CandidateClaim, ExtractedFact, GroundingResult};
use crate::knowledge_atom::KnowledgeAtom;
use crate::ontology_proposal::{
    score_proposal, OntologyProposal, ProposalCheck, ProposalState, ProposalThresholds,
};
use crate::source_governance::{Classification, RecordGovernance, SourceContainer, SourceGroup};

/// Deepest container nesting a source tree may carry.
pub const MAX_CONTAINER_DEPTH: usize = 32;
/// Most members one source group may hold.
pub const MAX_GROUP_MEMBERS: usize = 100_000;
/// Most `rdfs:subClassOf` steps the grounding cache follows from one class
/// (MEDULLA-INGEST-WORKER-R701.8).
pub const MAX_HIERARCHY_STEPS: usize = 64;
/// Share of a source's held records one reconcile plan may delete (percent).
pub const MASS_DELETE_PERCENT: usize = 20;

/// A stable refusal code with bounded detail.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IngestRefusal {
    #[error("SOURCE_GOVERNANCE_CONFLICT: classification {classification:?} disagrees with access_public={access_public}")]
    GovernanceConflict {
        classification: Classification,
        access_public: bool,
    },
    #[error("SOURCE_RETENTION_MISSING: retention is required by policy")]
    RetentionMissing,
    #[error("SOURCE_GOVERNANCE_INVALID: {0}")]
    GovernanceInvalid(String),
    #[error("ROW_VERSION_INVALID: {0}")]
    RowVersionInvalid(String),
    #[error("SOURCE_RECORD_UNKNOWN: {0}")]
    RecordUnknown(String),
    #[error("SOURCE_GROUP_TOO_LARGE: {members} members exceed {MAX_GROUP_MEMBERS}")]
    GroupTooLarge { members: usize },
    #[error("SOURCE_MASS_DELETE_GUARD: {missing} of {held} held records would be deleted")]
    MassDeleteGuard { missing: usize, held: usize },
    #[error("SOURCE_CONTAINER_CYCLE: {0}")]
    ContainerCycle(String),
    #[error("SOURCE_CONTAINER_TOO_DEEP: {0}")]
    ContainerTooDeep(String),
    #[error("PROPOSAL_NOT_ADMITTED: failed {0:?} check")]
    ProposalNotAdmitted(ProposalCheck),
    #[error("ATOM_PROVENANCE_MISSING: atom {0} has no provenance key")]
    AtomProvenanceMissing(String),
    #[error("ONTOLOGY_HIERARCHY_CYCLE: class {0} reaches itself or nests deeper than {MAX_HIERARCHY_STEPS} steps")]
    OntologyHierarchyCycle(String),
}

impl IngestRefusal {
    /// The stable wire code, the text before the first colon of the message.
    pub fn code(&self) -> String {
        let text = self.to_string();
        text.split(':').next().unwrap_or_default().to_string()
    }
}

impl RecordGovernance {
    /// Admit a record's governance, or refuse it before anything is written.
    /// A public classification must match `access_public`, and a policy that
    /// requires retention refuses a record that sent none.
    pub fn admit(&self, retention_required: bool) -> Result<(), IngestRefusal> {
        if (self.classification == Classification::Public) != self.access_public {
            return Err(IngestRefusal::GovernanceConflict {
                classification: self.classification,
                access_public: self.access_public,
            });
        }
        if retention_required && self.retention.is_none() {
            return Err(IngestRefusal::RetentionMissing);
        }
        Ok(())
    }
}

/// What a valid row version means for the held evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowVersionDecision {
    First,
    Supersedes,
    Replay,
}

/// Validate an incoming row version against the highest stored one. Only
/// non-negative integers are accepted, and a version may never decrease.
pub fn check_row_version(
    stored: Option<u64>,
    incoming: &serde_json::Value,
) -> Result<RowVersionDecision, IngestRefusal> {
    let version = incoming.as_u64().ok_or_else(|| {
        IngestRefusal::RowVersionInvalid(format!("version {incoming} is not an integer"))
    })?;
    match stored {
        None => Ok(RowVersionDecision::First),
        Some(held) if version > held => Ok(RowVersionDecision::Supersedes),
        Some(held) if version == held => Ok(RowVersionDecision::Replay),
        Some(held) => Err(IngestRefusal::RowVersionInvalid(format!(
            "version {version} is below stored version {held}"
        ))),
    }
}

/// Refuse an access update for a record the engine does not hold.
pub fn check_access_target(held: &BTreeSet<String>, record_ref: &str) -> Result<(), IngestRefusal> {
    if held.contains(record_ref) {
        Ok(())
    } else {
        Err(IngestRefusal::RecordUnknown(record_ref.to_string()))
    }
}

/// Refuse a group whose member set is past the bound.
pub fn check_group_size(group: &SourceGroup) -> Result<(), IngestRefusal> {
    if group.members.len() > MAX_GROUP_MEMBERS {
        return Err(IngestRefusal::GroupTooLarge {
            members: group.members.len(),
        });
    }
    Ok(())
}

/// Plan the deletions a source enumeration implies. An incomplete enumeration
/// never deletes, so it yields an empty plan. A plan that would delete more
/// than 20 percent of the held records is refused unless an admin overrides.
pub fn plan_reconcile(
    held: &BTreeSet<String>,
    enumerated: &BTreeSet<String>,
    complete: bool,
    admin_override: bool,
) -> Result<BTreeSet<String>, IngestRefusal> {
    if !complete {
        return Ok(BTreeSet::new());
    }
    let missing: BTreeSet<String> = held.difference(enumerated).cloned().collect();
    if !admin_override && missing.len() * 100 > held.len() * MASS_DELETE_PERCENT {
        return Err(IngestRefusal::MassDeleteGuard {
            missing: missing.len(),
            held: held.len(),
        });
    }
    Ok(missing)
}

/// Plan the deletions of an identifier-only enumeration from SHA-256 identifier
/// digests (MEDULLA-INGEST-WORKER-R903.2). `held` and `enumerated` are sorted
/// ascending without duplicates; one merge pass keeps memory at the two input
/// vectors plus the missing set, which is what makes 1M identifiers cheap.
/// Refusal and override rules are those of [`plan_reconcile`].
pub fn plan_reconcile_digests(
    held: &[[u8; 32]],
    enumerated: &[[u8; 32]],
    complete: bool,
    admin_override: bool,
) -> Result<Vec<[u8; 32]>, IngestRefusal> {
    if !complete {
        return Ok(Vec::new());
    }
    let mut missing = Vec::new();
    let mut next = 0usize;
    for digest in held {
        while next < enumerated.len() && enumerated[next] < *digest {
            next += 1;
        }
        if next >= enumerated.len() || enumerated[next] != *digest {
            missing.push(*digest);
        }
    }
    if !admin_override && missing.len() * 100 > held.len() * MASS_DELETE_PERCENT {
        return Err(IngestRefusal::MassDeleteGuard {
            missing: missing.len(),
            held: held.len(),
        });
    }
    Ok(missing)
}

/// Validate a container batch: no parent cycle and nesting within the bound.
/// A parent outside the batch is a root of this batch.
pub fn check_container_tree(containers: &[SourceContainer]) -> Result<(), IngestRefusal> {
    let parents: BTreeMap<&str, Option<&str>> = containers
        .iter()
        .map(|c| (c.container_id.as_str(), c.parent_id.as_deref()))
        .collect();
    for start in parents.keys() {
        let mut seen = HashSet::new();
        let mut cursor = Some(*start);
        let mut depth = 0usize;
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return Err(IngestRefusal::ContainerCycle((*start).to_string()));
            }
            depth += 1;
            if depth > MAX_CONTAINER_DEPTH {
                return Err(IngestRefusal::ContainerTooDeep((*start).to_string()));
            }
            cursor = parents.get(id).copied().flatten();
        }
    }
    Ok(())
}

/// The ontology cache the grounder consults: predicate to (domain, range)
/// classes, entity name to class, and each class to the transitive set of its
/// superclasses (MEDULLA-INGEST-WORKER-R701.8).
#[derive(Clone, Debug, Default)]
pub struct GroundingCache {
    pub predicates: BTreeMap<String, (String, String)>,
    pub classes: BTreeMap<String, String>,
    /// `subclass_of[C]` holds every strict superclass of `C`. A class with none is absent.
    pub subclass_of: BTreeMap<String, BTreeSet<String>>,
}

/// The transitive superclass sets of the direct `rdfs:subClassOf` edges `direct`
/// (class to its direct superclasses). A class that reaches itself, or a chain
/// longer than [`MAX_HIERARCHY_STEPS`], refuses `ONTOLOGY_HIERARCHY_CYCLE` before
/// any cache is built (MEDULLA-INGEST-WORKER-R701.8). A class with no superclass
/// is absent from the result.
pub fn subclass_closure(
    direct: &BTreeMap<String, BTreeSet<String>>,
) -> Result<BTreeMap<String, BTreeSet<String>>, IngestRefusal> {
    let mut closure = BTreeMap::new();
    for class in direct.keys() {
        let mut supers: BTreeSet<String> = BTreeSet::new();
        let mut frontier: Vec<&str> = vec![class.as_str()];
        for step in 1..=MAX_HIERARCHY_STEPS + 1 {
            let mut next: Vec<&str> = Vec::new();
            for node in &frontier {
                for parent in direct.get(*node).into_iter().flatten() {
                    if parent == class {
                        return Err(IngestRefusal::OntologyHierarchyCycle(class.clone()));
                    }
                    if supers.insert(parent.clone()) {
                        next.push(parent.as_str());
                    }
                }
            }
            if next.is_empty() {
                frontier.clear();
                break;
            }
            frontier = next;
            if step > MAX_HIERARCHY_STEPS {
                return Err(IngestRefusal::OntologyHierarchyCycle(class.clone()));
            }
        }
        if !supers.is_empty() {
            closure.insert(class.clone(), supers);
        }
    }
    Ok(closure)
}

impl GroundingCache {
    /// Whether the entity class `class` is the declared class `declared` or one of its
    /// subclasses.
    fn class_fits(&self, class: Option<&String>, declared: &str) -> bool {
        match class {
            Some(class) => {
                class == declared
                    || self
                        .subclass_of
                        .get(class)
                        .is_some_and(|supers| supers.contains(declared))
            }
            None => false,
        }
    }
}

/// Ground one fact. Direction is repaired when only the swapped pair satisfies
/// the predicate's domain and range; a fact that fits neither way stays an
/// ungrounded claim. An entity fits a declared class when its class is that
/// class or a subclass of it; a grounded result keeps the declared classes.
pub fn ground_fact(fact: ExtractedFact, cache: &GroundingCache) -> CandidateClaim {
    let grounding = ground(&fact, cache);
    CandidateClaim { fact, grounding }
}

fn ground(fact: &ExtractedFact, cache: &GroundingCache) -> GroundingResult {
    let Some((domain, range)) = cache.predicates.get(&fact.predicate) else {
        return GroundingResult::Ungrounded;
    };
    let subject = cache.classes.get(&fact.subject);
    let object = cache.classes.get(&fact.object);
    let fits = |s: Option<&String>, o: Option<&String>| {
        cache.class_fits(s, domain) && cache.class_fits(o, range)
    };
    if fits(subject, object) {
        grounded(domain, &fact.predicate, range, false)
    } else if fits(object, subject) {
        grounded(domain, &fact.predicate, range, true)
    } else {
        GroundingResult::Ungrounded
    }
}

fn grounded(domain: &str, property: &str, range: &str, repaired: bool) -> GroundingResult {
    GroundingResult::Grounded {
        subject_class: domain.to_string(),
        property: property.to_string(),
        object_class: range.to_string(),
        direction_repaired: repaired,
    }
}

/// Incremental duplicate filter: an O(1) hash-set probe per fact.
#[derive(Debug, Default)]
pub struct FactDeduper {
    seen: HashSet<(String, String, String)>,
}

impl FactDeduper {
    /// True when the fact is new; false when an equal triple was seen before.
    pub fn admit(&mut self, fact: &ExtractedFact) -> bool {
        self.seen.insert((
            fact.subject.clone(),
            fact.predicate.clone(),
            fact.object.clone(),
        ))
    }
}

/// The relationship of an inferred entity link (MEDULLA-INGEST-WORKER-R701.7).
pub const RELATED_TO: &str = "related_to";

/// Two entities that co-occur in enough atoms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityLink {
    /// The lesser entity name.
    pub first: String,
    /// The greater entity name.
    pub second: String,
    /// The atoms the two entities share, stored and new.
    pub shared_mentions: u64,
    /// The shared atoms already stored before this batch.
    pub previously_shared: u64,
}

/// Link entities that co-occur in at least `min_shared` atoms
/// (MEDULLA-INGEST-WORKER-R701.7). Each claim is one new atom whose entities are its
/// subject and object. `stored` holds, per ordered entity pair, the atoms already stored
/// from earlier batches, so a link forms as evidence accumulates across batches. Only a
/// pair a new claim touches is considered; a pair below the threshold gets no link. Links
/// come out sorted by entity pair.
pub fn infer_links(
    claims: &[CandidateClaim],
    stored: &BTreeMap<(String, String), u64>,
    min_shared: u64,
) -> Vec<EntityLink> {
    let mut counts: BTreeMap<(&str, &str), u64> = BTreeMap::new();
    for claim in claims {
        let (subject, object) = (claim.fact.subject.as_str(), claim.fact.object.as_str());
        if subject == object {
            continue;
        }
        let pair = if subject < object {
            (subject, object)
        } else {
            (object, subject)
        };
        *counts.entry(pair).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|((first, second), added)| {
            let previously_shared = stored
                .get(&(first.to_string(), second.to_string()))
                .copied()
                .unwrap_or(0);
            EntityLink {
                first: first.to_string(),
                second: second.to_string(),
                shared_mentions: previously_shared + added,
                previously_shared,
            }
        })
        .filter(|link| link.shared_mentions >= min_shared)
        .collect()
}

/// The result of judging an induced proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionDecision {
    pub state: ProposalState,
    /// Instances may populate against a proposal only once it is admitted.
    pub instances_allowed: bool,
}

/// Judge a proposal. A failing check refuses with `PROPOSAL_NOT_ADMITTED` and
/// the failing check; the caller leaves the proposal a draft.
pub fn judge_proposal(
    proposal: &OntologyProposal,
    observed: &[String],
    thresholds: &ProposalThresholds,
) -> Result<AdmissionDecision, IngestRefusal> {
    match score_proposal(proposal, observed).failing_check(thresholds) {
        Some(check) => Err(IngestRefusal::ProposalNotAdmitted(check)),
        None => Ok(AdmissionDecision {
            state: ProposalState::Admitted,
            instances_allowed: true,
        }),
    }
}

/// Refuse an atom that carries no provenance key; such an atom is not stored.
pub fn check_atom_provenance(atom: &KnowledgeAtom) -> Result<(), IngestRefusal> {
    if atom.provenance_key.trim().is_empty() {
        return Err(IngestRefusal::AtomProvenanceMissing(atom.atom_id.clone()));
    }
    Ok(())
}

/// Select the atoms whose text shares a term with the query, best first, up to
/// a token budget (whitespace words). No relevant atom yields an empty context.
pub fn select_atoms<'a>(
    query: &str,
    atoms: &'a [KnowledgeAtom],
    token_budget: usize,
) -> Vec<&'a KnowledgeAtom> {
    let terms: HashSet<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut scored: Vec<(usize, &KnowledgeAtom)> = atoms
        .iter()
        .map(|atom| {
            let hits = atom
                .text
                .split_whitespace()
                .filter(|word| terms.contains(&word.to_lowercase()))
                .count();
            (hits, atom)
        })
        .filter(|(hits, _)| *hits > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.atom_id.cmp(&b.1.atom_id)));
    let mut used = 0usize;
    let mut chosen = Vec::new();
    for (_, atom) in scored {
        let tokens = atom.text.split_whitespace().count();
        if used + tokens > token_budget {
            break;
        }
        used += tokens;
        chosen.push(atom);
    }
    chosen
}
