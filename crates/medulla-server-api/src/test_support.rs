//! Test-only helpers shared by crate unit-test modules.

/// Decode a successful raw typed server response in route-level tests.
#[cfg(feature = "server")]
pub fn decode_raw_response<T: serde::de::DeserializeOwned>(
    label: &str,
    response: medulla_protocol_types::protocol::Response,
) -> T {
    assert!(
        response.error.is_none(),
        "{label} was refused: {:?}",
        response.error
    );
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = response.result else {
        panic!("{label} did not return a raw typed result");
    };
    rmp_serde::from_slice(&bytes).expect("decode raw typed server response")
}

#[cfg(test)]
#[test]
fn concurrent_temporary_paths_are_unique() {
    let paths = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    (0..256)
                        .map(|_| {
                            medulla_kernel::kernel::persistence::temp_dir(
                                "medulla-path",
                                "parallel",
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    let unique: std::collections::HashSet<_> = paths.iter().collect();
    assert_eq!(unique.len(), paths.len());
}

/// Isolate an exact outbound allowlist for a server fixture and restore it on drop.
/// Both constructors use the durable-fixture environment lock when crypto is enabled.
#[cfg(feature = "federation-sql")]
pub struct FederationAllowGuard {
    previous: Option<std::ffi::OsString>,
    _lock: tokio::sync::RwLockWriteGuard<'static, ()>,
}

#[cfg(feature = "federation-sql")]
impl FederationAllowGuard {
    fn install(origin: &str, lock: tokio::sync::RwLockWriteGuard<'static, ()>) -> Self {
        let key = medulla_federation::federation::HTTP_JSON_FEDERATION_ALLOW_ENV;
        let previous = std::env::var_os(key);
        std::env::set_var(key, origin);
        Self {
            previous,
            _lock: lock,
        }
    }

    fn env_lock() -> &'static tokio::sync::RwLock<()> {
        #[cfg(any(feature = "security", feature = "raft"))]
        {
            &medulla_kernel::crypto::TEST_ENV_LOCK
        }
        #[cfg(not(any(feature = "security", feature = "raft")))]
        {
            static LOCK: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());
            &LOCK
        }
    }

    pub fn blocking(origin: &str) -> Self {
        Self::install(origin, Self::env_lock().blocking_write())
    }

    pub async fn acquire(origin: &str) -> Self {
        Self::install(origin, Self::env_lock().write().await)
    }
}

#[cfg(feature = "federation-sql")]
impl Drop for FederationAllowGuard {
    fn drop(&mut self) {
        let key = medulla_federation::federation::HTTP_JSON_FEDERATION_ALLOW_ENV;
        if let Some(previous) = self.previous.take() {
            std::env::set_var(key, previous);
        } else {
            std::env::remove_var(key);
        }
    }
}

/// Common terminal fixture used by durable and transport retry identity tests.
#[cfg(any(feature = "server", feature = "redb"))]
pub fn terminal_method(lease_epoch: u64, fencing_token: u64) -> medulla_protocol_types::protocol::Method {
    medulla_protocol_types::protocol::Method::CommitWorkItemResult {
        tenant: "tenant-a".into(),
        work_item_id: "work-1".into(),
        worker_id: "worker-a".into(),
        lease_epoch,
        fencing_token,
        idempotency_key: "terminal-key".into(),
        outcome: "succeeded".into(),
        result_ref: Some("result:sha256:one".into()),
        outcome_extension: None,
        run_completion: None,
        error_ref: None,
        retryable: false,
        now_ms: 1_000,
    }
}
