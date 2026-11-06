//! Temporal rule-derived facts with a persisted origin chain
//! (MEDULLA-FEDERATED-QUERY-R006.1).
//!
//! Rule form `temporal-rules-v1`:
//!
//! ```text
//! RULE temporal-rules-v1 <head> <- <body> & <body> [& <body> ...]
//! ```
//!
//! Every body atom is a premise predicate; a join needs the premises to share a
//! subject and to overlap on a non-empty closed-open event-time interval
//! `[valid_from, valid_to)`. The derived fact holds over the intersection. Derived
//! facts feed back as premises of the same subject, so chains derive in
//! event-time order until no new fact appears. Derivation is pure: every
//! refusal happens before any write, so a refused derivation leaves the graph
//! revision unchanged. [`derive_facts_mutation`] turns a derivation into the
//! `ApplyMutation` request the served write persists.

use std::collections::{BTreeMap, BTreeSet};

use medulla_identity_types::authority::AuthorityScope;
use medulla_mutation_types::mutation::{
    MutationDomain, MutationEffect, MutationPayload, MutationPrecondition, RecordMutation,
    RecordTarget, RequestedMutationResult,
};
use medulla_types::contract::{
    BoundedVec, Digest256, EngineErrorCode, OpaqueId, RecordBytes, ResourceId, SchemaId,
    ServerErrorCode,
};
use medulla_types::derived_fact::{
    derived_fact_id, DerivedFact, DerivedFactRecord, OriginKind, TEMPORAL_RULES_VERSION,
};

/// Schema id of the persisted derived-fact record.
pub const DERIVED_FACT_SCHEMA: &str = "derived-fact.v1";

/// Why a derivation or its write was refused. A refusal never changes the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeriveRefusal {
    /// One of the registered `ServerErrorCode` refusals of the rule surface.
    Coded(ServerErrorCode, String),
    /// A rule text that does not follow the `temporal-rules-v1` form.
    Invalid(String),
}

impl DeriveRefusal {
    /// The wire token the refusal is reported under.
    pub fn token(&self) -> &'static str {
        match self {
            Self::Coded(code, _) => code.as_str(),
            Self::Invalid(_) => EngineErrorCode::InvalidArgument.as_str(),
        }
    }

    fn coded(code: ServerErrorCode, detail: impl Into<String>) -> Self {
        Self::Coded(code, detail.into())
    }
}

/// A parsed temporal rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemporalRule {
    /// Rule-form version; only `temporal-rules-v1` is understood.
    pub version: String,
    /// Predicate of the derived fact.
    pub head: String,
    /// Premise predicates joined by the rule, in body order (at least one).
    pub body: Vec<String>,
}

impl TemporalRule {
    /// Parse the `temporal-rules-v1` rule form.
    pub fn parse(text: &str) -> Result<Self, DeriveRefusal> {
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let invalid = |why: &str| DeriveRefusal::Invalid(format!("rule form: {why}"));
        if tokens.first() != Some(&"RULE") || tokens.len() < 5 {
            return Err(invalid(
                "expected `RULE <version> <head> <- <body> [& <body>]`",
            ));
        }
        let version = tokens[1];
        if version != TEMPORAL_RULES_VERSION {
            return Err(DeriveRefusal::coded(
                ServerErrorCode::RuleVersionUnsupported,
                format!("rule version `{version}`; supported: {TEMPORAL_RULES_VERSION}"),
            ));
        }
        if tokens[3] != "<-" {
            return Err(invalid("expected `<-` after the head"));
        }
        let mut body = Vec::new();
        for (index, token) in tokens[4..].iter().enumerate() {
            if index % 2 == 0 {
                if *token == "&" || *token == "<-" {
                    return Err(invalid("expected a body predicate"));
                }
                body.push((*token).to_string());
            } else if *token != "&" {
                return Err(invalid("body predicates join with `&`"));
            }
        }
        if (tokens.len() - 4) % 2 == 0 {
            return Err(invalid("rule body ends with `&`"));
        }
        Ok(Self {
            version: version.to_string(),
            head: tokens[2].to_string(),
            body,
        })
    }

    /// Digest of the rule: version, head and body predicates.
    pub fn digest(&self) -> Result<Digest256, String> {
        let mut fields: Vec<&[u8]> = vec![self.version.as_bytes(), self.head.as_bytes()];
        fields.extend(self.body.iter().map(|atom| atom.as_bytes()));
        Digest256::framed(b"medulla/temporal-rule/v1", &fields)
    }
}

