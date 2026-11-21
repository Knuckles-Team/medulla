//! Attached-source conformance (MEDULLA-UNIFIED-DATA-PLANE-R022): the change-capture
//! replay of a recorded Debezium topic against the source's own row state.

use std::collections::BTreeMap;

use medulla_catalog_types::dialect_conformance::ConformanceOutcome;
use medulla_catalog_types::replay_conformance::compare_replay_to_snapshot;
use medulla_protocol_types::debezium_bridge::{DebeziumChangeEvent, DebeziumOp};
use medulla_protocol_types::protocol::Method;
use serde_json::Value;
use sha2::{Digest, Sha256};

const REPLAY_FIXTURE: &str = include_str!("../conformance/fixtures/replay_postgres16.jsonl");
/// The engine the fixture was recorded from.
const ENGINE: &str = "postgres";
const ENGINE_VERSION: &str = "16";
/// Rows the fixture's table holds before the mutations, and the mutations that follow.
const BASE_ROWS: usize = 1_000;
const MUTATIONS: usize = 200;

fn fixture_events() -> Vec<DebeziumChangeEvent> {
    REPLAY_FIXTURE
        .lines()
        .map(|line| serde_json::from_str(line).expect("a fixture line is a Debezium event"))
        .collect()
}

/// The row state the replay leaves: every event converted by the Debezium bridge
/// into a draft, the draft's graph operation applied in stream order.
fn replayed_state(events: &[DebeziumChangeEvent]) -> BTreeMap<String, Value> {
    let mut state = BTreeMap::new();
    for (ordinal, event) in events.iter().enumerate() {
        let draft = event
            .to_draft(format!("env-{ordinal}"), format!("batch-{ordinal}"))
            .expect("every fixture event carries the row its op needs");
        for operation in draft.mutation.operations {
            match operation.method {
                Method::AddNode {
                    node_id,
                    properties_msgpack,
                } => {
                    let row: Value =
                        rmp_serde::from_slice(&properties_msgpack).expect("a row decodes");
                    state.insert(node_id, row);
                }
                Method::RemoveNode { node_id } => {
                    state.remove(&node_id);
                }
                other => panic!("the bridge emits only node operations, got {other:?}"),
            }
        }
    }
    state
}

/// The source's own row state at the end of the stream: the recorded rows folded by
/// primary key without the bridge, as the source table would hold them.
fn snapshot_state(events: &[DebeziumChangeEvent]) -> BTreeMap<String, Value> {
    let mut table: BTreeMap<String, Value> = BTreeMap::new();
    for event in events {
        let row = event
            .after
            .as_ref()
            .or(event.before.as_ref())
            .expect("a row");
        let key = format!(
            "{}.{}:{}",
            event.source.db,
            event.source.table,
            row.get("id").expect("a primary key")
        );
        match event.op {
            DebeziumOp::Delete => {
                table.remove(&key);
            }
            _ => {
                table.insert(key, row.clone());
            }
        }
    }
    table
}

/// One SHA-256 (64 lowercase hex) over the rows sorted by primary key.
fn rows_digest(state: &BTreeMap<String, Value>) -> String {
    let mut hasher = Sha256::new();
    for (key, row) in state {
        hasher.update(key.as_bytes());
        hasher.update([0]);
        hasher.update(row.to_string().as_bytes());
        hasher.update([b'\n']);
    }
    hex::encode(hasher.finalize())
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R022.3.2
#[test]
fn replay_equals_snapshot() {
    let events = fixture_events();
    assert_eq!(events.len(), BASE_ROWS + MUTATIONS);
    let ops = |op: DebeziumOp| events[BASE_ROWS..].iter().filter(|e| e.op == op).count();
    assert!(ops(DebeziumOp::Create) > 0 && ops(DebeziumOp::Update) > 0);
    assert!(ops(DebeziumOp::Delete) > 0);

    let snapshot = snapshot_state(&events);
    let replayed = replayed_state(&events);
    let (replay_digest, snapshot_digest) = (rows_digest(&replayed), rows_digest(&snapshot));
    assert_eq!(replay_digest.len(), 64);
    assert!(replay_digest
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert_eq!(replay_digest, snapshot_digest);
    let entry = compare_replay_to_snapshot(ENGINE, ENGINE_VERSION, &replayed, &snapshot)
        .expect("a named engine version");
    assert_eq!(entry.outcome, ConformanceOutcome::Green);

    // A stream that lost one event no longer equals the source: a failing entry.
    let mut dropped = events.clone();
    dropped.remove(BASE_ROWS + 2);
    let short = replayed_state(&dropped);
    assert_ne!(rows_digest(&short), snapshot_digest);
    let failing = compare_replay_to_snapshot(ENGINE, ENGINE_VERSION, &short, &snapshot)
        .expect("a named engine version");
    assert_eq!(failing.outcome, ConformanceOutcome::Deviated);
    assert!(
        failing.validate().is_err(),
        "a deviation still needs a reviewer"
    );
}

// ---- MEDULLA-UNIFIED-DATA-PLANE-R022.2: the matrix manifest and the runner entry ----

const MATRIX: &str = include_str!("../conformance/attached_matrix.toml");
/// The pipelines workflow writes the image digests to this file, outside the repository.
const IMAGE_MANIFEST_VAR: &str = "MEDULLA_CONFORMANCE_IMAGE_MANIFEST";

/// One engine version of the matrix. It has no digest: the workflow pins it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixEngine {
    pub dialect: String,
    pub version: String,
    pub image: Option<String>,
    pub dsn_env: Option<String>,
}

/// One image the workflow pulled, with the digest it resolved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ImagePin {
    pub image: String,
    pub digest: String,
}

