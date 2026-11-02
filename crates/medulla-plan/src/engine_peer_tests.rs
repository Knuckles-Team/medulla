//! Pushdown to a peer engine, end to end against a peer that enforces what the served
//! engine enforces (MEDULLA-FEDERATED-QUERY-R052): it verifies the signed request envelope
//! with its own secret, serves a principal only the rows that principal is granted, and
//! may lack the pushdown capability altogether. Every request crosses the real framed
//! transport and the real signer; the peer records how it judged each one.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla_protocol_types::{
    protocol::Method, protocol::Request, protocol::Response, protocol::ResultPayload,
};
use medulla_types::acl::RequestContextClaims;
use medulla_types::wire::{ForeignSourceSpec, UqlResult};

use medulla_exec::budget::FederationBudget;
use medulla_federation::federation_opt::{FederationSession, FetchStrategy, FragmentTrace, BUDGET_EXCEEDED};
use medulla_exec::algebra::Op;
use medulla_exec::exec::{execute, PlanCtx};
use medulla_exec::algebra::Plan;

/// The registered statement: `d1`, `d2`, `d4` and `d5` of the fixture.
const BASE: &str = "MATCH (:Doc) |> WHERE year > 2023";
const PRINCIPAL: &str = "agent:peer-test";

/// A signing secret that is visibly not a credential.
fn fixture_secret(tag: &str) -> String {
    format!("fixture-{tag}-not-a-credential")
}

/// How the peer judged one request.
#[derive(Clone, Debug, PartialEq)]
enum Judged {
    /// The envelope did not verify under the peer's secret.
    Unauthenticated,
    /// The verified principal holds no grant on this peer.
    Forbidden,
    /// The peer does not evaluate this statement (it lacks the pushdown capability).
    Unsupported,
    /// Evaluated: the rows returned, and the rows the principal's grant withheld.
    Served {
        text: String,
        rows: Vec<String>,
        withheld: Vec<String>,
    },
}

/// What the peer enforces.
struct Policy {
    secret: String,
    /// The one principal holding a grant, and the row ids it may read.
    granted: HashSet<String>,
    /// Whether the peer evaluates a statement other than [`BASE`].
    pushdown: bool,
}

impl Policy {
    fn granting(ids: &[&str]) -> Self {
        Self {
            secret: fixture_secret("peer"),
            granted: ids.iter().map(|id| id.to_string()).collect(),
            pushdown: true,
        }
    }
}

/// The signed envelope a request carries in its `eg2.` token.
#[derive(serde::Deserialize)]
struct Envelope {
    context: RequestContextClaims,
    timestamp: u64,
    nonce: String,
    idempotency_key: String,
    mac: String,
}

/// The claims of `request`'s envelope, if its MAC verifies under `secret` over what was
/// actually received: the request id, graph, method name and body hash, and the claims.
pub(super) fn verified_claims(secret: &str, request: &Request) -> Option<RequestContextClaims> {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};

    let encoded = hex::decode(request.auth_token.strip_prefix("eg2.")?).ok()?;
    let envelope: Envelope = serde_json::from_slice(&encoded).ok()?;
    let body_hash = hex::encode(Sha256::digest(request.method.canonical_body_bytes()));
    let signed = medulla_protocol_types::protocol::build_envelope_v2_bytes(
        request.id,
        &request.graph,
        &request.method.tag_name(),
        &body_hash,
        &envelope.context,
        envelope.timestamp,
        &envelope.nonce,
        &envelope.idempotency_key,
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(&signed);
    mac.verify_slice(&hex::decode(&envelope.mac).ok()?).ok()?;
    let claims = envelope.context;
    (request.agent_id.as_deref() == Some(claims.agent_id.as_str())).then_some(claims)
}

impl Policy {
    /// Judge `request` and, when it is served, produce the result it is answered with.
    fn judge(&self, request: &Request, ctx: &PlanCtx) -> (Judged, Option<UqlResult>) {
        let Some(claims) = verified_claims(&self.secret, request) else {
            return (Judged::Unauthenticated, None);
        };
        if claims.principal != PRINCIPAL {
            return (Judged::Forbidden, None);
        }
        match &request.method {
            Method::Uql { text, params, .. } if self.pushdown || text == BASE => {
                let statement = medulla_uql::parse_statement(text, params).expect("peer UQL");
                let result = crate::uql::serve::run_statement(&statement, ctx).expect("peer run");
                self.serve(text, result)
            }
            _ => (Judged::Unsupported, None),
        }
    }

