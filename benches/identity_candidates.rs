//! `decide/resolve_entity_blocking` (MEDULLA-UNIFIED-DATA-PLANE-R030.701): candidate
//! generation over 10,000 records with blocking. Budget: p50 20 ms, p99 100 ms,
//! memory 128 MiB. A bench never gates a pull request.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use medulla_index_types::entity_resolution::{
    propose_links, FellegiSunterModel, FieldWeight, ResolutionRecord,
};
use medulla_types::entity_link_proposal::{AttachedApplication, EntityRef};

fn records(count: usize) -> Vec<ResolutionRecord> {
    (0..count)
        .map(|n| ResolutionRecord {
            entity: EntityRef {
                application: if n % 2 == 0 {
                    AttachedApplication::Twenty
                } else {
                    AttachedApplication::Erpnext
                },
                entity_id: format!("e{n}"),
            },
            name: format!("name{:04} family{:03}", n / 2 % 5_000, n % 500),
            fields: BTreeMap::new(),
        })
        .collect()
}

fn main() {
    let records = records(10_000);
    let model = FellegiSunterModel {
        fields: Vec::<FieldWeight>::new(),
        upper: 1.0,
        lower: -1.0,
    };
    let mut samples = Vec::new();
    for _ in 0..21 {
        let started = Instant::now();
        black_box(propose_links(black_box(&records), &model));
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    println!(
        "decide/resolve_entity_blocking p50={}us p99={}us",
        samples[samples.len() / 2],
        samples[samples.len() - 1]
    );
}
