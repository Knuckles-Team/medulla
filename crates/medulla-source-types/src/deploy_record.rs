//! Deploy records (MEDULLA-SDLC-R002.7, MEDULLA-SDLC-R002.8): the durable, tenant-keyed record of
//! one applied deployment plan.
//!
//! One record is keyed by the digest of the plan that was applied. It names the target, the
//! outcome of every node the plan touched and the image digests it rolled out; the engine adds the
//! verified actor and the time of the engine clock. A record is written once: the same body again
//! replays the stored record, a different body for the same plan digest refuses
//! `DEPLOY_RECORD_SEALED`. The node is a sealed record class, so no generic write can change it.
//!
//! Pure data and decisions; the served handler reads the stored record, calls
//! [`handle_deploy_record_put`] and writes the one node.

use medulla_types::refusal_code::RefusalCode;
use medulla_types::tenant_cursor::CursorFamily;
use serde::{Deserialize, Serialize};

use crate::lifecycle::sha256_hex;

/// The graph `type` of a deploy record node.
pub const DEPLOY_RECORD_TYPE: &str = "DeployRecord";
/// The node property holding the canonical JSON of the stored [`DeployRecord`].
pub const DEPLOY_RECORD_BODY_PROPERTY: &str = "deploy_record";
/// The node property holding the seal of the record.
pub const DEPLOY_RECORD_SEAL_PROPERTY: &str = "deploy_seal";
/// The node property holding the engine-clock time the record was written, in milliseconds.
pub const DEPLOY_RECORD_RECORDED_AT_PROPERTY: &str = "recorded_at_ms";

/// A request is out of bounds, or a page cursor is malformed or minted for another tenant.
pub const DEPLOY_RECORD_INVALID: &str = "DEPLOY_RECORD_INVALID";
/// A different record is already stored for the plan digest.
pub const DEPLOY_RECORD_SEALED: &str = "DEPLOY_RECORD_SEALED";
/// No record of the verified tenant is stored for the plan digest. An absent record and a record
/// of another tenant answer alike, so the refusal is not an existence oracle.
pub const DEPLOY_RECORD_NOT_FOUND: &str = "DEPLOY_RECORD_NOT_FOUND";

/// The stable refusal codes of the deploy record methods.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(DEPLOY_RECORD_INVALID),
    RefusalCode::new(DEPLOY_RECORD_SEALED),
    RefusalCode::new(DEPLOY_RECORD_NOT_FOUND),
];

/// Most nodes one record names.
pub const MAX_DEPLOY_NODES: usize = 256;
/// Most image digests one record names.
pub const MAX_DEPLOY_IMAGES: usize = 256;
/// Longest target, node name or approver, in bytes.
pub const MAX_DEPLOY_TEXT_BYTES: usize = 256;
/// Longest node failure code, in bytes.
pub const MAX_DEPLOY_CODE_BYTES: usize = 64;
/// Most records one `DeployRecordList` page carries, and the page size when none is asked for.
pub const MAX_DEPLOY_RECORD_LIST_LIMIT: u32 = 100;
/// Longest opaque list cursor a caller may hand back.
pub const MAX_DEPLOY_RECORD_CURSOR_BYTES: usize = 1024;
/// Domain-separation prefix of the `DeployRecordList` cursor family.
pub const DEPLOY_RECORD_CURSOR_DOMAIN: &[u8] = b"medulla/deploy-record-list-cursor/v1";

const LIST_CURSOR: CursorFamily = CursorFamily {
    domain: DEPLOY_RECORD_CURSOR_DOMAIN,
    max_bytes: MAX_DEPLOY_RECORD_CURSOR_BYTES,
    noun: "deploy record list",
};

/// A typed refusal of a deploy record method.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeployRefusal {
    /// A field is out of bounds or a cursor is not usable.
    #[error("DEPLOY_RECORD_INVALID: {0}")]
    Invalid(String),
    /// A different record is already stored for the plan digest.
    #[error("DEPLOY_RECORD_SEALED: {0}")]
    Sealed(String),
    /// No record of the verified tenant is stored for the plan digest.
    #[error("DEPLOY_RECORD_NOT_FOUND: {0}")]
    NotFound(String),
}

impl DeployRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => DEPLOY_RECORD_INVALID,
            Self::Sealed(_) => DEPLOY_RECORD_SEALED,
            Self::NotFound(_) => DEPLOY_RECORD_NOT_FOUND,
        }
    }

    /// The one refusal of a read that cannot find its record.
    pub fn not_found() -> Self {
        Self::NotFound("no deploy record for this plan digest".to_owned())
    }
}

