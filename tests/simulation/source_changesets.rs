//! MEDULLA-SCM-R011: the seeded simulation of `SubmitChangeset`.
//!
//! Every seed drives the real signed `dispatch` surface over a durable engine:
//! blobs are uploaded through `BlobBegin`/`BlobChunkPut`/`BlobCommit`, then
//! changesets land through `SubmitChangeset`, so the real accept, rebase, tree
//! and history-index code runs. A simulated clock orders overlapping writers;
//! failure injection drops an acknowledgement (a crash between the accept and
//! the answer) and retries the same idempotency key. After every step the
//! harness asserts three invariants against a model folded from the receipts:
//! chain linearity, tree equals replay, index equals rebuild.
//!
//! `SIM_SEEDS` sets the seed count (default 16; nightly runs set 1000).
//!
//! The git export round trip of MEDULLA-SCM-R010 is not served yet, so the
//! "export" check materializes the head tree through `ReadTree` and compares it
//! with the replay of every landed changeset, the same listing an export packs.

#![cfg(all(feature = "server", feature = "security", feature = "redb", feature = "blob"))]

#[path = "../common/mod.rs"]
mod common;
#[path = "../common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_compute::line_merge::{merge_lines, LineMerge};
use medulla_protocol_types::protocol::{GraphType, Method, Request, Response, ResultPayload};
use medulla_server_auth::auth::{compute_verified_envelope_token, VerifiedEnvelopeParams};
use medulla_server_blob::blob::store::RedbChunkStore;
use medulla_server_blob::blob::{BlobCursors, BlobStateExt};
use medulla_source_types::source_path_grant::SetPathGrantRequest;
use medulla_source_types::source_changeset::{
    Changeset, PathOp, ReadPathHistoryRequest, ReadPathHistoryReceipt, ReadTreeReceipt,
    ReadTreeRequest, SubmitChangesetReceipt, SubmitChangesetRequest, SubmittedBlobRef,
    SubmittedPath, TreeEntry,
};

const SECRET: &str = "scm-r011-simulation-secret";
const ENCRYPTION_KEY: &str = "scm-r011-simulation-encryption-key-0123456789";
const MODE: u32 = 0o100644;
const POOL: [&str; 4] = ["a", "b", "dir/c", "dir/d"];
const LINES: usize = 8;
const STEPS: usize = 8;
const DEFAULT_SEEDS: u64 = 16;

/// A deterministic generator (splitmix64) that refuses the degenerate seed 0.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Result<Self, String> {
        if seed == 0 {
            return Err("seed 0 cannot initialize the generator".to_string());
        }
        Ok(Self(seed))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// The simulated clock: writers arrive at ticks and are served in tick order.
struct SimClock {
    now: u64,
}

impl SimClock {
    fn arrival(&mut self, rng: &mut Rng) -> u64 {
        self.now += 1 + rng.below(5) as u64;
        self.now
    }
}

/// What the harness knows of the branch: every landed changeset with its
/// carrying commit, and the file contents after each landing.
struct Model {
    landed: Vec<(String, Changeset)>,
    snapshots: Vec<BTreeMap<String, Vec<String>>>,
}

impl Model {
    fn head(&self) -> Option<String> {
        self.landed.last().map(|(commit, _)| commit.clone())
    }

    /// The contents as of `index` landed changesets (0 is the empty tree).
    fn contents_at(&self, index: usize) -> BTreeMap<String, Vec<String>> {
        match index {
            0 => BTreeMap::new(),
            n => self.snapshots[n - 1].clone(),
        }
    }

    fn current(&self) -> BTreeMap<String, Vec<String>> {
        self.contents_at(self.landed.len())
    }

    /// The full replay of the landed changesets.
    fn replay(&self) -> Vec<TreeEntry> {
        let mut tree: BTreeMap<String, (String, u32)> = BTreeMap::new();
        for (_, changeset) in &self.landed {
            for changed in &changeset.changed_paths {
                match (&changed.op, &changed.new_blob) {
                    (PathOp::Delete, _) | (_, None) => {
                        tree.remove(&changed.path);
                    }
                    (_, Some(digest)) => {
                        tree.insert(changed.path.clone(), (digest.clone(), changed.mode));
                    }
                }
            }
        }
        tree.into_iter()
            .map(|(path, (blob_digest, mode))| TreeEntry {
                path,
                blob_digest,
                mode,
            })
            .collect()
    }

