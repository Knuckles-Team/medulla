//! Durable mutation scope vocabulary: tenant/graph identity, durability domains,
//! committed versions and the outbox rows a commit publishes. Method-free, so
//! domain type crates can name a scope or an outbox row without the protocol hub.

mod domain;
mod identity;
mod outbox;

pub use domain::*;
pub use identity::*;
pub use outbox::*;