/// The origin of a source premise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PremiseOrigin {
    /// Sentence or axiom.
    pub kind: OriginKind,
    /// Reference of the sentence or axiom; empty counts as missing.
    pub reference: String,
}

/// A source fact a rule may join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Premise {
    /// Unique premise id.
    pub id: String,
    /// Predicate the body atoms match.
    pub predicate: String,
    /// Subject the join is keyed on.
    pub subject: String,
    /// Inclusive start of the interval (event time).
    pub valid_from: i64,
    /// Exclusive end of the interval (event time).
    pub valid_to: i64,
    /// Where the premise comes from; `None` refuses the derivation.
    pub origin: Option<PremiseOrigin>,
}

/// The caller's standing for the derivation and the graph generation it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeriveContext {
    /// Whether the caller may persist derived facts.
    pub may_write_derived: bool,
    /// Graph generation the premises were read at.
    pub graph_generation: u64,
}

/// One derivation step of the served trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationStep {
    /// Event-time order position (0-based) of the step.
    pub order: usize,
    /// Fixpoint round that produced the fact (chains have a higher round).
    pub round: usize,
    /// Id of the derived fact.
    pub fact_id: String,
    /// Premises joined, in body order.
    pub premise_ids: Vec<String>,
    /// Inclusive start of the derived interval.
    pub valid_from: i64,
    /// Exclusive end of the derived interval.
    pub valid_to: i64,
}

/// The result of one derivation: the facts to persist and the trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derivation {
    /// Facts in event-time order (then fact id), first revision, not tombstoned.
    pub facts: Vec<DerivedFactRecord>,
    /// One step per fact, same order.
    pub trace: Vec<DerivationStep>,
}

fn check_bounds(premise: &Premise) -> Result<(), DeriveRefusal> {
    if premise.valid_from >= premise.valid_to {
        return Err(DeriveRefusal::coded(
            ServerErrorCode::DerivedBoundMalformed,
            format!(
                "premise `{}` interval [{}, {}) is empty or inverted",
                premise.id, premise.valid_from, premise.valid_to
            ),
        ));
    }
    Ok(())
}

