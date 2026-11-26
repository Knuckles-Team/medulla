//! The operator and consumer surface over any owner's mutation outbox (X10).
//!
//! The generic functions live in `medulla-transaction` (`outbox::views`), so every
//! owner crate -- not only the server -- serves the same views; this module is
//! the server's name for them.