    /// The commits that touched `path`, newest first.
    fn touching(&self, path: &str) -> Vec<String> {
        self.landed
            .iter()
            .rev()
            .filter(|(_, changeset)| changeset.changed_paths.iter().any(|c| c.path == path))
            .map(|(commit, _)| commit.clone())
            .collect()
    }
}

/// One simulated engine, branch and model.
struct Sim {
    state: test_support::SharedState,
    graph: String,
    branch: String,
    next_id: u64,
    next_key: u64,
    rng: Rng,
    clock: SimClock,
    model: Model,
}

fn lines_to_bytes(lines: &[String]) -> Vec<u8> {
    let mut text = lines.join("\n");
    text.push('\n');
    text.into_bytes()
}

fn decode<T: serde::de::DeserializeOwned>(label: &str, response: Response) -> T {
    assert!(
        response.error.is_none(),
        "{label} was refused: {:?}",
        response.error
    );
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{label} does not decode: {error}")),
        Some(ResultPayload::Json(value)) => serde_json::from_value(value)
            .unwrap_or_else(|error| panic!("{label} does not decode: {error}")),
        other => panic!("{label} answered an unexpected payload: {other:?}"),
    }
}

/// A signed request with a caller-chosen idempotency key, so a retry after a
/// dropped acknowledgement carries the same key.
fn signed(id: u64, graph: &str, idempotency_key: &str, method: Method) -> Request {
    common::configure_authority();
    let context = common::context_for(common::TEST_AGENT);
    let mut request = Request {
        id,
        graph: graph.to_string(),
        auth_token: String::new(),
        agent_id: Some(context.agent_id.clone()),
        method,
    };
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let nonce = format!("sim-{}-{id}-{idempotency_key}", std::process::id());
    request.auth_token = compute_verified_envelope_token(
        SECRET,
        &request,
        &VerifiedEnvelopeParams {
            context: &context,
            timestamp,
            nonce: &nonce,
            idempotency_key,
        },
    );
    request
}

/// What a submission came to.
enum Outcome {
    Landed(Box<SubmitChangesetReceipt>),
    Refused(String),
}

impl Sim {
    /// Initialize one seed: a durable engine, a graph and a branch. A seed the
    /// generator refuses records no state: nothing is created before it.
    async fn new(seed: u64) -> Result<Self, String> {
        let rng = Rng::new(seed)?;
        test_support::provision_encryption_key_once(ENCRYPTION_KEY);
        let dir = test_support::fresh_dir("medulla-scm-r011");
        let dir_s = dir.to_string_lossy().to_string();
        let (_backend, state) =
            test_support::redb_state_at(SECRET, common::current_isolation(), &dir_s);
        let store = Arc::new(RedbChunkStore::open(&dir_s).map_err(|e| format!("{e:?}"))?);
        state
            .write()
            .await
            .set_blob_cursors(Some(Arc::new(BlobCursors::new(store))));
        let graph = format!("scm-sim-{seed}");
        let branch = format!("sim/{seed}");
        let mut sim = Self {
            state,
            graph: graph.clone(),
            branch,
            next_id: 10,
            next_key: 0,
            rng,
            clock: SimClock { now: 0 },
            model: Model {
                landed: Vec::new(),
                snapshots: Vec::new(),
            },
        };
        let created = sim
            .call(
                "init-graph",
                Method::CreateGraph {
                    graph_name: graph.clone(),
                    graph_type: GraphType::Global,
                },
            )
            .await;
        if let Some(error) = created.error {
            return Err(format!("CreateGraph: {error}"));
        }
        let made = sim
            .call(
                "init-branch",
                Method::CreateBranch {
                    tenant: common::TEST_TENANT.to_string(),
                    graph,
                    name: sim.branch.clone(),
                    base_engine_version: None,
                },
            )
            .await;
        if let Some(error) = made.error {
            return Err(format!("CreateBranch: {error}"));
        }
        // MEDULLA-SCM-R007: the source methods deny by default.
        let granted = sim
            .call(
                "init-grant",
                Method::SetPathGrant {
                    request: Box::new(SetPathGrantRequest {
                        repo: sim.graph.clone(),
                        principal: common::TEST_AGENT.to_string(),
                        prefix: String::new(),
                        read: true,
                        write: true,
                    }),
                },
            )
            .await;
        if let Some(error) = granted.error {
            return Err(format!("SetPathGrant: {error}"));
        }
        Ok(sim)
    }

