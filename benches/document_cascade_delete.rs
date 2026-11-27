//! Served `DocumentDelete` of a 500-chunk Document (MEDULLA-UNIFIED-DATA-PLANE-R651.6).
//!
//! Criterion group `document_cascade_delete`: one Document with 500 chunks (each with one
//! extracted fact) built through the served `AddNode` / `AddEdge` methods, then deleted
//! through the served `DocumentDelete`. Budgets (MEDULLA-DECISION-ENGINE-R178): p50 2 ms,
//! p99 12 ms, peak memory 16 MiB. Nightly only; never a PR gate.
//!
//! Run: `cargo bench --features server,security,query --bench document_cascade_delete`

#![allow(dead_code)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use medulla_protocol_types::protocol::Method;
use medulla_query_types::document_ops::DocumentDeleteRequest;
use tokio::runtime::Builder;

const SECRET: &str = "document-cascade-delete-bench-secret";
const GRAPH: &str = "__commons__";
const CHUNKS: u64 = 500;

async fn build_document(state: &test_support::SharedState, id: &mut u64, doc: &str) {
    let mut send = |method: Method| {
        let call = test_support::call_ok(state, SECRET, *id, GRAPH, method);
        *id += 1;
        call
    };
    send(Method::AddNode {
        node_id: doc.to_string(),
        properties_msgpack: test_support::json_bytes(
            serde_json::json!({ "type": "Document", "text": "body" }),
        ),
    })
    .await;
    for ordinal in 0..CHUNKS {
        let chunk = format!("{doc}:chunk:{ordinal}");
        let fact = format!("{doc}:fact:{ordinal}");
        for (node, label) in [(&chunk, "DocumentChunk"), (&fact, "ExtractedFact")] {
            send(Method::AddNode {
                node_id: node.clone(),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({ "type": label, "ordinal": ordinal }),
                ),
            })
            .await;
        }
        for (source, target, relationship) in [
            (&chunk, doc, "chunkOf"),
            (&fact, chunk.as_str(), "extractedFrom"),
        ] {
            send(Method::AddEdge {
                source_id: source.clone(),
                target_id: target.to_string(),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({ "relationship": relationship }),
                ),
            })
            .await;
        }
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R651.6
fn document_cascade_delete(c: &mut Criterion) {
    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let mut next_id = 1u64;
    let mut serial = 0u64;
    let mut group = c.benchmark_group("document_cascade_delete");
    group.bench_function("chunks_500", |b| {
        b.iter_batched(
            || {
                serial += 1;
                let doc = format!("doc:bench:{serial}");
                rt.block_on(build_document(&state, &mut next_id, &doc));
                doc
            },
            |doc| {
                let method = Method::DocumentDelete {
                    request: Box::new(DocumentDeleteRequest {
                        tenant: common::TEST_TENANT.to_string(),
                        document_id: doc,
                        max_nodes: None,
                    }),
                };
                let response = rt.block_on(test_support::dispatch(
                    &state,
                    test_support::request(SECRET, 90_000_000, GRAPH, method),
                ));
                assert!(response.error.is_none(), "{:?}", response.error);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = document_cascade_delete
}
criterion_main!(benches);