/// What a live runner does for one engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineRun {
    Run { dsn: String, digest: Option<String> },
    Skip(String),
}

fn parse_matrix(text: &str) -> Vec<MatrixEngine> {
    let document: toml_edit::DocumentMut = text.parse().expect("the matrix is TOML");
    let tables = document["engine"]
        .as_array_of_tables()
        .expect("the matrix holds [[engine]] tables");
    tables
        .iter()
        .map(|table| {
            let text_of = |key: &str| table.get(key).map(|v| v.as_str().unwrap().to_string());
            for key in table.iter().map(|(key, _)| key) {
                assert!(
                    ["dialect", "version", "image", "dsn_env"].contains(&key),
                    "unexpected matrix key `{key}`"
                );
            }
            MatrixEngine {
                dialect: text_of("dialect").expect("an engine names its dialect"),
                version: text_of("version").expect("an engine names its version"),
                image: text_of("image"),
                dsn_env: text_of("dsn_env"),
            }
        })
        .collect()
}

fn well_formed_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The digest the workflow pinned for an image-backed engine: `Ok(None)` for an engine with
/// no image, `Ok(Some(digest))` for exactly one well-formed pin of the same image.
pub fn pinned_digest<'a>(
    engine: &MatrixEngine,
    pins: &'a [ImagePin],
) -> Result<Option<&'a str>, String> {
    let Some(image) = engine.image.as_deref() else {
        return Ok(None);
    };
    let mut matching = pins.iter().filter(|pin| pin.image == image);
    match (matching.next(), matching.next()) {
        (Some(pin), None) if well_formed_digest(&pin.digest) => Ok(Some(pin.digest.as_str())),
        _ => Err(format!("IMAGE_UNPINNED: {image}")),
    }
}

/// Whether a live runner starts for `engine`. `pins` is `None` when the manifest variable is
/// unset; `env` reads the engine's DSN variable.
pub fn plan_engine(
    engine: &MatrixEngine,
    pins: Option<&[ImagePin]>,
    env: &dyn Fn(&str) -> Option<String>,
) -> EngineRun {
    let (Some(_), Some(dsn_env)) = (engine.image.as_deref(), engine.dsn_env.as_deref()) else {
        return EngineRun::Run {
            dsn: String::new(),
            digest: None,
        };
    };
    let Some(pins) = pins else {
        return EngineRun::Skip("image manifest unset".to_string());
    };
    let Some(dsn) = env(dsn_env).filter(|dsn| !dsn.is_empty()) else {
        return EngineRun::Skip(format!("{dsn_env} unset"));
    };
    match pinned_digest(engine, pins) {
        Ok(digest) => EngineRun::Run {
            dsn,
            digest: digest.map(str::to_string),
        },
        Err(refusal) => EngineRun::Skip(refusal),
    }
}

/// The `engine_version` a conformance entry carries: the version, with the pinned image
/// digest when there is one, so a green entry names the exact image it ran.
pub fn entry_engine_version(engine: &MatrixEngine, digest: Option<&str>) -> String {
    match digest {
        Some(digest) => format!("{}@{digest}", engine.version),
        None => engine.version.clone(),
    }
}