/// What one node of the plan did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeployNodeResult {
    /// The node's step ran and passed.
    Succeeded,
    /// The node's step ran and failed.
    Failed,
    /// The node's step did not run, because an earlier one failed.
    Skipped,
    /// The node was rolled back after it ran.
    RolledBack,
}

/// The outcome of one node of the applied plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeployNodeOutcome {
    /// The node name the plan gave it.
    pub node: String,
    /// What the node did.
    pub outcome: DeployNodeResult,
    /// A stable failure code of the node, when it did not succeed.
    #[serde(default)]
    pub code: Option<String>,
}

/// The body of one deploy record, as the caller writes it. The actor and the time are not part of
/// it: the engine takes them from the verified request and its own clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeployRecordInput {
    /// The digest of the applied plan, `sha256:` and 64 lowercase hex digits. It is the key.
    pub plan_digest: String,
    /// The environment or cluster the plan was applied to.
    pub target: String,
    /// The outcome of every node the plan touched.
    pub nodes: Vec<DeployNodeOutcome>,
    /// The digests of the images the plan rolled out, each `sha256:` and 64 lowercase hex digits.
    pub image_digests: Vec<String>,
    /// Who approved the plan, when the plan had an approval gate.
    #[serde(default)]
    pub approver: Option<String>,
    /// When the apply started, in milliseconds.
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    /// When the apply finished, in milliseconds.
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
}

/// One stored deploy record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeployRecord {
    /// The body as it was written, in canonical order.
    pub record: DeployRecordInput,
    /// The verified principal that wrote the record.
    pub actor: String,
    /// When the record was written, by the engine clock, in milliseconds.
    pub recorded_at_ms: u64,
    /// The seal over the body (see [`deploy_record_seal`]).
    pub seal: String,
}

/// What `DeployRecordPut` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeployRecordReceipt {
    /// The plan digest the record is keyed by.
    pub plan_digest: String,
    /// The graph node id of the record.
    pub node_id: String,
    /// The seal over the body.
    pub seal: String,
    /// The verified principal that wrote the record.
    pub actor: String,
    /// When the record was written, by the engine clock, in milliseconds.
    pub recorded_at_ms: u64,
    /// True when the same body was already stored and nothing was written.
    pub replayed: bool,
}

/// One row of a `DeployRecordList` page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeployRecordSummary {
    /// The plan digest the record is keyed by.
    pub plan_digest: String,
    /// The target the plan was applied to.
    pub target: String,
    /// The verified principal that wrote the record.
    pub actor: String,
    /// When the record was written, by the engine clock, in milliseconds.
    pub recorded_at_ms: u64,
    /// How many nodes the record names.
    pub node_count: u32,
    /// True when every node succeeded.
    pub all_succeeded: bool,
}

/// One cursor-paged page of deploy records, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeployRecordPage {
    /// The records of this page.
    pub records: Vec<DeployRecordSummary>,
    /// The opaque cursor of the next page, absent on the last page.
    pub next_cursor: Option<String>,
}

/// What [`handle_deploy_record_put`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployRecordWrite {
    /// The record to store, or the stored one when the write replays.
    pub record: DeployRecord,
    /// True when the same body was already stored, so nothing is written.
    pub replayed: bool,
}

impl DeployRecordWrite {
    /// The receipt of this decision; `node_id` is the node of the record.
    pub fn receipt(&self, node_id: String) -> DeployRecordReceipt {
        DeployRecordReceipt {
            plan_digest: self.record.record.plan_digest.clone(),
            node_id,
            seal: self.record.seal.clone(),
            actor: self.record.actor.clone(),
            recorded_at_ms: self.record.recorded_at_ms,
            replayed: self.replayed,
        }
    }
}

