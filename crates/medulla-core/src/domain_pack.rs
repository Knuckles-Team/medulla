//! Domain pack evaluation (MEDULLA-TYPED-PACKS-R097.700).

pub mod mapping;

pub use mapping::{evaluate_mapping, fragment_markdown, run_cases, MappingError};