/// Derive the facts `rule` yields over `premises`.
pub fn derive(
    rule: &TemporalRule,
    premises: &[Premise],
    context: DeriveContext,
) -> Result<Derivation, DeriveRefusal> {
    if rule.version != TEMPORAL_RULES_VERSION {
        return Err(DeriveRefusal::coded(
            ServerErrorCode::RuleVersionUnsupported,
            format!("rule version `{}`", rule.version),
        ));
    }
    if !context.may_write_derived {
        return Err(DeriveRefusal::coded(
            ServerErrorCode::DerivedWriteUnauthorized,
            "the caller may not persist derived facts",
        ));
    }
    let rule_digest = rule.digest().map_err(DeriveRefusal::Invalid)?;

    // Validate premises and collapse identical duplicates; a premise id with two
    // different windows is a window conflict.
    let mut by_id: BTreeMap<&str, &Premise> = BTreeMap::new();
    for premise in premises {
        check_bounds(premise)?;
        if premise
            .origin
            .as_ref()
            .is_none_or(|origin| origin.reference.trim().is_empty())
        {
            return Err(DeriveRefusal::coded(
                ServerErrorCode::DerivedOriginMissing,
                format!("premise `{}` has no origin", premise.id),
            ));
        }
        match by_id.get(premise.id.as_str()) {
            Some(seen)
                if seen.valid_from != premise.valid_from || seen.valid_to != premise.valid_to =>
            {
                return Err(DeriveRefusal::coded(
                    ServerErrorCode::DerivedWindowConflict,
                    format!("premise `{}` carries two different windows", premise.id),
                ));
            }
            Some(_) => {}
            None => {
                by_id.insert(premise.id.as_str(), premise);
            }
        }
    }

    // Working set: (id, predicate, subject, from, to, origin); derived facts join
    // it so chains derive. Ordered by event time for deterministic output.
    #[derive(Clone)]
    struct Item {
        id: String,
        predicate: String,
        subject: String,
        from: i64,
        to: i64,
        origin: PremiseOrigin,
    }
    let mut items: Vec<Item> = by_id
        .values()
        .map(|p| Item {
            id: p.id.clone(),
            predicate: p.predicate.clone(),
            subject: p.subject.clone(),
            from: p.valid_from,
            to: p.valid_to,
            origin: p.origin.clone().expect("origin checked above"),
        })
        .collect();
    items.sort_by(|a, b| (a.from, &a.id).cmp(&(b.from, &b.id)));

    let mut produced: BTreeSet<String> = BTreeSet::new();
    let mut facts: Vec<(usize, DerivedFactRecord)> = Vec::new();
    let mut round = 0usize;
    loop {
        round += 1;
        let mut fresh: Vec<(Item, DerivedFactRecord)> = Vec::new();
        // Extend partial joins body atom by body atom.
        let mut partial: Vec<(Vec<usize>, i64, i64)> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.predicate == rule.body[0])
            .map(|(index, item)| (vec![index], item.from, item.to))
            .collect();
        for atom in &rule.body[1..] {
            let mut next = Vec::new();
            for (chosen, from, to) in &partial {
                let subject = &items[chosen[0]].subject;
                for (index, item) in items.iter().enumerate() {
                    if item.predicate != *atom || item.subject != *subject {
                        continue;
                    }
                    let (low, high) = ((*from).max(item.from), (*to).min(item.to));
                    if low < high {
                        let mut joined = chosen.clone();
                        joined.push(index);
                        next.push((joined, low, high));
                    }
                }
            }
            partial = next;
        }
        partial.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
        for (chosen, from, to) in partial {
            let premise_ids: Vec<String> = chosen.iter().map(|&i| items[i].id.clone()).collect();
            let fact_id = derived_fact_id(&rule_digest, &premise_ids, from, to)
                .map_err(DeriveRefusal::Invalid)?;
            // Set semantics: a fact already held over the same predicate, subject and
            // interval (source or derived) adds nothing, so chains reach a fixpoint.
            let window = (
                rule.head.clone(),
                items[chosen[0]].subject.clone(),
                from,
                to,
            );
            if items.iter().any(|i| {
                (&i.predicate, &i.subject, i.from, i.to)
                    == (&window.0, &window.1, window.2, window.3)
            }) || !produced.insert(fact_id.clone())
            {
                continue;
            }
            let origin = items[chosen[0]].origin.clone();
            let record = DerivedFactRecord {
                fact: DerivedFact {
                    fact_id: fact_id.clone(),
                    rule_digest,
                    premise_ids,
                    origin_kind: origin.kind,
                    origin_ref: origin.reference.clone(),
                    graph_generation: context.graph_generation,
                },
                valid_from: from,
                valid_to: to,
                revision: 1,
                tombstone: false,
            };
            let item = Item {
                id: fact_id,
                predicate: rule.head.clone(),
                subject: items[chosen[0]].subject.clone(),
                from,
                to,
                origin,
            };
            if fresh.iter().any(|(f, _)| {
                (&f.predicate, &f.subject, f.from, f.to)
                    == (&item.predicate, &item.subject, item.from, item.to)
            }) {
                continue;
            }
            fresh.push((item, record));
        }
        if fresh.is_empty() {
            break;
        }
        for (item, record) in fresh {
            facts.push((round, record));
            items.push(item);
        }
        items.sort_by(|a, b| (a.from, &a.id).cmp(&(b.from, &b.id)));
    }
    facts.sort_by(|a, b| {
        (a.1.valid_from, &a.1.fact.fact_id).cmp(&(b.1.valid_from, &b.1.fact.fact_id))
    });
    let trace = facts
        .iter()
        .enumerate()
        .map(|(order, (round, record))| DerivationStep {
            order,
            round: *round,
            fact_id: record.fact.fact_id.clone(),
            premise_ids: record.fact.premise_ids.clone(),
            valid_from: record.valid_from,
            valid_to: record.valid_to,
        })
        .collect();
    Ok(Derivation {
        facts: facts.into_iter().map(|(_, record)| record).collect(),
        trace,
    })
}

/// One derived record to persist. `prior_digest` is the digest of the revision
/// being superseded or retracted (`None` for a first revision).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedWrite {
    /// The revision to write (a retraction is a record with `tombstone` set).
    pub record: DerivedFactRecord,
    /// Digest of the stored prior revision, when this supersedes one.
    pub prior_digest: Option<Digest256>,
}

/// Canonical bytes of one record revision (named JSON, fields in declaration order).
pub fn record_bytes(record: &DerivedFactRecord) -> Result<RecordBytes, String> {
    let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    RecordBytes::new(bytes)
}

