//! The one ontology base.
//!
//! Every custom ontology IRI is `ONTOLOGY_BASE + <module> + "#"` (documents carry no
//! fragment separator). The base is spelled exactly once, in [`ontology_base!`]; every
//! other base-derived IRI in the workspace is built from these macros or constants.

/// The single organization ontology base: HTTPS, versionless, ending in `/`.
#[macro_export]
macro_rules! ontology_base {
    () => {
        "https://knuckles-team.github.io/ontology/"
    };
}

/// A base-derived IRI literal: `ontology_iri!("core#Person")` or, for an ontology
/// document, `ontology_iri!("core")`. Usable in `const` items and `format!`.
#[macro_export]
macro_rules! ontology_iri {
    ($suffix:literal) => {
        concat!($crate::ontology_base!(), $suffix)
    };
}

/// The ontology base as a constant.
pub const ONTOLOGY_BASE: &str = ontology_base!();
/// Namespace of the shared core vocabulary.
pub const CORE_NAMESPACE: &str = ontology_iri!("core#");
/// Document IRI of the shared core vocabulary.
pub const CORE_DOCUMENT: &str = ontology_iri!("core");

#[cfg(test)]
mod tests {
    use super::*;

    // spec: none (ontology namespaces move to the single organization base)
    #[test]
    fn derived_iris_sit_under_the_one_base() {
        assert!(ONTOLOGY_BASE.starts_with("https://") && ONTOLOGY_BASE.ends_with('/'));
        assert_eq!(CORE_NAMESPACE, format!("{ONTOLOGY_BASE}core#"));
        assert_eq!(CORE_DOCUMENT, format!("{ONTOLOGY_BASE}core"));
        assert_eq!(ontology_iri!("core#Person"), format!("{ONTOLOGY_BASE}core#Person"));
    }
}