/// Whether `value` is `sha256:` followed by 64 lowercase hex digits.
pub fn is_sha256_ref(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The graph node id of the record of `plan_digest` in `tenant`: the tenant is part of the key,
/// as a short digest so no tenant name appears in a node id.
pub fn deploy_record_node_id(tenant: &str, plan_digest: &str) -> String {
    let tenant_key = &sha256_hex(tenant.as_bytes())[..16];
    let plan = plan_digest.strip_prefix("sha256:").unwrap_or(plan_digest);
    format!("deploy_record:{tenant_key}:{plan}")
}

/// The seal of a record body: the SHA-256 of its canonical JSON, as `sha256:<hex>`.
pub fn deploy_record_seal(record: &DeployRecordInput) -> String {
    format!(
        "sha256:{}",
        sha256_hex(serde_json::to_string(record).unwrap_or_default().as_bytes())
    )
}

fn invalid<T>(what: &str) -> Result<T, DeployRefusal> {
    Err(DeployRefusal::Invalid(format!("{what} is out of bounds")))
}

fn text_ok(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_DEPLOY_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn code_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DEPLOY_CODE_BYTES
        && value.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

impl DeployRecordInput {
    /// Check every bound and put the body in canonical order: nodes by name, image digests sorted
    /// and unique. The same body in another order seals identically, so a retry replays.
    pub fn canonical(mut self) -> Result<Self, DeployRefusal> {
        if !is_sha256_ref(&self.plan_digest) {
            return invalid("plan_digest");
        }
        if !text_ok(&self.target) {
            return invalid("target");
        }
        if self.nodes.is_empty() || self.nodes.len() > MAX_DEPLOY_NODES {
            return invalid("nodes");
        }
        if self.nodes.iter().any(|node| {
            !text_ok(&node.node) || node.code.as_deref().is_some_and(|code| !code_ok(code))
        }) {
            return invalid("a node");
        }
        self.nodes.sort_by(|left, right| left.node.cmp(&right.node));
        if self.nodes.windows(2).any(|pair| pair[0].node == pair[1].node) {
            return invalid("nodes (a node is named twice)");
        }
        if self.image_digests.len() > MAX_DEPLOY_IMAGES
            || !self.image_digests.iter().all(|digest| is_sha256_ref(digest))
        {
            return invalid("image_digests");
        }
        self.image_digests.sort();
        self.image_digests.dedup();
        if self.approver.as_deref().is_some_and(|approver| !text_ok(approver)) {
            return invalid("approver");
        }
        if let (Some(started), Some(finished)) = (self.started_at_ms, self.finished_at_ms) {
            if finished < started {
                return invalid("finished_at_ms (before started_at_ms)");
            }
        }
        Ok(self)
    }
}

/// `DeployRecordPut` (MEDULLA-SDLC-R002.7): decide what one record writes, or refuse before
/// anything is written.
///
/// `stored` is the record already held for the plan digest of the tenant, if any. A stored record
/// with the same canonical body replays: it is returned unchanged, so the actor and the time of
/// the first write stay. A stored record with a different body refuses `DEPLOY_RECORD_SEALED`.
/// `actor` is the verified principal and `now_ms` the time of the engine clock.
pub fn handle_deploy_record_put(
    stored: Option<&DeployRecord>,
    input: DeployRecordInput,
    actor: &str,
    now_ms: u64,
) -> Result<DeployRecordWrite, DeployRefusal> {
    let input = input.canonical()?;
    if actor.trim().is_empty() {
        return invalid("actor");
    }
    if let Some(held) = stored {
        if held.record != input {
            return Err(DeployRefusal::Sealed(format!(
                "a different record is already stored for {}",
                input.plan_digest
            )));
        }
        return Ok(DeployRecordWrite {
            record: held.clone(),
            replayed: true,
        });
    }
    let seal = deploy_record_seal(&input);
    Ok(DeployRecordWrite {
        record: DeployRecord {
            record: input,
            actor: actor.to_owned(),
            recorded_at_ms: now_ms,
            seal,
        },
        replayed: false,
    })
}

/// `DeployRecordGet` (MEDULLA-SDLC-R002.8): the stored record of `plan_digest`, or the one
/// not-found refusal. A malformed digest is also not found, so the answer is the same for every
/// digest that holds no record.
pub fn handle_deploy_record_get(
    stored: Option<DeployRecord>,
    plan_digest: &str,
) -> Result<DeployRecord, DeployRefusal> {
    match stored {
        Some(record) if is_sha256_ref(plan_digest) => Ok(record),
        _ => Err(DeployRefusal::not_found()),
    }
}

/// The resume key of a record in the newest-first order: the inverted time, then the digest.
fn resume_key(record: &DeployRecord) -> String {
    format!(
        "{:020}|{}",
        u64::MAX - record.recorded_at_ms,
        record.record.plan_digest
    )
}

/// `DeployRecordList` (MEDULLA-SDLC-R002.8): one page of `records` (every record of `tenant`),
/// newest first, resuming after `cursor`. `limit` is 1 to [`MAX_DEPLOY_RECORD_LIST_LIMIT`]
/// (the maximum when absent). A cursor that is malformed or was minted for another tenant
/// refuses `DEPLOY_RECORD_INVALID`.
pub fn handle_deploy_record_list(
    mut records: Vec<DeployRecord>,
    tenant: &str,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Result<DeployRecordPage, DeployRefusal> {
    let limit = match limit {
        None => MAX_DEPLOY_RECORD_LIST_LIMIT as usize,
        Some(0) => return invalid("limit"),
        Some(asked) if asked > MAX_DEPLOY_RECORD_LIST_LIMIT => return invalid("limit"),
        Some(asked) => asked as usize,
    };
    let after = cursor
        .map(|cursor| {
            LIST_CURSOR
                .decode(tenant, cursor, |key| !key.trim().is_empty())
                .map_err(DeployRefusal::Invalid)
        })
        .transpose()?;
    records.sort_by_cached_key(resume_key);
    let rest: Vec<DeployRecord> = records
        .into_iter()
        .filter(|record| after.as_ref().is_none_or(|key| resume_key(record) > *key))
        .collect();
    let more = rest.len() > limit;
    let page: Vec<DeployRecord> = rest.into_iter().take(limit).collect();
    let next_cursor = if more {
        page.last().map(|last| LIST_CURSOR.encode(tenant, &resume_key(last)))
    } else {
        None
    };
    Ok(DeployRecordPage {
        records: page
            .iter()
            .map(|held| DeployRecordSummary {
                plan_digest: held.record.plan_digest.clone(),
                target: held.record.target.clone(),
                actor: held.actor.clone(),
                recorded_at_ms: held.recorded_at_ms,
                node_count: u32::try_from(held.record.nodes.len()).unwrap_or(u32::MAX),
                all_succeeded: held
                    .record
                    .nodes
                    .iter()
                    .all(|node| node.outcome == DeployNodeResult::Succeeded),
            })
            .collect(),
        next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(seed: char) -> String {
        format!("sha256:{}", seed.to_string().repeat(64))
    }

    fn body(plan: char) -> DeployRecordInput {
        DeployRecordInput {
            plan_digest: digest(plan),
            target: "prod".to_owned(),
            nodes: vec![
                DeployNodeOutcome {
                    node: "web".to_owned(),
                    outcome: DeployNodeResult::Succeeded,
                    code: None,
                },
                DeployNodeOutcome {
                    node: "api".to_owned(),
                    outcome: DeployNodeResult::Failed,
                    code: Some("STEP_TIMEOUT".to_owned()),
                },
            ],
            image_digests: vec![digest('b'), digest('a'), digest('b')],
            approver: Some("usr:approver".to_owned()),
            started_at_ms: Some(10),
            finished_at_ms: Some(20),
        }
    }

    // spec: MEDULLA-SDLC-R002.7
    #[test]
    fn a_body_is_canonical_and_its_order_does_not_change_its_seal() {
        let mut shuffled = body('1');
        shuffled.nodes.reverse();
        shuffled.image_digests.reverse();
        let first = body('1').canonical().expect("valid");
        let second = shuffled.canonical().expect("valid");
        assert_eq!(first, second);
        assert_eq!(first.nodes[0].node, "api");
        assert_eq!(first.image_digests, vec![digest('a'), digest('b')]);
        assert_eq!(deploy_record_seal(&first), deploy_record_seal(&second));
        assert!(is_sha256_ref(deploy_record_seal(&first).as_str()));
    }

    // spec: MEDULLA-SDLC-R002.7
    #[test]
    fn a_first_write_takes_the_actor_and_the_clock_and_a_retry_replays_the_first() {
        let first = handle_deploy_record_put(None, body('1'), "usr:applier", 1_000).expect("write");
        assert!(!first.replayed);
        assert_eq!(first.record.actor, "usr:applier");
        assert_eq!(first.record.recorded_at_ms, 1_000);
        // The same body later, by another principal, returns the stored record unchanged.
        let retry =
            handle_deploy_record_put(Some(&first.record), body('1'), "usr:other", 9_000).expect("retry");
        assert!(retry.replayed);
        assert_eq!(retry.record, first.record);
        let node = deploy_record_node_id("acme", &digest('1'));
        let receipt = retry.receipt(node.clone());
        assert_eq!((receipt.actor.as_str(), receipt.recorded_at_ms), ("usr:applier", 1_000));
        assert_eq!(receipt.node_id, node);
    }

    // spec: MEDULLA-SDLC-R002.7
    #[test]
    fn a_different_body_for_the_same_plan_digest_refuses_sealed() {
        let first = handle_deploy_record_put(None, body('1'), "usr:applier", 1_000).expect("write");
        let mut changed = body('1');
        changed.target = "staging".to_owned();
        let refusal = handle_deploy_record_put(Some(&first.record), changed, "usr:applier", 2_000)
            .expect_err("a different body is refused");
        assert_eq!(refusal.code(), DEPLOY_RECORD_SEALED);
    }

    // spec: MEDULLA-SDLC-R002.7
    #[test]
    fn every_bound_refuses_before_a_write() {
        let cases: Vec<Box<dyn Fn(&mut DeployRecordInput)>> = vec![
            Box::new(|b| b.plan_digest = "abc".to_owned()),
            Box::new(|b| b.plan_digest = format!("sha256:{}", "A".repeat(64))),
            Box::new(|b| b.target = " ".to_owned()),
            Box::new(|b| b.nodes.clear()),
            Box::new(|b| b.nodes[0].node = b.nodes[1].node.clone()),
            Box::new(|b| b.nodes[0].code = Some("lower case".to_owned())),
            Box::new(|b| b.image_digests.push("latest".to_owned())),
            Box::new(|b| b.approver = Some("bad\napprover".to_owned())),
            Box::new(|b| b.finished_at_ms = Some(5)),
        ];
        for (index, mutate) in cases.iter().enumerate() {
            let mut candidate = body('1');
            mutate(&mut candidate);
            let refusal = handle_deploy_record_put(None, candidate, "usr:applier", 1)
                .expect_err(&format!("case {index} must refuse"));
            assert_eq!(refusal.code(), DEPLOY_RECORD_INVALID, "case {index}");
        }
        assert_eq!(
            handle_deploy_record_put(None, body('1'), "  ", 1).expect_err("blank actor").code(),
            DEPLOY_RECORD_INVALID
        );
    }

    // spec: MEDULLA-SDLC-R002.7
    #[test]
    fn the_node_id_is_keyed_by_tenant_and_plan_and_names_no_tenant() {
        let acme = deploy_record_node_id("acme", &digest('1'));
        let other = deploy_record_node_id("globex", &digest('1'));
        assert_ne!(acme, other);
        assert!(!acme.contains("acme"));
        assert_eq!(acme, deploy_record_node_id("acme", &digest('1')));
        assert!(acme.starts_with("deploy_record:"));
    }

    // spec: MEDULLA-SDLC-R002.8
    #[test]
    fn get_answers_one_refusal_for_a_missing_and_a_malformed_digest() {
        let held = handle_deploy_record_put(None, body('1'), "usr:applier", 1).expect("write").record;
        assert_eq!(
            handle_deploy_record_get(Some(held.clone()), &digest('1')).expect("found"),
            held
        );
        let missing = handle_deploy_record_get(None, &digest('2')).expect_err("missing");
        let malformed = handle_deploy_record_get(Some(held), "not-a-digest").expect_err("malformed");
        assert_eq!(missing, malformed);
        assert_eq!(missing.code(), DEPLOY_RECORD_NOT_FOUND);
    }

    fn stored(plan: char, at: u64) -> DeployRecord {
        handle_deploy_record_put(None, body(plan), "usr:applier", at).expect("write").record
    }

    // spec: MEDULLA-SDLC-R002.8
    #[test]
    fn list_pages_newest_first_with_a_tenant_bound_cursor() {
        let records = vec![stored('1', 100), stored('2', 300), stored('3', 200)];
        let first = handle_deploy_record_list(records.clone(), "acme", Some(2), None).expect("page");
        let digests: Vec<&str> = first.records.iter().map(|r| r.plan_digest.as_str()).collect();
        assert_eq!(digests, vec![digest('2').as_str(), digest('3').as_str()]);
        assert!(!first.records[0].all_succeeded);
        assert_eq!(first.records[0].node_count, 2);
        let cursor = first.next_cursor.expect("more remain");
        let second =
            handle_deploy_record_list(records.clone(), "acme", Some(2), Some(&cursor)).expect("page");
        assert_eq!(second.records.len(), 1);
        assert_eq!(second.records[0].plan_digest, digest('1'));
        assert!(second.next_cursor.is_none());
        // A cursor of another tenant, a garbled one, and a bad limit refuse the same code.
        for refusal in [
            handle_deploy_record_list(records.clone(), "globex", Some(2), Some(&cursor)),
            handle_deploy_record_list(records.clone(), "acme", Some(2), Some("zz")),
            handle_deploy_record_list(records.clone(), "acme", Some(0), None),
            handle_deploy_record_list(records, "acme", Some(MAX_DEPLOY_RECORD_LIST_LIMIT + 1), None),
        ] {
            assert_eq!(refusal.expect_err("refused").code(), DEPLOY_RECORD_INVALID);
        }
    }
}