/// Build the `ApplyMutation` request that persists `writes` under `scope`.
/// A first revision requires the record to be absent; an update or retraction
/// requires the stored prior revision's digest. Returns `None` for no writes.
pub fn derive_facts_mutation(
    mutation_id: OpaqueId,
    scope: AuthorityScope,
    writes: &[DerivedWrite],
) -> Result<Option<MutationPayload>, DeriveRefusal> {
    if writes.is_empty() {
        return Ok(None);
    }
    let invalid = |error: String| DeriveRefusal::Invalid(error);
    let tenant = scope
        .tenant
        .clone()
        .ok_or_else(|| invalid("derived-fact mutation requires a tenant scope".into()))?;
    let scope_digest = scope.digest().map_err(invalid)?;
    let target = |fact_id: &str| -> Result<RecordTarget, DeriveRefusal> {
        Ok(RecordTarget {
            tenant: tenant.clone(),
            scope_digest,
            domain: MutationDomain::new("graph").map_err(invalid)?,
            schema_id: SchemaId::new(DERIVED_FACT_SCHEMA).map_err(invalid)?,
            record_id: ResourceId::new(fact_id).map_err(invalid)?,
        })
    };
    let mut preconditions = Vec::with_capacity(writes.len());
    let mut effects = Vec::with_capacity(writes.len());
    for (ordinal, write) in writes.iter().enumerate() {
        let record_target = target(&write.record.fact.fact_id)?;
        let payload = record_bytes(&write.record).map_err(invalid)?;
        preconditions.push(match write.prior_digest {
            None => MutationPrecondition::RecordAbsent {
                target: record_target.clone(),
            },
            Some(expected_digest) => MutationPrecondition::RecordDigestEquals {
                target: record_target.clone(),
                expected_digest,
            },
        });
        effects.push(MutationEffect {
            ordinal: u32::try_from(ordinal).map_err(|e| invalid(e.to_string()))?,
            target: record_target,
            mutation: RecordMutation::Put {
                payload_digest: payload.digest().map_err(invalid)?,
                payload,
            },
        });
    }
    let payload = MutationPayload::new(
        mutation_id,
        scope,
        BoundedVec::new(preconditions).map_err(invalid)?,
        BoundedVec::new(effects).map_err(invalid)?,
        BoundedVec::new(vec![]).map_err(invalid)?,
        BoundedVec::new(vec![]).map_err(invalid)?,
        RequestedMutationResult::new("receipt_only").map_err(invalid)?,
    )
    .map_err(invalid)?;
    Ok(Some(payload))
}