    async fn call(&mut self, key: &str, method: Method) -> Response {
        self.next_id += 1;
        let request = signed(self.next_id, &self.graph.clone(), key, method);
        test_support::dispatch(&self.state, request).await
    }

    fn fresh_key(&mut self, label: &str) -> String {
        self.next_key += 1;
        format!("{label}-{}", self.next_key)
    }

    /// Upload `bytes` through the blob protocol and return the committed ref.
    async fn upload(&mut self, bytes: &[u8]) -> SubmittedBlobRef {
        let key = self.fresh_key("blob");
        let cursor: u64 = decode(
            "BlobBegin",
            self.call(&format!("{key}-begin"), Method::BlobBegin { chunk_size: 0 })
                .await,
        );
        let _: serde_json::Value = decode(
            "BlobChunkPut",
            self.call(
                &format!("{key}-put"),
                Method::BlobChunkPut {
                    cursor,
                    data: bytes.to_vec(),
                },
            )
            .await,
        );
        let digest: String = decode(
            "BlobCommit",
            self.call(&format!("{key}-commit"), Method::BlobCommit { cursor })
                .await,
        );
        SubmittedBlobRef {
            digest,
            size: bytes.len() as u64,
        }
    }

    /// Send one submission under `key`. With `drop_ack`, a landed answer is
    /// treated as lost and the same key is sent again: the retry must return the
    /// identical record and head, and leave the chain as it was.
    async fn submit(
        &mut self,
        key: &str,
        base: Option<String>,
        paths: Vec<SubmittedPath>,
        drop_ack: bool,
    ) -> Outcome {
        let request = SubmitChangesetRequest {
            repo: self.graph.clone(),
            branch: self.branch.clone(),
            base_head: base,
            message: format!("simulated change {key}"),
            paths,
            trailers: None,
            run_trace_id: None,
        };
        let send = |request: SubmitChangesetRequest| Method::SubmitChangeset {
            request: Box::new(request),
        };
        let first = self.call(key, send(request.clone())).await;
        if let Some(error) = first.error {
            return Outcome::Refused(error);
        }
        let landed: SubmitChangesetReceipt = decode("SubmitChangeset", first);
        assert!(!landed.replayed, "a first attempt is never a replay");
        if drop_ack {
            let retried: SubmitChangesetReceipt =
                decode("SubmitChangeset retry", self.call(key, send(request)).await);
            assert_eq!(retried.changeset, landed.changeset);
            assert_eq!(retried.branch_head, landed.branch_head);
        }
        Outcome::Landed(Box::new(landed))
    }

    /// Record a landing in the model and assert chain linearity.
    fn land(&mut self, receipt: &SubmitChangesetReceipt, contents: BTreeMap<String, Vec<String>>) {
        if let Some(previous) = self.model.head() {
            assert_eq!(
                receipt.changeset.parents,
                vec![previous.clone()],
                "chain linearity: the new changeset extends the previous head"
            );
            assert_ne!(receipt.branch_head, previous, "the head advanced");
        }
        self.model
            .landed
            .push((receipt.branch_head.clone(), receipt.changeset.clone()));
        self.model.snapshots.push(contents);
    }

    fn entry(path: &str, op: PathOp, blob: Option<SubmittedBlobRef>) -> SubmittedPath {
        SubmittedPath {
            path: path.to_string(),
            op,
            blob,
            mode: MODE,
            renamed_from: None,
        }
    }

    /// The first changeset adds every pooled file.
    async fn seed_tree(&mut self) {
        let mut contents = BTreeMap::new();
        let mut paths = Vec::new();
        for path in POOL {
            let lines: Vec<String> = (0..LINES).map(|n| format!("{path} line {n}")).collect();
            let blob = self.upload(&lines_to_bytes(&lines)).await;
            paths.push(Self::entry(path, PathOp::Add, Some(blob)));
            contents.insert(path.to_string(), lines);
        }
        let key = self.fresh_key("seed");
        match self.submit(&key, None, paths, false).await {
            Outcome::Landed(receipt) => self.land(&receipt, contents),
            Outcome::Refused(error) => panic!("the seed changeset was refused: {error}"),
        }
    }

    /// A writer's edit of line `line` of `path` against the contents it saw.
    async fn edit(
        &mut self,
        seen: &BTreeMap<String, Vec<String>>,
        path: &str,
        line: usize,
        token: &str,
    ) -> (SubmittedPath, Vec<String>) {
        let mut lines = seen[path].clone();
        lines[line] = format!("{path} edited {token}");
        let blob = self.upload(&lines_to_bytes(&lines)).await;
        (Self::entry(path, PathOp::Modify, Some(blob)), lines)
    }

