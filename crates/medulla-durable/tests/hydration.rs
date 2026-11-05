use std::collections::BTreeMap;

use medulla_durable::{
    classify_file, plan_baseline, split_repository, BaselinePlan, DeltaManifest,
    DeltaManifestEntry, DeltaManifestKey, FileClass, HydrationEvidence, HydrationModelError,
    HydrationSource, RepoFiles, RepoSplitPlan, StagedStage,
};
use medulla_types::scheduler_config::SchedulerConfig;

fn config() -> SchedulerConfig {
    serde_json::from_value(serde_json::json!({
        "total_workers": 4,
        "max_queue_depth": 100
    }))
    .unwrap()
}

fn entry(uri: &str, hash: &str) -> DeltaManifestEntry {
    DeltaManifestEntry {
        key: DeltaManifestKey {
            graph: "workspace".into(),
            category: "source".into(),
            uri: uri.into(),
        },
        content_hash: hash.into(),
    }
}

fn round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

// spec: MEDULLA-REPO-INGEST-R650.1
#[test]
fn types_round_trip_and_manifest_digest_is_order_independent() {
    let mut forward = DeltaManifest::new("tenant-a");
    forward.record(entry("repo://a", "sha256:a")).unwrap();
    forward.record(entry("repo://b", "sha256:b")).unwrap();
    let mut backward = DeltaManifest::new("tenant-a");
    backward.record(entry("repo://b", "sha256:b")).unwrap();
    backward.record(entry("repo://a", "sha256:a")).unwrap();
    assert_eq!(forward.manifest_digest, backward.manifest_digest);
    assert_eq!(forward, backward);
    forward.validate().unwrap();

    // The digest is tenant-scoped and content-sensitive.
    let mut other_tenant = DeltaManifest::new("tenant-b");
    other_tenant.record(entry("repo://a", "sha256:a")).unwrap();
    other_tenant.record(entry("repo://b", "sha256:b")).unwrap();
    assert_ne!(forward.manifest_digest, other_tenant.manifest_digest);
    let mut changed = forward.clone();
    changed.record(entry("repo://a", "sha256:z")).unwrap();
    assert_ne!(forward.manifest_digest, changed.manifest_digest);

    // A tampered digest is refused.
    let mut tampered = forward.clone();
    tampered.entries[0].content_hash = "sha256:evil".into();
    assert_eq!(
        tampered.validate().unwrap_err(),
        HydrationModelError::ManifestDigestMismatch
    );

    assert_eq!(round_trip(&forward), forward);

    let plan = plan_baseline(
        &[HydrationSource {
            tenant_id: "tenant-a".into(),
            scope: "ws".into(),
            items: 5,
        }],
        &config(),
    )
    .unwrap();
    assert_eq!(round_trip(&plan), plan);
    let evidence = HydrationEvidence {
        tenant_id: "tenant-a".into(),
        plan_digest: plan.digest(),
        split_weights: BTreeMap::from([("src".to_string(), 3)]),
        skipped: 2,
        changed: 1,
        coverage_by_class: BTreeMap::from([(FileClass::Source, 3), (FileClass::Binary, 1)]),
    };
    evidence.validate().unwrap();
    assert_eq!(round_trip(&evidence), evidence);

    let split = RepoSplitPlan {
        tenant_id: "tenant-a".into(),
        repository_uri: "repo://a".into(),
        directory_weights: BTreeMap::from([("src".to_string(), 3)]),
        parts: vec![vec!["src".into()]],
    };
    assert_eq!(round_trip(&split), split);

    let classes = [
        (FileClass::Source, "\"source\""),
        (FileClass::Document, "\"document\""),
        (FileClass::Config, "\"config\""),
        (FileClass::Binary, "\"binary\""),
        (FileClass::Data, "\"data\""),
        (FileClass::Unknown, "\"unknown\""),
    ];
    for (class, name) in classes {
        assert_eq!(serde_json::to_string(&class).unwrap(), name);
    }
    let _: StagedStage = round_trip(&StagedStage::Reason);
    let _: BaselinePlan = plan;
}

