//! medulla-nlq: Natural-language questions to UQL, with regressions.
//!
//! Layer L5 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (natural language).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// DecideText (EH-073): the decision front end, kept out of UQL and `wire::Op`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decide_text;
#[cfg(feature = "nl-query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod nl;

/// Question tokenisation shared by the NL planner and the lexical rerank scorer;
/// available without the `nl-query` feature.
pub mod terms {
    /// The lower-cased alphanumeric terms of `text` in order, repeats kept, words
    /// of one or two characters dropped.
    pub fn question_terms(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| t.len() > 2)
            .map(str::to_ascii_lowercase)
            .collect()
    }
}
