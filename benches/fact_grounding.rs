//! Fact grounding at batch size (MEDULLA-INGEST-WORKER-R701.6).
//!
//! Nightly bench group `ingest/fact_grounding`: `admit_facts` over 10 batches of 1,024 facts, a
//! quarter with subject and object swapped and a quarter naming no ontology class. Budget per
//! batch: p50 below 2 ms, p99 below 12 ms; peak resident memory (`VmHWM`) below 48 MiB. The run
//! exits non-zero when a budget is exceeded. A service level, not a pull request gate.
//!
// spec: MEDULLA-INGEST-WORKER-R701.6
//!
//! Run: cargo bench --bench fact_grounding

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use medulla_ingestion_types::extracted_fact::{ExtractedFact, FactBatch};
use {
    medulla_h_ingest_repository::source_ingestion::facts::admit_facts,
    medulla_h_ingest_repository::source_ingestion::facts::build_grounding_cache,
    medulla_h_ingest_repository::source_ingestion::facts::DecodedFacts,
};

const BATCHES: usize = 10;
const FACTS: usize = 1_024;
const BUDGET_P50: Duration = Duration::from_millis(2);
const BUDGET_P99: Duration = Duration::from_millis(12);
const BUDGET_MIB: f64 = 48.0;

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn ontology() -> String {
    let mut text = String::from(
        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         @prefix ex: <http://example.org/o#> .\n\
         ex:wrote rdfs:domain ex:Person ; rdfs:range ex:Work .\n",
    );
    for index in 0..64 {
        text.push_str(&format!(
            "ex:Person{index} a ex:Person ; rdfs:label \"Person {index}\" .\n\
             ex:Work{index} a ex:Work ; rdfs:label \"Work {index}\" .\n"
        ));
    }
    text
}

fn fact(subject: String, object: String, start: u64) -> ExtractedFact {
    serde_json::from_value(serde_json::json!({
        "subject": subject,
        "predicate": "wrote",
        "object": object,
        "evidence": {"source_ref": "doc:bench", "start": start, "end": start + 20},
        "confidence": 0.9
    }))
    .expect("bench facts are valid")
}

fn main() {
    let cache = build_grounding_cache(&[ontology().as_str()]).expect("bench ontology parses");
    let facts: Vec<ExtractedFact> = (0..FACTS)
        .map(|index| {
            let person = format!("Person {}", index % 64);
            let work = format!("Work {}", index / 16);
            let start = index as u64 * 32;
            match index % 4 {
                0 => fact(work, person, start),
                1 => fact(format!("Unknown {index}"), format!("Other {index}"), start),
                _ => fact(person, work, start),
            }
        })
        .collect();
    let decoded = DecodedFacts {
        batch: FactBatch { facts },
        rejected: Vec::new(),
    };
    let held = BTreeMap::new();
    let mut samples = Vec::with_capacity(BATCHES);
    for _ in 0..BATCHES {
        let started = Instant::now();
        let admission = admit_facts(
            "tenant:bench",
            &decoded,
            &cache,
            &held,
            &BTreeMap::new(),
            2,
            None,
        )
        .expect("bench facts admit");
        samples.push(started.elapsed());
        assert!(admission.admitted > 0);
    }
    samples.sort();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let peak = high_water_mib();
    println!(
        "ingest/fact_grounding p50 = {p50:?} (budget {BUDGET_P50:?}) p99 = {p99:?} (budget {BUDGET_P99:?}) peak_mib = {peak:.1} (budget {BUDGET_MIB})"
    );
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
    assert!(
        peak < BUDGET_MIB,
        "peak {peak:.1} MiB exceeds {BUDGET_MIB} MiB"
    );
}