    /// Withhold every row outside the grant, as row-level policy does on a served engine.
    fn serve(&self, text: &str, mut result: UqlResult) -> (Judged, Option<UqlResult>) {
        let UqlResult::Rows { rows, .. } = &mut result else {
            return (Judged::Unsupported, None);
        };
        let (kept, withheld): (Vec<_>, Vec<_>) = std::mem::take(rows)
            .into_iter()
            .partition(|row| self.granted.contains(&row.id));
        let judged = Judged::Served {
            text: text.to_string(),
            rows: kept.iter().map(|row| row.id.clone()).collect(),
            withheld: withheld.into_iter().map(|row| row.id).collect(),
        };
        *rows = kept;
        (judged, Some(result))
    }
}

/// What a peer has seen.
#[derive(Default)]
struct Seen {
    judged: Vec<Judged>,
    requests: Vec<Request>,
}

/// A framed loopback peer engine over the planner fixture.
struct Peer {
    endpoint: String,
    secret: String,
    seen: Arc<Mutex<Seen>>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).ok()?;
    let mut body = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut body).ok()?;
    rmp_serde::from_slice(&body).ok()
}

fn write_response(stream: &mut TcpStream, response: &Response) {
    let encoded = rmp_serde::to_vec_named(response).expect("encode response");
    let _ = stream.write_all(&(encoded.len() as u32).to_be_bytes());
    let _ = stream.write_all(&encoded);
}

impl Peer {
    fn spawn(policy: Policy) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind peer");
        let endpoint = listener.local_addr().expect("peer address").to_string();
        let secret = policy.secret.clone();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let record = seen.clone();
        std::thread::spawn(move || {
            let fixture = medulla_exec::fixture::build();
            let ctx = PlanCtx::new(&fixture.view, &fixture.semantic);
            for stream in listener.incoming().take(64) {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Some(request) = read_request(&mut stream) else {
                    continue;
                };
                let (judged, result) = policy.judge(&request, &ctx);
                let response = match result {
                    Some(result) => Response::ok(
                        request.id,
                        ResultPayload::Raw(rmp_serde::to_vec_named(&result).expect("encode")),
                    ),
                    None => Response::err(request.id, "PERMISSION_DENIED"),
                };
                {
                    let mut seen = record.lock().expect("peer record lock");
                    seen.judged.push(judged);
                    seen.requests.push(request);
                }
                write_response(&mut stream, &response);
            }
        });
        Self {
            endpoint,
            secret,
            seen,
        }
    }

    fn judged(&self) -> Vec<Judged> {
        self.seen.lock().expect("peer record lock").judged.clone()
    }

    /// The source spec of a caller signing as `principal` with `secret`.
    fn spec_as(&self, principal: &str, secret: &str) -> ForeignSourceSpec {
        ForeignSourceSpec::RemoteEngine {
            endpoint: self.endpoint.clone(),
            graph: "g".into(),
            secret: secret.to_string(),
            context: Box::new(RequestContextClaims {
                principal: principal.into(),
                agent_id: principal.into(),
                tenant: "tenant-test".into(),
                audience: "medulla".into(),
                policy_version: "test".into(),
                ..RequestContextClaims::default()
            }),
            uql: BASE.into(),
            cypher: String::new(),
            id_field: "id".into(),
        }
    }

    /// The spec of the granted principal signing with the peer's own secret.
    fn spec(&self) -> ForeignSourceSpec {
        self.spec_as(PRINCIPAL, &self.secret)
    }
}

/// `MATCH (:Doc)` locally (six documents), joined with the peer; or, with `limit`, the
/// peer as a source followed by `LIMIT`.
fn run(
    spec: ForeignSourceSpec,
    limit: Option<usize>,
    budget: FederationBudget,
) -> (Result<Vec<String>, String>, Vec<FragmentTrace>) {
    let fixture = medulla_exec::fixture::build();
    let session = FederationSession::new(budget);
    let source = Box::new(spec);
    let ops = match limit {
        None => vec![
            Op::Scan {
                label: "Doc".into(),
            },
            Op::ForeignScan { source, join: true },
        ],
        Some(k) => vec![
            Op::ForeignScan {
                source,
                join: false,
            },
            Op::Limit { k },
        ],
    };
    let federated = medulla_federation::federation_opt::FederatedExec::with_session(&session);
    let ctx = PlanCtx::new(&fixture.view, &fixture.semantic).with_foreign_executor(&federated);
    let rows = execute(&Plan::new(ops), &ctx).map(|rows| rows.ids());
    (rows, session.trace())
}

