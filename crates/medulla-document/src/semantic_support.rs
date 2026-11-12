//! The versioned table of the languages medulla-document ships with a parser
//! AND a resolver for (MEDULLA-REPO-INGEST-R006.4). Semantic typing of a
//! repository's source is only declared when both halves are shipped: a
//! request whose mode is not supported by the table refuses with
//! `SEMANTIC_MODE_UNSUPPORTED`, and a caller that explicitly asks for
//! syntax-only output is always served the syntax-only answer instead.
//!
//! The table is versioned: resolving semantics for a generation binds the
//! table version alongside the content, so a table revision that adds or
//! retires languages is visible to everyone who reuses older artifacts.

use serde::{Deserialize, Serialize};

/// Semantic-typing mode of one request (MEDULLA-REPO-INGEST-R006.4).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SemanticMode {
    /// Full semantic typing: one `SEMANTIC_LANGUAGES` row must cover the
    /// language, else the request refuses with `SEMANTIC_MODE_UNSUPPORTED`.
    Semantic,
    /// Syntax-only output: shipped for every language, no semantics resolved.
    SyntaxOnly,
}

/// The output shape a supported mode serves back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticOutput {
    /// Full semantic typing for a table-covered language.
    Typed,
    /// Syntax-only structure without resolved semantics.
    SyntaxOnly,
}

/// One declared language of [`SEMANTIC_LANGUAGES`]: both halves shipped in
/// `medulla-document`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticLanguage {
    /// Review-stable language name.
    pub language: &'static str,
    /// The shipped medulla-document parser module for the form.
    pub parser: &'static str,
    /// The shipped medulla-document resolver module that binds semantics.
    pub resolver: &'static str,
}

/// Table version of [`SEMANTIC_LANGUAGES`]. Bump on any addition or retirement
/// and record it with generations bound to this table.
pub const SEMANTIC_LANGUAGES_VERSION: u32 = 1;

/// The declared languages with a shipped parser AND resolver in
/// `medulla-document` (MEDULLA-REPO-INGEST-R006.4). Versioned by
/// [`SEMANTIC_LANGUAGES_VERSION`]; review-stable order, never sorted ad hoc.
pub const SEMANTIC_LANGUAGES: &[SemanticLanguage] = &[
    // Bounded UTF-8, form-feed page and line-structure decoding plus governed
    // lexical postings: the base language every other table entry builds on.
    SemanticLanguage {
        language: "text",
        parser: "decoder::NativeTextDecoder",
        resolver: "processing",
    },
    // HTML is detected and reduced to the same governed structure ahead of the
    // native decode (GOC-06): parser in `html`, resolver the native decoder.
    SemanticLanguage {
        language: "html",
        parser: "html",
        resolver: "decoder::NativeTextDecoder",
    },
    SemanticLanguage {
        language: "toc",
        parser: "decoder::NativeTextDecoder",
        resolver: "toc",
    },
    SemanticLanguage {
        language: "skill_plan",
        parser: "decoder::NativeTextDecoder",
        resolver: "skill_plan",
    },
    SemanticLanguage {
        language: "transcript",
        parser: "transcript::TRANSCRIPT_PARSER_TABLE",
        resolver: "transcript",
    },
];

/// The whole table covers this language, or nothing does.
pub fn semantic_language_of(language: &str) -> Option<&'static SemanticLanguage> {
    SEMANTIC_LANGUAGES
        .iter()
        .find(|entry| entry.language == language)
}

/// Serve `mode` for `language`, or refuse: `SEMANTIC_MODE_UNSUPPORTED` exactly
/// when full semantic typing is requested for a language the versioned table
/// does not cover. `SyntaxOnly` is always servable.
pub fn semantic_output_for(mode: SemanticMode, language: &str) -> Result<SemanticOutput, String> {
    match mode {
        SemanticMode::Semantic if semantic_language_of(language).is_some() => {
            Ok(SemanticOutput::Typed)
        }
        SemanticMode::Semantic => Err(format!(
            "{SEMANTIC_MODE_UNSUPPORTED}: language {language:?} is not in SEMANTIC_LANGUAGES \
             version {SEMANTIC_LANGUAGES_VERSION}"
        )),
        SemanticMode::SyntaxOnly => Ok(SemanticOutput::SyntaxOnly),
    }
}

/// Stable refusal name of a semantic-typing request the versioned table
/// cannot serve; the alternative is an explicit syntax-only request.
pub const SEMANTIC_MODE_UNSUPPORTED: &str = "SEMANTIC_MODE_UNSUPPORTED";

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-REPO-INGEST-R006.4
    //
    // The versioned table only ever declares languages with BOTH a shipped
    // parser and a shipped resolver in medulla-document, is nonempty, stable
    // in its review order, and every declaration resolves back through the
    // lookup.
    #[test]
    fn declared_languages_carry_a_shipped_parser_and_resolver() {
        const _: () = assert!(SEMANTIC_LANGUAGES_VERSION >= 1);
        assert!(!SEMANTIC_LANGUAGES.is_empty());
        let names = SEMANTIC_LANGUAGES.iter().map(|entry| entry.language);
        names.clone().for_each(|name| {
            assert!(!name.is_empty(), "an unnamed language declares nothing");
        });
        let mut seen: Vec<String> = names.map(str::to_owned).collect();
        seen.sort();
        let count = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), count, "languages are unique in the table");
        for entry in SEMANTIC_LANGUAGES {
            assert!(
                !entry.parser.is_empty() && !entry.resolver.is_empty(),
                "language {} must name both halves",
                entry.language
            );
            assert!(
                semantic_language_of(entry.language) == Some(entry),
                "every table row must be served by the lookup"
            );
        }
        assert!(semantic_language_of("text").is_some());
        assert!(semantic_language_of("no-such-language").is_none());
    }

    // spec: MEDULLA-REPO-INGEST-R006.4
    //
    // An unsupported mode refuses with SEMANTIC_MODE_UNSUPPORTED; an explicit
    // syntax-only request is served instead, and a table-covered language
    // serves full semantic typing.
    #[test]
    fn unsupported_mode_refuses_and_syntax_only_is_served() {
        let refusal = match semantic_output_for(SemanticMode::Semantic, "c++") {
            Err(refusal) => refusal,
            Ok(_) => panic!("an off-table language must refuse semantic typing"),
        };
        assert!(
            refusal.starts_with(SEMANTIC_MODE_UNSUPPORTED),
            "the stable code leads the refusal, got: {refusal}"
        );
        assert_eq!(
            semantic_output_for(SemanticMode::SyntaxOnly, "c++").expect("syntax-only is servable"),
            SemanticOutput::SyntaxOnly
        );
        assert_eq!(
            semantic_output_for(SemanticMode::Semantic, "text").expect("table-covered serves"),
            SemanticOutput::Typed
        );
        assert_eq!(
            semantic_output_for(SemanticMode::SyntaxOnly, "text").expect("still servable"),
            SemanticOutput::SyntaxOnly
        );
    }
}
