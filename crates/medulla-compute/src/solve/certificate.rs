//! Re-export of the certificate wire types this module's search and verifier
//! use. The definitions (and their config/certificate methods) live in
//! `medulla_types::solve::certificate` -- see that module's own doc for why there
//! is exactly one copy.

pub use medulla_decision_types::{
    solve::certificate::Algorithm, solve::certificate::BoundProof, solve::certificate::Certificate,
    solve::certificate::ConfigError, solve::certificate::DualEntry, solve::certificate::Incumbent,
    solve::certificate::LagrangeDual, solve::certificate::LeafProof, solve::certificate::ProofNode,
    solve::certificate::SolveStatus, solve::certificate::SolverConfig,
    solve::certificate::SolverConfigSpec, solve::certificate::DEFAULT_CERTIFICATE_LEAVES,
    solve::certificate::DEFAULT_NODE_BUDGET, solve::certificate::MAX_BOUND_DENOMINATOR,
    solve::certificate::MAX_CERTIFICATE_LEAVES, solve::certificate::MAX_MULTIPLIER,
    solve::certificate::MAX_NODE_BUDGET,
};