fn pin(image: &str, digest: &str) -> ImagePin {
    ImagePin {
        image: image.to_string(),
        digest: digest.to_string(),
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R022.2
#[test]
fn manifest_pins_every_image_digest() {
    let engines = parse_matrix(MATRIX);
    assert_eq!(engines.len(), 5);
    let dialects: Vec<&str> = engines.iter().map(|e| e.dialect.as_str()).collect();
    assert_eq!(
        dialects,
        ["postgres", "mysql", "mariadb", "clickhouse", "sqlite"]
    );
    let unique: std::collections::BTreeSet<&str> = dialects.iter().copied().collect();
    assert_eq!(unique.len(), dialects.len(), "dialects are unique");
    // `parse_matrix` refuses any key but the four, so a `digest` key cannot parse.
    for engine in &engines {
        if engine.image.is_some() {
            assert_eq!(
                engine.dsn_env.as_deref(),
                Some(format!("MEDULLA_CONFORMANCE_{}_DSN", engine.dialect.to_uppercase()).as_str())
            );
        } else {
            assert_eq!(engine.dialect, "sqlite");
            assert!(engine.dsn_env.is_none());
        }
    }

    let good = format!("sha256:{}", "ab12".repeat(16));
    let postgres = &engines[0];
    let sqlite = &engines[4];
    let pins = vec![pin("postgres:16", &good)];
    assert_eq!(pinned_digest(postgres, &pins), Ok(Some(good.as_str())));
    assert_eq!(
        pinned_digest(&engines[1], &pins),
        Err("IMAGE_UNPINNED: mysql:8.4".to_string())
    );
    let duplicate = vec![pin("postgres:16", &good), pin("postgres:16", &good)];
    assert_eq!(
        pinned_digest(postgres, &duplicate),
        Err("IMAGE_UNPINNED: postgres:16".to_string())
    );
    let upper = vec![pin("postgres:16", &good.to_uppercase().replace("SHA256", "sha256"))];
    assert_eq!(
        pinned_digest(postgres, &upper),
        Err("IMAGE_UNPINNED: postgres:16".to_string())
    );
    assert_eq!(pinned_digest(sqlite, &[]), Ok(None));

    let dsn_set = |name: &str| (name == "MEDULLA_CONFORMANCE_POSTGRES_DSN").then(|| "dsn".to_string());
    let dsn_empty = |_: &str| Some(String::new());
    let none = |_: &str| None;
    assert_eq!(
        plan_engine(postgres, None, &dsn_set),
        EngineRun::Skip("image manifest unset".to_string())
    );
    assert_eq!(
        plan_engine(postgres, Some(&pins), &none),
        EngineRun::Skip("MEDULLA_CONFORMANCE_POSTGRES_DSN unset".to_string())
    );
    assert_eq!(
        plan_engine(postgres, Some(&pins), &dsn_empty),
        EngineRun::Skip("MEDULLA_CONFORMANCE_POSTGRES_DSN unset".to_string())
    );
    assert_eq!(
        plan_engine(postgres, Some(&[]), &dsn_set),
        EngineRun::Skip("IMAGE_UNPINNED: postgres:16".to_string())
    );
    assert_eq!(
        plan_engine(postgres, Some(&pins), &dsn_set),
        EngineRun::Run {
            dsn: "dsn".to_string(),
            digest: Some(good.clone())
        }
    );
    assert_eq!(
        plan_engine(sqlite, None, &none),
        EngineRun::Run {
            dsn: String::new(),
            digest: None
        }
    );
    assert_eq!(
        entry_engine_version(postgres, Some(&good)),
        format!("16@{good}")
    );
    assert_eq!(entry_engine_version(sqlite, None), "3");

    // The nightly run: every image entry is pinned in the workflow's manifest.
    match std::env::var(IMAGE_MANIFEST_VAR) {
        Ok(path) => {
            let manifest: Vec<ImagePin> = serde_json::from_str(
                &std::fs::read_to_string(&path).expect("the image manifest is readable"),
            )
            .expect("the image manifest is the documented JSON");
            for engine in engines.iter().filter(|e| e.image.is_some()) {
                assert!(
                    matches!(pinned_digest(engine, &manifest), Ok(Some(_))),
                    "{} is pinned in the manifest",
                    engine.dialect
                );
            }
        }
        Err(_) => println!("skip: image manifest unset"),
    }
}