/// The retraction of `record`: the next revision as a tombstone.
pub fn retraction(record: &DerivedFactRecord) -> DerivedFactRecord {
    DerivedFactRecord {
        revision: record.revision + 1,
        tombstone: true,
        ..record.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_identity_types::authority::ScopeKind;
    use medulla_types::contract::{TenantId, MAX_SCOPE_COMPONENTS};

    const RULE: &str = "RULE temporal-rules-v1 colocated <- present & assigned";

    fn context() -> DeriveContext {
        DeriveContext {
            may_write_derived: true,
            graph_generation: 7,
        }
    }

    fn premise(id: &str, predicate: &str, from: i64, to: i64, kind: OriginKind) -> Premise {
        Premise {
            id: id.into(),
            predicate: predicate.into(),
            subject: "s1".into(),
            valid_from: from,
            valid_to: to,
            origin: Some(PremiseOrigin {
                kind,
                reference: format!("ref:{id}"),
            }),
        }
    }

    fn scope() -> AuthorityScope {
        AuthorityScope {
            kind: ScopeKind::new("graph").unwrap(),
            scope_id: ResourceId::new("graph:tenant:a/g").unwrap(),
            tenant: Some(TenantId::new("tenant:a").unwrap()),
            parent_scope_ids: BoundedVec::<ResourceId, MAX_SCOPE_COMPONENTS>::new(vec![
                ResourceId::new("tenant:a").unwrap(),
            ])
            .unwrap(),
            graph_incarnation: None,
        }
    }

    #[test]
    fn matching_intervals_derive_with_origin_and_nonmatching_do_not() {
        let rule = TemporalRule::parse(RULE).unwrap();
        let out = derive(
            &rule,
            &[
                premise("p1", "present", 0, 10, OriginKind::Sentence),
                premise("p2", "assigned", 5, 20, OriginKind::Axiom),
            ],
            context(),
        )
        .unwrap();
        assert_eq!(out.facts.len(), 1);
        let fact = &out.facts[0];
        assert_eq!((fact.valid_from, fact.valid_to), (5, 10));
        assert_eq!(fact.fact.premise_ids, ["p1", "p2"]);
        assert_eq!(fact.fact.origin_kind, OriginKind::Sentence);
        assert_eq!(fact.fact.origin_ref, "ref:p1");
        assert_eq!(fact.fact.graph_generation, 7);
        assert_eq!(fact.fact.rule_digest, rule.digest().unwrap());
        assert_eq!(fact.fact.fact_id.len(), 64);
        assert_eq!(out.trace.len(), 1);

        // Closed-open: touching intervals share no instant.
        let none = derive(
            &rule,
            &[
                premise("p1", "present", 0, 5, OriginKind::Sentence),
                premise("p2", "assigned", 5, 20, OriginKind::Axiom),
            ],
            context(),
        )
        .unwrap();
        assert!(none.facts.is_empty());
    }

    #[test]
    fn chains_derive_in_event_time_order_and_reach_a_fixpoint() {
        let chain = TemporalRule::parse("RULE temporal-rules-v1 active <- active & grant").unwrap();
        let out = derive(
            &chain,
            &[
                premise("a0", "active", 0, 10, OriginKind::Sentence),
                premise("g2", "grant", 4, 10, OriginKind::Axiom),
                premise("g1", "grant", 0, 6, OriginKind::Sentence),
            ],
            context(),
        )
        .unwrap();
        let windows: Vec<(i64, i64)> = out
            .facts
            .iter()
            .map(|f| (f.valid_from, f.valid_to))
            .collect();
        assert_eq!(windows[0], (0, 6));
        let mut set = windows.clone();
        set.sort();
        assert_eq!(set, [(0, 6), (4, 6), (4, 10)]);
        assert!(windows.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(out.trace.iter().map(|s| s.round).max(), Some(2));
        assert_eq!(
            out.trace.iter().map(|s| s.order).collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn refusals_name_registered_codes() {
        let rule = TemporalRule::parse(RULE).unwrap();
        let token = |result: Result<Derivation, DeriveRefusal>| result.unwrap_err().token();
        assert_eq!(
            TemporalRule::parse("RULE temporal-rules-v2 h <- a")
                .unwrap_err()
                .token(),
            "RULE_VERSION_UNSUPPORTED"
        );
        let mut missing = premise("p1", "present", 0, 10, OriginKind::Sentence);
        missing.origin = None;
        assert_eq!(
            token(derive(&rule, &[missing], context())),
            "DERIVED_ORIGIN_MISSING"
        );
        assert_eq!(
            token(derive(
                &rule,
                &[premise("p1", "present", 10, 10, OriginKind::Sentence)],
                context()
            )),
            "DERIVED_BOUND_MALFORMED"
        );
        assert_eq!(
            token(derive(
                &rule,
                &[
                    premise("p1", "present", 0, 10, OriginKind::Sentence),
                    premise("p1", "present", 0, 11, OriginKind::Sentence),
                ],
                context()
            )),
            "DERIVED_WINDOW_CONFLICT"
        );
        assert_eq!(
            token(derive(
                &rule,
                &[],
                DeriveContext {
                    may_write_derived: false,
                    graph_generation: 1
                }
            )),
            "DERIVED_WRITE_UNAUTHORIZED"
        );
        for code in [
            "DERIVED_BOUND_MALFORMED",
            "DERIVED_ORIGIN_MISSING",
            "DERIVED_WINDOW_CONFLICT",
            "DERIVED_WRITE_UNAUTHORIZED",
            "RULE_VERSION_UNSUPPORTED",
        ] {
            assert!(ServerErrorCode::ALL.iter().any(|c| c.as_str() == code));
        }
    }

    #[test]
    fn mutation_puts_each_fact_and_retraction_bumps_revision() {
        let rule = TemporalRule::parse(RULE).unwrap();
        let out = derive(
            &rule,
            &[
                premise("p1", "present", 0, 10, OriginKind::Sentence),
                premise("p2", "assigned", 5, 20, OriginKind::Axiom),
            ],
            context(),
        )
        .unwrap();
        assert!(
            derive_facts_mutation(OpaqueId::new("m:0").unwrap(), scope(), &[])
                .unwrap()
                .is_none()
        );
        let first = DerivedWrite {
            record: out.facts[0].clone(),
            prior_digest: None,
        };
        let payload =
            derive_facts_mutation(OpaqueId::new("m:1").unwrap(), scope(), &[first.clone()])
                .unwrap()
                .unwrap();
        payload.canonical_payload_digest().unwrap();
        let tomb = retraction(&first.record);
        assert_eq!((tomb.revision, tomb.tombstone), (2, true));
        let stored = record_bytes(&first.record).unwrap().digest().unwrap();
        derive_facts_mutation(
            OpaqueId::new("m:2").unwrap(),
            scope(),
            &[DerivedWrite {
                record: tomb,
                prior_digest: Some(stored),
            }],
        )
        .unwrap()
        .unwrap();
    }
}
