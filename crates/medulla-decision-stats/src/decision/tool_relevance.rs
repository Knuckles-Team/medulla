//! Tool relevance: lexical and embedding rank fusion
//! (MEDULLA-DECISION-ENGINE-R134.110.2).
//!
//! Pure and deterministic: no clock, no store, no I/O. The lexical arm is the
//! candidate-local [`bm25::scores`] over each tool's name and description; the
//! embedding arm is the cosine similarity between Q32 integer embeddings. The
//! arms fuse by reciprocal rank. Only visible tools, inside the role allowlist,
//! take part, so an invisible tool never moves a score.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use medulla_decision_types::decision::statistical::tool_subset::{
    smallest_covering_subset, ToolCoverageCandidate, ToolSubsetRequestBody,
};
use medulla_decision_types::decision::tool_relevance::{
    AssignedTool, ToolAssignmentRequest, ToolEntry, ToolRanking, ToolRankingKnobs,
    ToolRelevanceError, ToolRelevanceRefusal, MAX_REQUIRED_CAPABILITIES,
};
use medulla_types::contract::BoundedVec;
use std::collections::BTreeMap;

use super::{bm25, quant};

/// One tool of the index with its stored embedding and visibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedTool {
    pub entry: ToolEntry,
    /// The stored Q32 embedding of the tool, when it has one.
    pub embedding: Option<Vec<i64>>,
    /// Whether the requesting principal may see the tool.
    pub visible: bool,
}

fn quantise(value: f64) -> Result<i64, ToolRelevanceError> {
    quant::q32(value)
        .map(|quantised| quantised.value)
        .map_err(|refusal| ToolRelevanceError::Malformed(refusal.detail))
}

/// Cosine similarity of two integer vectors of equal length: integer dot
/// products, an IEEE square root, quantised once onto Q32. A zero vector is 0.
fn cosine_q32(task: &[i64], tool: &[i64]) -> Result<i64, ToolRelevanceError> {
    let dot: i128 = task
        .iter()
        .zip(tool)
        .map(|(a, b)| i128::from(*a) * i128::from(*b))
        .sum();
    let squares = |values: &[i64]| -> i128 {
        values
            .iter()
            .map(|value| i128::from(*value) * i128::from(*value))
            .sum()
    };
    let norm = (squares(task) as f64).sqrt() * (squares(tool) as f64).sqrt();
    if norm == 0.0 {
        return Ok(0);
    }
    quantise((dot as f64 / norm).clamp(-1.0, 1.0))
}

/// 1-based ranks of `scored` (id, score) pairs, best first, ties by id.
fn ranks<S: Copy>(
    mut scored: Vec<(String, S)>,
    order: impl Fn(&S, &S) -> Ordering,
) -> Vec<(String, u32)> {
    scored.sort_by(|(a_id, a), (b_id, b)| order(b, a).then_with(|| a_id.cmp(b_id)));
    scored
        .into_iter()
        .enumerate()
        .map(|(index, (id, _))| (id, u32::try_from(index + 1).unwrap_or(u32::MAX)))
        .collect()
}

fn rank_of(ranks: &[(String, u32)], tool_id: &str) -> Option<u32> {
    ranks
        .iter()
        .find(|(id, _)| id == tool_id)
        .map(|(_, rank)| *rank)
}

