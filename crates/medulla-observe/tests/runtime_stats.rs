//! The runtime sampler.

use std::time::Duration;

use medulla_observe::runtime_stats::RuntimeSampler;

// spec: MEDULLA-OBSERVE-R006.3
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_worker_is_flagged_after_two_windows() {
    let sampler = RuntimeSampler::with_window(Duration::from_millis(100));
    sampler.set_spin_busy_ratio(0.5);
    let handle = tokio::runtime::Handle::current();
    let first = sampler.snapshot(&handle);
    assert_eq!(first.workers.len(), 2);
    assert!(first.workers.iter().all(|worker| !worker.spinning));
    // Keep both workers busy for several windows.
    let spinners: Vec<_> = (0..2)
        .map(|_| {
            tokio::spawn(async {
                let until = std::time::Instant::now() + Duration::from_millis(600);
                while std::time::Instant::now() < until {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    let mut flagged = false;
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(120)).await;
        let snapshot = sampler.snapshot(&handle);
        flagged |= snapshot.workers.iter().any(|worker| worker.spinning);
    }
    for spinner in spinners {
        spinner.await.unwrap();
    }
    assert!(flagged, "a worker busy across two windows must be flagged");
}

// spec: MEDULLA-OBSERVE-R006.3
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_runtime_reports_no_spinning_worker() {
    let sampler = RuntimeSampler::with_window(Duration::from_millis(50));
    let handle = tokio::runtime::Handle::current();
    sampler.snapshot(&handle);
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        let snapshot = sampler.snapshot(&handle);
        assert!(snapshot.workers.iter().all(|worker| !worker.spinning));
        assert!(snapshot
            .workers
            .iter()
            .all(|worker| (0.0..=1.0).contains(&worker.busy_ratio)));
    }
}