    /// Two writers see the same head; the first lands, the second arrives with
    /// a stale base. Disjoint paths land both; one path rebases by line or
    /// refuses `SOURCE_CONFLICT`.
    async fn overlapping_writers(&mut self, same_path: bool) {
        let base_index = self.model.landed.len();
        let base = self.model.head();
        let seen = self.model.contents_at(base_index);
        let first_path = POOL[self.rng.below(POOL.len())];
        let second_path = if same_path {
            first_path
        } else {
            let other: Vec<&str> = POOL.iter().copied().filter(|p| *p != first_path).collect();
            other[self.rng.below(other.len())]
        };
        let (first_line, second_line) = (self.rng.below(LINES), self.rng.below(LINES));
        // The clock orders the two arrivals.
        let mut arrivals = [
            (self.clock.arrival(&mut self.rng), 0usize),
            (self.clock.arrival(&mut self.rng), 1usize),
        ];
        arrivals.sort();
        let drop_ack = self.rng.below(3) == 0;
        let mut landed_first: Option<(String, Vec<String>)> = None;
        for (tick, writer) in arrivals {
            let prior = landed_first.clone();
            let (path, line) = if writer == 0 {
                (first_path, first_line)
            } else {
                (second_path, second_line)
            };
            let token = format!("w{writer}t{tick}");
            let (entry, lines) = self.edit(&seen, path, line, &token).await;
            let key = self.fresh_key("writer");
            let outcome = self
                .submit(&key, base.clone(), vec![entry], drop_ack)
                .await;
            match (prior, outcome) {
                (None, Outcome::Landed(receipt)) => {
                    let mut contents = self.model.current();
                    contents.insert(path.to_string(), lines.clone());
                    self.land(&receipt, contents);
                    landed_first = Some((path.to_string(), lines));
                    self.check().await;
                }
                (None, Outcome::Refused(error)) => {
                    panic!("the first writer on the head was refused: {error}")
                }
                (Some((first, theirs)), Outcome::Landed(receipt)) => {
                    let theirs = &theirs;
                    let mut contents = self.model.current();
                    if first.as_str() == path {
                        // Rebased by line: the landed bytes are the diff3 merge.
                        let base_bytes = lines_to_bytes(&seen[path]);
                        let merged = match merge_lines(
                            &base_bytes,
                            &lines_to_bytes(&lines),
                            &lines_to_bytes(theirs),
                        ) {
                            LineMerge::Clean(bytes) => bytes,
                            LineMerge::Conflict(_) => {
                                panic!("a conflicting overlap landed without a merge")
                            }
                        };
                        let expected = self.upload(&merged).await;
                        let landed_blob = receipt.changeset.changed_paths[0]
                            .new_blob
                            .clone()
                            .expect("a landed modify names its blob");
                        assert_eq!(landed_blob, expected.digest, "the landed bytes are the merge");
                        let text = String::from_utf8(merged).expect("merged text");
                        contents.insert(
                            path.to_string(),
                            text.lines().map(str::to_string).collect(),
                        );
                    } else {
                        contents.insert(path.to_string(), lines);
                    }
                    self.land(&receipt, contents);
                    self.check().await;
                }
                (Some((first, theirs)), Outcome::Refused(error)) => {
                    // Only an overlap on one path may refuse, and only as a conflict.
                    assert_eq!(first, path, "a disjoint writer must land: {error}");
                    assert!(error.contains("SOURCE_CONFLICT"), "unexpected refusal: {error}");
                    assert!(
                        error.contains(&format!("paths=[{path}]")),
                        "the conflict names the path: {error}"
                    );
                    let _ = (theirs, lines);
                    self.check().await;
                }
            }
        }
    }

