//! The read and claim audit sink (MEDULLA-IDENTITY-R610.5).
//!
//! A bounded queue feeds one drain task that appends the queued audit writes
//! to the graph audit chain. A full queue refuses the read, so no read goes
//! unaudited.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use medulla_types::contract::ServerErrorCode;
use tokio::sync::mpsc;

/// One audit write: the future appends the line to the graph audit chain.
pub type ReadAuditWrite = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// The bounded read-audit queue and its drain task.
pub struct ReadAuditSink {
    queue: mpsc::Sender<ReadAuditWrite>,
}

impl ReadAuditSink {
    /// A sink with `depth` queue slots; spawns its drain task on the current runtime.
    pub fn spawn(depth: usize) -> Self {
        let (queue, mut inbox) = mpsc::channel::<ReadAuditWrite>(depth.max(1));
        tokio::spawn(async move {
            while let Some(write) = inbox.recv().await {
                if let Err(error) = write.await {
                    tracing::error!(%error, "read audit append failed");
                }
            }
        });
        Self { queue }
    }

    /// The process sink, sized by `identity.read_audit_queue_depth`. Call from
    /// inside the server runtime.
    pub fn global() -> &'static Self {
        static SINK: OnceLock<ReadAuditSink> = OnceLock::new();
        SINK.get_or_init(|| {
            let depth = crate::knob_config::knobs().identity_read_audit_queue_depth;
            Self::spawn(usize::try_from(depth).unwrap_or(4_096))
        })
    }

    /// Queue one audit write. A full or closed queue refuses `AUDIT_UNAVAILABLE`.
    pub fn enqueue(&self, write: ReadAuditWrite) -> Result<(), ServerErrorCode> {
        self.queue
            .try_send(write)
            .map_err(|_| ServerErrorCode::AuditUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies MEDULLA-IDENTITY-R610.5: Evidence: read and claim audit sink
    // spec: MEDULLA-IDENTITY-R610.5
    #[tokio::test]
    async fn full_queue_refuses_with_audit_unavailable() {
        let (queue, _inbox) = mpsc::channel::<ReadAuditWrite>(1);
        let sink = ReadAuditSink { queue };
        sink.enqueue(Box::pin(async { Ok(()) })).unwrap();
        let refused = sink.enqueue(Box::pin(async { Ok(()) })).unwrap_err();
        assert_eq!(refused.as_str(), "AUDIT_UNAVAILABLE");
    }
}