fn repo(thread_shuffle: usize) -> RepoFiles {
    let mut paths: Vec<String> = Vec::new();
    for dir in 0..40 {
        for file in 0..(dir % 7 + 1) * 3 {
            paths.push(format!("d{dir:02}/f{file}.rs"));
        }
    }
    paths.push("README.md".into());
    let shift = thread_shuffle % paths.len();
    paths.rotate_left(shift);
    RepoFiles {
        tenant_id: "tenant-a".into(),
        repository_uri: "repo://big".into(),
        paths,
    }
}

// spec: MEDULLA-REPO-INGEST-R650.2
#[test]
fn plan_is_deterministic_across_thread_counts() {
    let expected = split_repository(&repo(0), 100).unwrap();
    let (sender, receiver) = std::sync::mpsc::sync_channel(8);
    for n in 1..=8usize {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let _ = sender.send(split_repository(&repo(n * 17), 100).unwrap());
        });
    }
    for _ in 0..8 {
        let plan = receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("split worker finished within the deadline");
        assert_eq!(plan, expected);
    }
    // Every file is accounted for and every part respects the target.
    let total: u64 = expected.directory_weights.values().sum();
    assert_eq!(total, repo(0).paths.len() as u64);
    for part in &expected.parts {
        let weight: u64 = part.iter().map(|d| expected.directory_weights[d]).sum();
        assert!(weight <= 100 || part.len() == 1);
    }
    // Largest directories fill first.
    let first = &expected.parts[0][0];
    let max = expected.directory_weights.values().max().unwrap();
    assert_eq!(expected.directory_weights[first], *max);
    assert_eq!(expected.directory_weights["."], 1);

    // plan_baseline is deterministic, caps at the scope cap, and refuses mixed tenants.
    let sources = vec![
        HydrationSource {
            tenant_id: "t".into(),
            scope: "b".into(),
            items: 9_000,
        },
        HydrationSource {
            tenant_id: "t".into(),
            scope: "a".into(),
            items: 9_000,
        },
    ];
    let plan = plan_baseline(&sources, &config()).unwrap();
    assert_eq!(plan.scope, "a,b");
    assert_eq!(plan.max_work_items, 10_000);
    assert_eq!(plan, plan_baseline(&sources, &config()).unwrap());
    let mut mixed = sources.clone();
    mixed[1].tenant_id = "u".into();
    assert_eq!(
        plan_baseline(&mixed, &config()).unwrap_err(),
        HydrationModelError::MixedTenants
    );
    assert_eq!(
        plan_baseline(&[], &config()).unwrap_err(),
        HydrationModelError::NoSources
    );
    assert_eq!(
        split_repository(&repo(0), 0).unwrap_err(),
        HydrationModelError::NonPositiveBound("target")
    );
}

// spec: MEDULLA-REPO-INGEST-R650.2
#[test]
fn classification_uses_extension_then_magic_bytes() {
    assert_eq!(classify_file("src/a.rs", b""), FileClass::Source);
    assert_eq!(classify_file("a/README.MD", b""), FileClass::Document);
    assert_eq!(classify_file("Cargo.toml", b""), FileClass::Config);
    assert_eq!(classify_file("x.csv", b""), FileClass::Data);
    // The extension wins over the bytes.
    assert_eq!(classify_file("a.rs", b"%PDF-1.7"), FileClass::Source);
    for magic in [
        &b"%PDF-1"[..],
        b"PK\x03\x04zz",
        b"\x7fELF\x02",
        b"\x89PNG\r\n",
        b"GIF89a",
        b"\xff\xd8\xff\xe0",
    ] {
        assert_eq!(classify_file("blob", magic), FileClass::Binary, "{magic:?}");
        assert_eq!(classify_file("blob.zzz", magic), FileClass::Binary);
    }
    assert_eq!(classify_file("blob", b"hello"), FileClass::Unknown);
    assert_eq!(classify_file(".gitignore", b"target"), FileClass::Unknown);
}