fn join(spec: ForeignSourceSpec) -> (Result<Vec<String>, String>, Vec<FragmentTrace>) {
    run(spec, None, FederationBudget::default())
}

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

const EVERY_ROW: &[&str] = &["d1", "d2", "d4", "d5"];

// spec: MEDULLA-FEDERATED-QUERY-R052
#[test]
fn a_verified_pushdown_narrows_the_transfer_without_changing_the_answer() {
    let peer = Peer::spawn(Policy::granting(EVERY_ROW));
    let (rows, trace) = join(peer.spec());
    assert_eq!(rows.unwrap(), ids(EVERY_ROW));
    assert_eq!(trace[0].strategy, FetchStrategy::BindJoin);
    assert_eq!(trace[0].keys_pushed, 6, "the six local documents");
    let judged = peer.judged();
    let [Judged::Served { text, rows, .. }] = &judged[..] else {
        panic!("one served request: {judged:?}")
    };
    assert_ne!(text, BASE, "the key filter was appended");
    assert_eq!(rows, &ids(EVERY_ROW));

    let (limited, trace) = run(peer.spec(), Some(2), FederationBudget::default());
    assert_eq!(limited.unwrap(), ids(&EVERY_ROW[..2]));
    assert_eq!(trace[0].limit_pushed, Some(2));
    let Some(Judged::Served { rows, .. }) = peer.judged().pop() else {
        panic!("the limited request was served")
    };
    assert_eq!(rows.len(), 2, "two rows crossed, not four");
}

/// The join through `spec` fails, and the peer judged every request it received — the
/// pushed ones and the fallback alike — as `verdict`: no row was served.
fn assert_every_request_refused(peer: &Peer, spec: ForeignSourceSpec, verdict: Judged) {
    let (rows, _) = join(spec);
    assert!(rows.is_err(), "{rows:?}");
    let judged = peer.judged();
    assert!(!judged.is_empty());
    assert!(judged.iter().all(|j| *j == verdict), "{judged:?}");
}

#[test]
fn a_request_signed_with_another_secret_is_refused_and_serves_no_row() {
    let peer = Peer::spawn(Policy::granting(EVERY_ROW));
    let forged = peer.spec_as(PRINCIPAL, &fixture_secret("forged"));
    assert_every_request_refused(&peer, forged, Judged::Unauthenticated);
}

#[test]
fn the_envelope_binds_the_secret_the_statement_the_graph_and_the_request_id() {
    let peer = Peer::spawn(Policy::granting(EVERY_ROW));
    join(peer.spec()).0.unwrap();
    let genuine = peer.seen.lock().expect("peer record lock").requests[0].clone();
    assert!(verified_claims(&peer.secret, &genuine).is_some());
    assert!(verified_claims(&fixture_secret("forged"), &genuine).is_none());

    let mut widened = genuine.clone();
    widened.method = Method::Uql {
        text: "MATCH (:Doc)".into(),
        params: Default::default(),
        prepare: false,
        foreign_execution: None,
    };
    assert!(
        verified_claims(&peer.secret, &widened).is_none(),
        "a token minted for the pushed statement does not authorize another one"
    );
    let mut regraphed = genuine.clone();
    regraphed.graph = "another-graph".into();
    assert!(verified_claims(&peer.secret, &regraphed).is_none());
    let mut replayed = genuine.clone();
    replayed.id = genuine.id.wrapping_add(1);
    assert!(verified_claims(&peer.secret, &replayed).is_none());
    let mut impersonating = genuine;
    impersonating.agent_id = Some("agent:someone-else".into());
    assert!(verified_claims(&peer.secret, &impersonating).is_none());
}

#[test]
fn a_verified_principal_without_a_grant_is_refused() {
    let peer = Peer::spawn(Policy::granting(EVERY_ROW));
    let stranger = peer.spec_as("agent:stranger", &peer.secret);
    assert_every_request_refused(&peer, stranger, Judged::Forbidden);
}

