# MEDULLA-CLUSTER — Quickstart

Governing spec: [spec.md](spec.md).

## Cluster rows, served

1. Send `Method::DsKMeans { data, k: 2, max_iter: 100 }` with two separated blobs.
2. Read one label per blob and send the call again. The labels repeat.
3. Send `k: 5` with two rows. The answer text starts with `CLUSTERING_K_EXCEEDS_ROWS`.

Proof command: `cargo test -p medulla-server-served-tests clustering_kernel`.

## Tune a run

Pin knobs in the decision policy: `cluster.batch_rows`, `cluster.epochs`, `cluster.threads`. A revision that moves `cluster.max_steps` is refused with `KNOB_NOT_TUNABLE`.

## Benchmark both modes

`cargo bench -p medulla-ann --bench recall -- 100000 128 1024 32 32 200 emeans`, then the same with `exact`. Compare `build_s`, `inertia`, `peak_rss_mb` and recall@10.