    /// A writer whose base is several changesets behind the head, touching a
    /// path none of them touched: the dirty base rebases cleanly.
    async fn dirty_base(&mut self) {
        let behind = self.model.landed.len().min(1 + self.rng.below(3));
        let base_index = self.model.landed.len() - behind;
        let touched: Vec<String> = self.model.landed[base_index..]
            .iter()
            .flat_map(|(_, c)| c.changed_paths.iter().map(|p| p.path.clone()))
            .collect();
        let Some(path) = POOL
            .iter()
            .copied()
            .find(|p| !touched.iter().any(|t| t.as_str() == *p))
        else {
            return;
        };
        let base = if base_index == 0 {
            None
        } else {
            Some(self.model.landed[base_index - 1].0.clone())
        };
        let seen = self.model.contents_at(base_index);
        let line = self.rng.below(LINES);
        let token = format!("dirty{}", self.clock.arrival(&mut self.rng));
        let (entry, lines) = self.edit(&seen, path, line, &token).await;
        let key = self.fresh_key("dirty");
        match self.submit(&key, base, vec![entry], false).await {
            Outcome::Landed(receipt) => {
                let mut contents = self.model.current();
                contents.insert(path.to_string(), lines);
                self.land(&receipt, contents);
            }
            Outcome::Refused(error) => panic!("a disjoint dirty-base writer was refused: {error}"),
        }
        self.check().await;
    }

    /// A crash between the accept and the answer: the answer is lost, the key
    /// is retried, and the retry replays the identical record.
    async fn crash_and_replay(&mut self) {
        let base = self.model.head();
        let seen = self.model.current();
        let path = POOL[self.rng.below(POOL.len())];
        let line = self.rng.below(LINES);
        let token = format!("crash{}", self.clock.arrival(&mut self.rng));
        let (entry, lines) = self.edit(&seen, path, line, &token).await;
        let key = self.fresh_key("crash");
        match self.submit(&key, base, vec![entry], true).await {
            Outcome::Landed(receipt) => {
                let mut contents = self.model.current();
                contents.insert(path.to_string(), lines);
                let before = self.model.landed.len();
                self.land(&receipt, contents);
                assert_eq!(self.model.landed.len(), before + 1, "a replay adds nothing");
            }
            Outcome::Refused(error) => panic!("an uncontended writer was refused: {error}"),
        }
        self.check().await;
    }

    /// The invariants: tree equals replay and index equals rebuild, read back
    /// through the served `ReadTree` and `ReadPathHistory`.
    async fn check(&mut self) {
        let head = self.model.head().expect("a landed head");
        let key = self.fresh_key("read");
        let tree: ReadTreeReceipt = decode(
            "ReadTree",
            self.call(
                &key,
                Method::ReadTree {
                    request: Box::new(ReadTreeRequest {
                        repo: self.graph.clone(),
                        branch: self.branch.clone(),
                        rev: head.clone(),
                        prefix: String::new(),
                        depth: 0,
                    }),
                },
            )
            .await,
        );
        assert_eq!(tree.entries, self.model.replay(), "tree equals replay");
        for path in POOL {
            let key = self.fresh_key("hist");
            let history: ReadPathHistoryReceipt = decode(
                "ReadPathHistory",
                self.call(
                    &key,
                    Method::ReadPathHistory {
                        request: Box::new(ReadPathHistoryRequest {
                            repo: self.graph.clone(),
                            branch: self.branch.clone(),
                            rev: head.clone(),
                            path: path.to_string(),
                            limit: 1000,
                        }),
                    },
                )
                .await,
            );
            let commits: Vec<String> = history.entries.into_iter().map(|e| e.commit).collect();
            assert_eq!(
                commits,
                self.model.touching(path),
                "index equals rebuild for {path}"
            );
        }
    }

    /// One seed: the seed changeset, then `STEPS` seeded steps.
    async fn run(&mut self) {
        self.seed_tree().await;
        self.check().await;
        for _ in 0..STEPS {
            match self.rng.below(4) {
                0 => self.overlapping_writers(false).await,
                1 => self.overlapping_writers(true).await,
                2 => self.dirty_base().await,
                _ => self.crash_and_replay().await,
            }
        }
    }
}

fn seed_count() -> u64 {
    std::env::var("SIM_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEEDS)
}

// spec: MEDULLA-SCM-R011
#[tokio::test(flavor = "multi_thread")]
async fn simulate_concurrent_accept_crash_and_replay() {
    for seed in 1..=seed_count() {
        let mut sim = Sim::new(seed)
            .await
            .unwrap_or_else(|error| panic!("seed {seed} failed to initialize: {error}"));
        sim.run().await;
    }
}

// spec: MEDULLA-SCM-R011
#[tokio::test(flavor = "multi_thread")]
async fn a_seed_that_fails_to_initialize_is_refused_and_records_no_state() {
    let refused = Sim::new(0).await.err().expect("seed 0 is refused");
    assert!(refused.contains("seed 0"), "{refused}");
}