#[test]
fn a_pushed_key_outside_the_grant_is_withheld_by_the_peer() {
    let granted = ["d1", "d2", "d5"];
    let peer = Peer::spawn(Policy::granting(&granted));
    let (rows, trace) = join(peer.spec());
    assert_eq!(
        rows.unwrap(),
        ids(&granted),
        "the join the caller is entitled to: `d4` is local, pushed, and not granted"
    );
    assert_eq!(trace[0].strategy, FetchStrategy::BindJoin);
    let judged = peer.judged();
    let [Judged::Served {
        text,
        rows,
        withheld,
    }] = &judged[..]
    else {
        panic!("one served request: {judged:?}")
    };
    assert!(text.contains("d4"), "the ungranted key was pushed: {text}");
    assert_eq!(rows, &ids(&granted));
    assert_eq!(withheld, &ids(&["d4"]), "and the peer refused that row");

    let naive = Peer::spawn(Policy {
        pushdown: false,
        ..Policy::granting(&granted)
    });
    assert_eq!(
        join(naive.spec()).0.unwrap(),
        ids(&granted),
        "the unpushed route returns the same rows"
    );
}

#[test]
fn a_peer_without_the_capability_falls_back_to_the_bounded_base_statement() {
    let policy = || Policy {
        pushdown: false,
        ..Policy::granting(EVERY_ROW)
    };
    let peer = Peer::spawn(policy());
    let (rows, trace) = join(peer.spec());
    assert_eq!(rows.unwrap(), ids(EVERY_ROW), "the local residual decides");
    assert_eq!(trace[0].strategy, FetchStrategy::FallbackFullFetch);
    let judged = peer.judged();
    let (served, refused) = judged.split_last().expect("requests reached the peer");
    assert!(
        matches!(served, Judged::Served { text, rows, .. } if text == BASE && rows.len() == 4),
        "{served:?}"
    );
    assert_eq!(
        refused,
        [
            Judged::Unsupported,
            Judged::Unsupported,
            Judged::Unsupported
        ],
        "three shrinking key batches were refused, then key lookups were given up"
    );

    let limited = Peer::spawn(policy());
    let (rows, trace) = run(limited.spec(), Some(2), FederationBudget::default());
    assert_eq!(
        rows.unwrap(),
        ids(&EVERY_ROW[..2]),
        "the outer LIMIT still holds"
    );
    assert_eq!(trace[0].strategy, FetchStrategy::FallbackFullFetch);
    assert_eq!(limited.judged()[0], Judged::Unsupported);

    let bounded = Peer::spawn(policy());
    let tight = FederationBudget {
        max_rows: 3,
        ..FederationBudget::default()
    };
    let (rows, _) = run(bounded.spec(), None, tight);
    let error = rows.expect_err("the fallback's four rows exceed a three-row budget");
    assert!(
        error.starts_with(&format!("{BUDGET_EXCEEDED}:rows")),
        "{error}"
    );
}