/// Rank the tools of `tools` for `request` by reciprocal-rank fusion of the
/// lexical and embedding arms.
///
/// `allowlist` is the `toolset_refs` of the role's published toolset; `None`
/// applies no allowlist. `rrf_k` is the knob `tool_relevance.rrf_k`. The result
/// holds every allowed visible tool, best first: fused score descending, then
/// tool id. `relevance_q32` is the fused score over its maximum
/// (`arms / (rrf_k + 1)`), a Q32 value in `[0, 1]`. A tool embedding whose length
/// differs from the task embedding is `EMBEDDING_DIMENSION_MISMATCH`.
pub fn fuse_tool_ranks(
    tools: &[IndexedTool],
    request: &ToolAssignmentRequest,
    allowlist: Option<&[String]>,
    rrf_k: u32,
) -> Result<Vec<AssignedTool>, ToolRelevanceError> {
    let allowed_ids: Option<BTreeSet<&str>> =
        allowlist.map(|ids| ids.iter().map(String::as_str).collect());
    let mut allowed: Vec<&IndexedTool> = tools
        .iter()
        .filter(|tool| tool.visible)
        .filter(|tool| {
            allowed_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(tool.entry.tool_id.as_str()))
        })
        .collect();
    allowed.sort_by(|a, b| a.entry.tool_id.cmp(&b.entry.tool_id));

    // Lexical arm: only a tool with a positive score has a lexical rank.
    let documents: Vec<String> = allowed
        .iter()
        .map(|tool| format!("{} {}", tool.entry.name, tool.entry.description))
        .collect();
    let documents: Vec<&str> = documents.iter().map(String::as_str).collect();
    let lexical = ranks(
        allowed
            .iter()
            .zip(bm25::scores(&documents, &request.task_text))
            .filter(|(_, score)| *score > 0.0)
            .map(|(tool, score)| (tool.entry.tool_id.clone(), score))
            .collect(),
        |a: &f64, b: &f64| a.total_cmp(b),
    );

    // Embedding arm: only when the task carries an embedding.
    let embedding = match request.task_embedding.as_ref() {
        None => None,
        Some(task) => {
            let mut scored = Vec::new();
            for tool in &allowed {
                let Some(stored) = tool.embedding.as_deref() else {
                    continue;
                };
                if stored.len() != task.len() {
                    return Err(ToolRelevanceError::EmbeddingDimensionMismatch {
                        tool_id: tool.entry.tool_id.clone(),
                        task: task.len(),
                        tool: stored.len(),
                    });
                }
                scored.push((
                    tool.entry.tool_id.clone(),
                    cosine_q32(task.as_slice(), stored)?,
                ));
            }
            Some(ranks(scored, |a: &i64, b: &i64| a.cmp(b)))
        }
    };

    let arms = if embedding.is_some() { 2.0 } else { 1.0 };
    let k = f64::from(rrf_k);
    let mut ranked = Vec::with_capacity(allowed.len());
    for tool in &allowed {
        let id = tool.entry.tool_id.as_str();
        let lexical_rank = rank_of(&lexical, id);
        let embedding_rank = embedding.as_ref().and_then(|ranks| rank_of(ranks, id));
        let fused: f64 = [lexical_rank, embedding_rank]
            .into_iter()
            .flatten()
            .map(|rank| 1.0 / (k + f64::from(rank)))
            .sum();
        let relevance = (fused / (arms / (k + 1.0))).clamp(0.0, 1.0);
        ranked.push(AssignedTool {
            tool_id: tool.entry.tool_id.clone(),
            server_id: tool.entry.server_id.clone(),
            lexical_rank,
            embedding_rank,
            fused_score_q32: quantise(fused)?,
            relevance_q32: quantise(relevance)?,
            rank: 0,
        });
    }
    ranked.sort_by(|a, b| {
        b.fused_score_q32
            .cmp(&a.fused_score_q32)
            .then_with(|| a.tool_id.cmp(&b.tool_id))
    });
    for (index, tool) in ranked.iter_mut().enumerate() {
        tool.rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
    }
    Ok(ranked)
}

