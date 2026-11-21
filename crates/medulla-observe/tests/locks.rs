//! Lock sites: holders, waiters and the longest wait.

use std::sync::Arc;
use std::time::Duration;

use medulla_observe::registry::locks::LockRegistry;
use tokio::sync::Mutex;

// spec: MEDULLA-OBSERVE-R006.4
#[tokio::test(flavor = "multi_thread")]
async fn a_held_lock_shows_its_holder_and_a_waiter() {
    let registry = LockRegistry::new();
    let lock = Arc::new(Mutex::new(()));
    let site = registry.site("lock_graph:g");
    let held = site.acquire(Arc::clone(&lock).lock_owned()).await;
    let row = registry.snapshot().remove(0);
    assert_eq!(row.name, "lock_graph:g");
    assert!(row.holder.is_some());
    assert_eq!(row.waiters, 0);

    let waiter_site = registry.site("lock_graph:g");
    let waiter_lock = Arc::clone(&lock);
    let waiter = tokio::spawn(async move {
        let held = waiter_site.acquire(waiter_lock.lock_owned()).await;
        drop(held);
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while registry.snapshot()[0].waiters == 0 {
        assert!(std::time::Instant::now() < deadline, "the waiter never queued");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    drop(held);
    waiter.await.unwrap();

    let row = registry.snapshot().remove(0);
    assert!(row.holder.is_none(), "a released lock has no holder");
    assert_eq!(row.waiters, 0);
    assert!(row.longest_wait_ms >= 20, "the wait was recorded: {row:?}");
}

// spec: MEDULLA-OBSERVE-R006.4
#[tokio::test]
async fn a_dropped_waiter_is_not_left_counted() {
    let registry = LockRegistry::new();
    let lock = Arc::new(Mutex::new(()));
    let site = registry.site("s");
    let held = site.acquire(Arc::clone(&lock).lock_owned()).await;
    let waiting = site.acquire(Arc::clone(&lock).lock_owned());
    let timed_out = tokio::time::timeout(Duration::from_millis(20), waiting).await;
    assert!(timed_out.is_err());
    assert_eq!(registry.snapshot()[0].waiters, 0);
    drop(held);
}

// spec: MEDULLA-OBSERVE-R006.4
#[tokio::test]
async fn a_shared_lock_records_waiters_without_a_holder() {
    let registry = LockRegistry::new();
    let site = registry.site("server_state");
    let value = site.wait(async { 5 }).await;
    assert_eq!(value, 5);
    let row = registry.snapshot().remove(0);
    assert!(row.holder.is_none());
    assert_eq!(row.waiters, 0);
}