mod engine_remote_tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use medulla_types::acl::RequestContextClaims;
    use medulla_types::wire::ForeignSourceSpec;

    use medulla_federation::federation_opt::capability::RemoteRequest;
    use medulla_federation::federation_opt::engine::EngineRemote;
    use medulla_federation::federation_opt::remote::RemoteFetch;
    use medulla_exec::exec::{execute, PlanCtx};
    use super::verified_claims;
    use medulla_federation::federation_opt::remote::Identity;

    fn remote(query: &str) -> ForeignSourceSpec {
        ForeignSourceSpec::RemoteEngine {
            endpoint: "127.0.0.1:1".into(),
            graph: "g".into(),
            secret: "test-secret".into(), // sanitizer:ignore test fixture, not a credential
            context: Box::new(RequestContextClaims::default()),
            uql: query.into(),
            cypher: String::new(),
            id_field: "id".into(),
        }
    }

    #[test]
    fn pushed_keys_and_limit_match_full_peer_execution() {
        let spec = remote("MATCH (:Doc) |> WHERE year > 2023");
        let peer = EngineRemote::new(&spec, Identity::of_spec(&spec, None));
        let fixture = medulla_exec::fixture::build();
        let ctx = PlanCtx::new(&fixture.view, &fixture.semantic);
        let full = execute(
            &medulla_uql::parse("MATCH (:Doc) |> WHERE year > 2023").unwrap(),
            &ctx,
        )
        .unwrap();
        let request = RemoteRequest::keys(vec!["d2".into(), "d4".into()]);
        let text = peer.render(&request).unwrap();
        let pushed = execute(&medulla_uql::parse(&text).unwrap(), &ctx).unwrap();
        let wanted = full
            .ids()
            .into_iter()
            .filter(|id| id == "d2" || id == "d4")
            .collect::<Vec<_>>();
        assert_eq!(pushed.ids(), wanted);
        assert!(pushed.len() < full.len(), "the peer serves fewer rows");

        let limited = peer
            .render(&RemoteRequest {
                limit: Some(2),
                ..RemoteRequest::full()
            })
            .unwrap();
        let first_two = execute(&medulla_uql::parse(&limited).unwrap(), &ctx).unwrap();
        assert_eq!(first_two.ids(), full.ids()[..2]);
    }

    #[test]
    fn synthetic_and_non_pipeline_results_are_fetch_only() {
        for query in [
            "DECISIONS",
            "PROFILE MATCH (:Doc)",
            "MATCH (:Doc) WITH PROOF",
        ] {
            let spec = remote(query);
            let peer = EngineRemote::new(&spec, Identity::of_spec(&spec, None));
            assert!(peer.capabilities().max_keys().is_none());
        }
    }

    #[test]
    fn key_literals_are_quoted_and_parse_as_one_literal() {
        let spec = remote("MATCH (:Doc)");
        let peer = EngineRemote::new(&spec, Identity::of_spec(&spec, None));
        let key = "x' |> LIMIT 0".to_string();
        let request = RemoteRequest::keys(vec![key.clone()]);
        let parsed = medulla_uql::parse(&peer.render(&request).unwrap()).unwrap();
        assert!(
            matches!(&parsed.ops[1], medulla_types::wire::Op::Filter { preds }
            if matches!(&preds[..], [medulla_types::wire::Pred::In { values, .. }]
                if values == &vec![medulla_types::wire::PredLiteral::Str(key)]))
        );
    }

    #[test]
    fn framed_peer_serves_equivalent_rows_with_less_transfer() {
        use medulla_protocol_types::{
            protocol::Method, protocol::Request, protocol::Response, protocol::ResultPayload,
        };

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let (done, served_counts) = std::sync::mpsc::sync_channel(1);
        let ForeignSourceSpec::RemoteEngine {
            secret: signing_secret,
            ..
        } = remote("")
        else {
            unreachable!()
        };
        std::thread::spawn(move || {
            let fixture = medulla_exec::fixture::build();
            let ctx = PlanCtx::new(&fixture.view, &fixture.semantic);
            let mut served = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut header = [0u8; 4];
                stream.read_exact(&mut header).unwrap();
                let mut body = vec![0; u32::from_be_bytes(header) as usize];
                stream.read_exact(&mut body).unwrap();
                let request: Request = rmp_serde::from_slice(&body).unwrap();
                assert!(
                    verified_claims(&signing_secret, &request).is_some(),
                    "the peer serves only a request whose envelope verifies"
                );
                let Method::Uql { text, params, .. } = request.method else {
                    panic!("the peer must receive UQL");
                };
                assert!(params.is_empty());
                let statement = medulla_uql::parse_statement(&text, &params).unwrap();
                let result = crate::uql::serve::run_statement(&statement, &ctx).unwrap();
                let medulla_types::wire::UqlResult::Rows { rows, .. } = &result else {
                    panic!("rows expected");
                };
                served.push(rows.len());
                let payload = ResultPayload::Raw(rmp_serde::to_vec_named(&result).unwrap());
                let response = Response::ok(request.id, payload);
                let encoded = rmp_serde::to_vec_named(&response).unwrap();
                stream
                    .write_all(&(encoded.len() as u32).to_be_bytes())
                    .unwrap();
                stream.write_all(&encoded).unwrap();
            }
            done.send(served).unwrap();
        });

        let mut spec = remote("MATCH (:Doc) |> WHERE year > 2023");
        let ForeignSourceSpec::RemoteEngine {
            endpoint: host,
            context,
            ..
        } = &mut spec
        else {
            unreachable!()
        };
        *host = endpoint;
        context.principal = "agent:peer-test".into();
        context.agent_id = "agent:peer-test".into();
        context.tenant = "tenant-test".into();
        context.audience = "medulla".into();
        context.policy_version = "test".into();
        let peer = EngineRemote::new(&spec, Identity::of_spec(&spec, None));
        let full = peer.fetch(&RemoteRequest::full()).unwrap();
        let selected = peer
            .fetch(&RemoteRequest::keys(vec!["d2".into(), "d4".into()]))
            .unwrap();
        let limited = peer
            .fetch(&RemoteRequest {
                limit: Some(2),
                ..RemoteRequest::full()
            })
            .unwrap();
        assert_eq!(selected.ids(), vec!["d2", "d4"]);
        assert_eq!(limited.ids(), full.ids()[..2]);
        let served = served_counts
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the peer served all three requests");
        assert_eq!(served, vec![full.len(), 2, 2]);
    }
}
