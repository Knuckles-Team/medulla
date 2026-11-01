//! Safe UQL pushdown for a peer engine. The original signed transport and result decoder
//! remain in `federation::RemoteEngineSource`; only the query text changes.

use medulla_types::wire::{ForeignSourceSpec, Op, Plan, Pred, PredLiteral};

use super::capability::{KeyLookup, LimitPushdown, RemoteRequest, SourceCapabilities};
use super::remote::{Identity, RemoteFetch};
use crate::federation::{ForeignSource, RemoteEngineSource};
use medulla_exec::rowset::RowSet;

const MAX_KEYS: usize = 100;
const MAX_QUERY_BYTES: usize = 1024 * 1024;

pub struct EngineRemote<'a> {
    spec: &'a ForeignSourceSpec,
    identity: Identity,
    plan: Option<Plan>,
}

impl<'a> EngineRemote<'a> {
    pub fn new(spec: &'a ForeignSourceSpec, identity: Identity) -> Self {
        let plan = match spec {
            ForeignSourceSpec::RemoteEngine { uql, .. } if !uql.trim().is_empty() => {
                medulla_uql::parse(uql).ok()
            }
            _ => None,
        };
        Self {
            spec,
            identity,
            plan,
        }
    }

    fn source(&self) -> RemoteEngineSource<'_> {
        RemoteEngineSource::from_spec(self.spec).expect("EngineRemote has an engine spec")
    }

    /// Key predicates use the peer graph's reserved `id` column. A query producing
    /// synthetic ids cannot safely use that column, even when its result has an `id`.
    fn node_backed(&self) -> bool {
        let Some(plan) = &self.plan else { return false };
        matches!(plan.ops.first(), Some(Op::Scan { .. } | Op::ScanAll {}))
            && plan.ops.iter().skip(1).all(|op| {
                matches!(
                    op,
                    Op::Filter { .. } | Op::Limit { .. } | Op::Project { .. }
                )
            })
    }

    pub fn render(&self, request: &RemoteRequest) -> Option<String> {
        let mut plan = self.plan.clone()?;
        if !request.keys.is_empty() {
            if !self.node_backed() || request.keys.len() > MAX_KEYS {
                return None;
            }
            plan.ops.push(Op::Filter {
                preds: vec![Pred::In {
                    prop: "id".into(),
                    values: request.keys.iter().cloned().map(PredLiteral::Str).collect(),
                }],
            });
        }
        if let Some(k) = request.limit {
            plan.ops.push(Op::Limit { k });
        }
        let rendered = plan.to_uql().ok()?;
        (rendered.len() <= MAX_QUERY_BYTES
            && medulla_uql::parse(&rendered).ok() == Some(medulla_uql::canonicalize(&plan)))
        .then_some(rendered)
    }
}

impl RemoteFetch for EngineRemote<'_> {
    fn parallel_safe(&self) -> Option<&(dyn RemoteFetch + Sync)> {
        Some(self)
    }

    fn capabilities(&self) -> SourceCapabilities {
        let key_pushable = self.node_backed()
            && self
                .render(&RemoteRequest::keys(
                    vec!["medulla-capability-probe".into()],
                ))
                .is_some();
        let limit_pushable = self
            .render(&RemoteRequest {
                limit: Some(1),
                ..RemoteRequest::full()
            })
            .is_some();
        SourceCapabilities::single_full_fetch(
            if key_pushable {
                KeyLookup::Batched { max_keys: MAX_KEYS }
            } else {
                KeyLookup::Unsupported
            },
            if limit_pushable {
                LimitPushdown::Native
            } else {
                LimitPushdown::Unsupported
            },
        )
    }

    fn identity(&self) -> &Identity {
        &self.identity
    }

    fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String> {
        let source = self.source();
        if request.keys.is_empty() && request.limit.is_none() {
            return source.fetch();
        }
        match self.render(request) {
            Some(text) => source.fetch_uql_text(&text),
            None if !request.keys.is_empty() => {
                Err("federation: remote UQL key lookup could not be rendered".into())
            }
            None => source.fetch(),
        }
    }

    fn fit_keys(&self, keys: &[String]) -> usize {
        let mut size = self.spec_uql_len();
        keys.iter()
            .take(MAX_KEYS)
            .take_while(|key| {
                size += key.len().saturating_mul(6).saturating_add(16);
                size <= MAX_QUERY_BYTES
            })
            .count()
            .max(1)
    }

    fn key_expressible(&self, key: &str) -> bool {
        !key.contains('\0')
            && key.len().saturating_mul(6) + self.spec_uql_len() + 64 <= MAX_QUERY_BYTES
    }
}

impl EngineRemote<'_> {
    fn spec_uql_len(&self) -> usize {
        match self.spec {
            ForeignSourceSpec::RemoteEngine { uql, .. } => uql.len(),
            _ => 0,
        }
    }
}
