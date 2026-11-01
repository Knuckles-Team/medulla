//! Re-export of the model wire types [`super::Model::try_from`] validates and
//! lowers. The definitions live in `medulla_decision_types::solve::spec` -- see that
//! module's own doc for why there is exactly one copy.

pub use medulla_decision_types::{
    solve::spec::Coefficient, solve::spec::ConstraintBody, solve::spec::ConstraintSpec,
    solve::spec::ModelSpec, solve::spec::ObjectiveLevelSpec, solve::spec::ObjectiveTerm,
    solve::spec::Relation, solve::spec::RowId, solve::spec::Term, solve::spec::VarId,
};