/// Rank `entries` by `relevance` and select a covering subset inside the token
/// budget (MEDULLA-DECISION-ENGINE-R134.2).
///
/// `relevance` is the fused Q32 relevance per tool id (R134.110.2); a tool with
/// no entry has relevance 0. A tool whose relevance is below the floor is
/// dropped (exact integer comparison); the rest sort by relevance descending,
/// then tool id, and the best `top_k` stay. Relevance is a prefilter, not the
/// objective: the kept tools go to [`smallest_covering_subset`], greedy by
/// uncovered-capability gain per token, then lower token cost, then tool id.
/// With no required capability the kept tools are taken in rank order while
/// their cumulative `token_cost` stays within the budget.
pub fn rank_tools_by_relevance(
    entries: &[ToolEntry],
    relevance: &BTreeMap<String, i64>,
    required_capabilities: &BoundedVec<String, MAX_REQUIRED_CAPABILITIES>,
    context_budget_tokens: u32,
    knobs: &ToolRankingKnobs,
) -> Result<ToolRanking, ToolRelevanceRefusal> {
    let floor = i128::from(knobs.relevance_floor_micros) << 32;
    let mut scored: Vec<(&ToolEntry, i64)> = entries
        .iter()
        .map(|entry| {
            (
                entry,
                relevance.get(entry.tool_id.as_str()).copied().unwrap_or(0),
            )
        })
        .filter(|(_, score)| i128::from(*score) * 1_000_000 >= floor)
        .collect();
    if scored.is_empty() {
        return Err(ToolRelevanceRefusal::NoRelevantTool);
    }
    scored.sort_by(|(a, a_score), (b, b_score)| {
        b_score.cmp(a_score).then_with(|| a.tool_id.cmp(&b.tool_id))
    });
    scored.truncate(knobs.top_k as usize);

    let selected_tool_ids = if required_capabilities.is_empty() {
        let mut spent: u64 = 0;
        let mut chosen = Vec::new();
        for (entry, _) in &scored {
            spent += u64::from(entry.token_cost);
            if spent > u64::from(context_budget_tokens) {
                break;
            }
            chosen.push(entry.tool_id.clone());
        }
        chosen
    } else {
        let candidates = scored
            .iter()
            .map(|(entry, _)| ToolCoverageCandidate {
                tool_id: entry.tool_id.clone(),
                capabilities: entry.capability_iris.clone(),
                token_cost: entry.token_cost,
            })
            .collect::<Vec<_>>();
        // At most 50 kept tools, inside the 256-candidate bound.
        let candidates = BoundedVec::new(candidates)
            .expect("top_k keeps at most 50 candidates, inside the subset bound");
        smallest_covering_subset(&ToolSubsetRequestBody {
            required_capabilities: required_capabilities.clone(),
            context_budget_tokens,
            candidates,
        })?
    };
    Ok(ToolRanking {
        kept: scored
            .into_iter()
            .map(|(entry, score)| (entry.tool_id.clone(), score))
            .collect(),
        selected_tool_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::record::outcome::DecisionRefusal;
    use medulla_decision_types::decision::tool_relevance::TOOL_RELEVANCE_SCHEMA_VERSION;

    const ONE: i64 = 1 << 32;

    fn tool(id: &str, name: &str, description: &str, embedding: Option<Vec<i64>>) -> IndexedTool {
        IndexedTool {
            entry: ToolEntry {
                schema_version: TOOL_RELEVANCE_SCHEMA_VERSION,
                tool_id: id.into(),
                server_id: "srv".into(),
                name: name.into(),
                description: description.into(),
                capability_iris: BoundedVec::default(),
                token_cost: 100,
            },
            embedding,
            visible: true,
        }
    }

    fn request(text: &str, embedding: Option<Vec<i64>>) -> ToolAssignmentRequest {
        ToolAssignmentRequest {
            schema_version: TOOL_RELEVANCE_SCHEMA_VERSION,
            task_text: text.into(),
            task_embedding: embedding.map(|values| BoundedVec::new(values).unwrap()),
            agent_role_iri: String::new(),
            top_k: 10,
            context_budget_tokens: 1000,
            required_capabilities: BoundedVec::default(),
        }
    }

    fn catalog() -> Vec<IndexedTool> {
        vec![
            tool(
                "t1",
                "export_report",
                "export the quarterly report",
                Some(vec![ONE, 0]),
            ),
            tool(
                "t2",
                "send_mail",
                "send an email message",
                Some(vec![0, ONE]),
            ),
            tool(
                "t3",
                "export_table",
                "export a table to csv",
                Some(vec![ONE, ONE]),
            ),
            tool("t4", "resize_image", "resize an image", None),
        ]
    }

    fn ids(ranked: &[AssignedTool]) -> Vec<&str> {
        ranked.iter().map(|tool| tool.tool_id.as_str()).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.110.2
    #[test]
    fn fused_order_and_visible_only_scores() {
        let request = request("export the quarterly report", Some(vec![ONE, 0]));
        let ranked = fuse_tool_ranks(&catalog(), &request, None, 60).unwrap();

        // t1 leads both arms; t3 is second lexically (export) and second by
        // cosine (45 degrees); t2 is orthogonal; t4 has no embedding and no
        // lexical match.
        assert_eq!(ids(&ranked), ["t1", "t3", "t2", "t4"]);
        assert_eq!(ranked[0].lexical_rank, Some(1));
        assert_eq!(ranked[0].embedding_rank, Some(1));
        assert_eq!(ranked[1].lexical_rank, Some(2));
        assert_eq!(ranked[1].embedding_rank, Some(2));
        assert_eq!(ranked[3].lexical_rank, None);
        assert_eq!(ranked[3].embedding_rank, None);
        assert_eq!(ranked[3].fused_score_q32, 0);
        assert_eq!(
            ranked.iter().map(|tool| tool.rank).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        // The best tool in both arms has relevance exactly 1.
        assert!(ranked[0].relevance_q32 >= ONE - 1 && ranked[0].relevance_q32 <= ONE);
        assert!(ranked[0].relevance_q32 > ranked[1].relevance_q32);

        // An invisible tool, however good its text, leaves every score unchanged.
        let mut with_hidden = catalog();
        let mut hidden = tool(
            "t0",
            "export_quarterly_report",
            "export the quarterly report report report",
            Some(vec![ONE, 0]),
        );
        hidden.visible = false;
        with_hidden.push(hidden);
        let again = fuse_tool_ranks(&with_hidden, &request, None, 60).unwrap();
        assert_eq!(again, ranked);

        // The role allowlist filters first: t1 is not in it, so it never ranks.
        let allow = vec!["t2".to_string(), "t3".to_string()];
        let restricted = fuse_tool_ranks(&catalog(), &request, Some(&allow), 60).unwrap();
        assert_eq!(ids(&restricted), ["t3", "t2"]);

        // Without a task embedding the lexical arm alone ranks, and relevance
        // is scaled by one arm.
        let text_only = fuse_tool_ranks(
            &catalog(),
            &self::request("export the quarterly report", None),
            None,
            60,
        )
        .unwrap();
        assert_eq!(text_only[0].tool_id, "t1");
        assert!(text_only.iter().all(|tool| tool.embedding_rank.is_none()));
        assert!(text_only[0].relevance_q32 >= ONE - 1);

        // A different embedding length is refused with the typed code.
        let mismatched = fuse_tool_ranks(
            &catalog(),
            &self::request("export", Some(vec![ONE, 0, 0])),
            None,
            60,
        )
        .unwrap_err();
        assert!(matches!(
            mismatched,
            ToolRelevanceError::EmbeddingDimensionMismatch {
                task: 3,
                tool: 2,
                ..
            }
        ));
        assert_eq!(
            mismatched.code().map(|code| code.as_str()),
            Some("EMBEDDING_DIMENSION_MISMATCH")
        );
    }

    fn entry(id: &str, caps: &[&str], cost: u32) -> ToolEntry {
        ToolEntry {
            schema_version: TOOL_RELEVANCE_SCHEMA_VERSION,
            tool_id: id.into(),
            server_id: "srv".into(),
            name: id.into(),
            description: String::new(),
            capability_iris: BoundedVec::new(caps.iter().map(|c| c.to_string()).collect()).unwrap(),
            token_cost: cost,
        }
    }

    fn scores(values: &[(&str, f64)]) -> BTreeMap<String, i64> {
        values
            .iter()
            .map(|(id, value)| (id.to_string(), (value * ONE as f64) as i64))
            .collect()
    }

    fn required(caps: &[&str]) -> BoundedVec<String, MAX_REQUIRED_CAPABILITIES> {
        BoundedVec::new(caps.iter().map(|c| c.to_string()).collect()).unwrap()
    }

    const KNOBS: ToolRankingKnobs = ToolRankingKnobs {
        relevance_floor_micros: 200_000,
        top_k: 12,
    };

    // spec: MEDULLA-DECISION-ENGINE-R134.2
    #[test]
    fn relevant_tools_fit_budget() {
        let entries = vec![
            entry("a", &["x"], 100),
            entry("b", &["y"], 100),
            entry("c", &["x", "y"], 150),
            entry("d", &["z"], 10),
            entry("e", &["x"], 100),
        ];
        // d sits below the 0.2 floor, so it never enters; b and e tie and
        // order by id.
        let relevance = scores(&[("a", 0.9), ("b", 0.5), ("c", 0.3), ("d", 0.1), ("e", 0.5)]);

        // Without a required capability the kept tools are taken in rank
        // order while the cumulative cost stays inside the budget.
        let ranking =
            rank_tools_by_relevance(&entries, &relevance, &required(&[]), 350, &KNOBS).unwrap();
        let kept: Vec<&str> = ranking.kept.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(kept, ["a", "b", "e", "c"]);
        assert_eq!(ranking.selected_tool_ids, ["a", "b", "e"]);

        // top_k cuts the ranking before selection.
        let cut = ToolRankingKnobs { top_k: 2, ..KNOBS };
        let ranking =
            rank_tools_by_relevance(&entries, &relevance, &required(&[]), 1000, &cut).unwrap();
        assert_eq!(ranking.selected_tool_ids, ["a", "b"]);

        // With required capabilities the covering subset is greedy by gain per
        // token: c covers x and y at 150 (gain 2 per 150) against a alone
        // (1 per 100), so c is the single pick; relevance is a prefilter only.
        let ranking =
            rank_tools_by_relevance(&entries, &relevance, &required(&["x", "y"]), 200, &KNOBS)
                .unwrap();
        assert_eq!(ranking.selected_tool_ids, ["c"]);

        // The coverage pick is by gain per token, not by relevance: b (100
        // tokens) beats the better ranked c (150) for y; a tool below the
        // floor never covers anything.
        let ranking =
            rank_tools_by_relevance(&entries, &relevance, &required(&["y"]), 200, &KNOBS).unwrap();
        assert_eq!(ranking.selected_tool_ids, ["b"]);
        let refused = rank_tools_by_relevance(&entries, &relevance, &required(&["z"]), 200, &KNOBS)
            .unwrap_err();
        assert_eq!(
            DecisionRefusal::code(&refused).as_str(),
            "TOOL_SUBSET_CAPABILITY_UNCOVERABLE"
        );

        // Cost ties break by tool id: a and e cover x at 100 tokens; a ranks
        // first and wins the tie.
        let ranking =
            rank_tools_by_relevance(&entries[..], &relevance, &required(&["x"]), 100, &KNOBS)
                .unwrap();
        assert_eq!(ranking.selected_tool_ids, ["a"]);

        // A budget the cheapest covering pick exceeds refuses.
        let refused =
            rank_tools_by_relevance(&entries, &relevance, &required(&["x", "y"]), 120, &KNOBS)
                .unwrap_err();
        assert_eq!(
            DecisionRefusal::code(&refused).as_str(),
            "TOOL_SUBSET_BUDGET_EXCEEDED"
        );

        // A floor that removes every tool refuses NO_RELEVANT_TOOL.
        let high = ToolRankingKnobs {
            relevance_floor_micros: 1_000_000,
            ..KNOBS
        };
        let refused = rank_tools_by_relevance(
            &entries,
            &scores(&[("a", 0.9)]),
            &required(&[]),
            1000,
            &high,
        )
        .unwrap_err();
        assert_eq!(refused, ToolRelevanceRefusal::NoRelevantTool);
        assert_eq!(DecisionRefusal::code(&refused).as_str(), "NO_RELEVANT_TOOL");

        // A floor of 0 keeps a tool with no relevance entry.
        let open = ToolRankingKnobs {
            relevance_floor_micros: 0,
            ..KNOBS
        };
        let ranking =
            rank_tools_by_relevance(&entries, &scores(&[]), &required(&[]), 1000, &open).unwrap();
        assert_eq!(ranking.kept.len(), 5);
    }
}
