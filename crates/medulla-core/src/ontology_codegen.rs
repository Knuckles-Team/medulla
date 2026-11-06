//! Deterministic ontology/SHACL-shape text generated from a typed Rust vocabulary
//! (MEDULLA-TYPED-PACKS-R090.1).
//!
//! The canonical ontology and SHACL shape corpus must be owned by a typed Rust
//! vocabulary, not hand-edited Turtle: regenerating the text from the same typed
//! declaration must reproduce byte-identical output across runs. This module is
//! the smallest producer-first slice of that mechanism — one tiny self-contained
//! vocabulary fragment and its generator — so the regeneration guarantee has one
//! real, tested implementation to extend onto the existing `crates/medulla-core/ontology/*.ttl`
//! corpus in later child requirements (R090.2+).

/// One `owl:Class` declaration in a typed vocabulary module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwlClass {
    pub local_name: &'static str,
    pub label: &'static str,
    pub comment: &'static str,
    pub subclass_of: Option<&'static str>,
}

/// One `owl:DatatypeProperty` declaration in a typed vocabulary module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatatypeProperty {
    pub local_name: &'static str,
    pub label: &'static str,
    pub range: &'static str,
}

/// The relation of a declared crosswalk. `SeeAlso` is `rdfs:seeAlso`; the rest
/// are the SKOS mapping properties. None implies `owl:sameAs`, equivalence or a
/// subclass axiom (MEDULLA-TYPED-PACKS-R071.1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrosswalkRelation {
    SeeAlso,
    ExactMatch,
    CloseMatch,
    BroadMatch,
    NarrowMatch,
}

impl CrosswalkRelation {
    fn term(self) -> &'static str {
        match self {
            Self::SeeAlso => "rdfs:seeAlso",
            Self::ExactMatch => "skos:exactMatch",
            Self::CloseMatch => "skos:closeMatch",
            Self::BroadMatch => "skos:broadMatch",
            Self::NarrowMatch => "skos:narrowMatch",
        }
    }
}

/// One declared crosswalk from a class of the module to an absolute target IRI.
/// Plain strings only: `medulla-core` does not depend on the pack record types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrosswalkDeclaration {
    pub source_local_name: &'static str,
    pub relation: CrosswalkRelation,
    pub target_iri: &'static str,
}

/// A typed vocabulary module: the single source of truth for one generated
/// ontology fragment. `classes` and `properties` are declared in a fixed order,
/// so turtle generation never depends on iteration order of a hash-based
/// collection — that is what makes regeneration byte-identical.
#[derive(Debug, Clone, Copy)]
pub struct VocabularyModule {
    pub module_id: &'static str,
    pub ontology_label: &'static str,
    pub ontology_comment: &'static str,
    pub classes: &'static [OwlClass],
    pub properties: &'static [DatatypeProperty],
    /// Declared crosswalks, emitted as mapping triples in declaration order.
    pub crosswalks: &'static [CrosswalkDeclaration],
}

impl VocabularyModule {
    /// Render this module's canonical Turtle text. Pure and deterministic: equal
    /// input always produces the equal, byte-identical output.
    pub fn to_turtle(&self) -> String {
        let mut out = String::new();
        out.push_str("@prefix : <http://knuckles.team/kg#> .\n");
        out.push_str("@prefix owl: <http://www.w3.org/2002/07/owl#> .\n");
        out.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n");
        out.push_str("@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n");

        for class in self.classes {
            out.push_str(&format!(":{} a owl:Class ;\n", class.local_name));
            out.push_str(&format!("    rdfs:label \"{}\" ;\n", class.label));
            match class.subclass_of {
                Some(parent) => {
                    out.push_str(&format!("    rdfs:comment \"{}\" ;\n", class.comment));
                    out.push_str(&format!("    rdfs:subClassOf :{} .\n\n", parent));
                }
                None => {
                    out.push_str(&format!("    rdfs:comment \"{}\" .\n\n", class.comment));
                }
            }
        }

        for prop in self.properties {
            out.push_str(&format!(":{} a owl:DatatypeProperty ;\n", prop.local_name));
            out.push_str(&format!("    rdfs:label \"{}\" ;\n", prop.label));
            out.push_str(&format!("    rdfs:range {} .\n\n", prop.range));
        }

        if !self.crosswalks.is_empty() {
            out.push_str("@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\n");
            for crosswalk in self.crosswalks {
                out.push_str(&format!(
                    ":{} {} <{}> .\n",
                    crosswalk.source_local_name,
                    crosswalk.relation.term(),
                    crosswalk.target_iri
                ));
            }
            out.push('\n');
        }

        out.push_str(&format!(
            "<http://knuckles.team/kg/{}> a owl:Ontology ;\n",
            self.module_id
        ));
        out.push_str(&format!("    rdfs:label \"{}\" ;\n", self.ontology_label));
        out.push_str(&format!(
            "    rdfs:comment \"{}\" .\n",
            self.ontology_comment
        ));
        out
    }
}

/// The IRI of the core foundation document.
pub const CORE_DOCUMENT_IRI: &str = "http://knuckles.team/kg/core";

/// Whether `turtle` declares `owl:imports` of the core document. A text scan of
/// each `owl:imports` object list (IRIs between `<` and `>`, separated by
/// commas, ended by `;` or `.` outside an IRI), so it needs no RDF parser.
pub fn imports_core_document(turtle: &str) -> bool {
    let mut rest = turtle;
    while let Some(at) = rest.find("owl:imports") {
        rest = &rest[at + "owl:imports".len()..];
        let mut inside = false;
        let mut iri = String::new();
        for character in rest.chars() {
            match (inside, character) {
                (false, '<') => inside = true,
                (true, '>') => {
                    if iri == CORE_DOCUMENT_IRI {
                        return true;
                    }
                    iri.clear();
                    inside = false;
                }
                (true, other) => iri.push(other),
                (false, ';' | '.') => break,
                (false, _) => {}
            }
        }
    }
    false
}

/// The build check of MEDULLA-TYPED-PACKS-R071.2: refuse any module that
/// declares `owl:imports` of the core IRI. `modules` are `(module id, Turtle)`
/// pairs; the refusal names the first offending module and carries the
/// registered `ONTOLOGY_PACK_IMPORTS_CORE` code.
pub fn refuse_modules_importing_core<'a>(
    modules: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), String> {
    for (module, turtle) in modules {
        if imports_core_document(turtle) {
            return Err(format!(
                "{}: module '{module}' declares owl:imports of {CORE_DOCUMENT_IRI}; \
                 a module extends core classes through rdfs:subClassOf",
                medulla_types::contract::ServerErrorCode::OntologyPackImportsCore.as_str()
            ));
        }
    }
    Ok(())
}

/// The codegen-mechanism self-test vocabulary: the first typed module generated by
/// this mechanism. Deliberately tiny — it exists to prove the generator, not to
/// model a real domain; `crates/medulla-core/ontology/*.ttl` migrate onto this same
/// mechanism in later child requirements.
pub const CODEGEN_SELFTEST_V1: VocabularyModule = VocabularyModule {
    module_id: "codegen_selftest",
    ontology_label: "codegen_selftest Ontology (generated)",
    ontology_comment: "Smallest self-test fragment proving typed-Rust-vocabulary-to-Turtle codegen is deterministic. CONCEPT:MEDULLA-KG.ontology.codegen-from-rust-vocabulary.",
    classes: &[OwlClass {
        local_name: "CodegenSelfTestArtifact",
        label: "Codegen self-test artifact",
        comment: "A generated-ontology regression fixture (generated).",
        subclass_of: None,
    }],
    properties: &[DatatypeProperty {
        local_name: "generated_at",
        label: "Generated_at",
        range: "xsd:dateTime",
    }],
    crosswalks: &[],
};

/// The checked-in expected output of `CODEGEN_SELFTEST_V1.to_turtle()`. Kept as a
/// file fixture (not only an inline string) so a reviewer can diff generated vs.
/// checked-in Turtle the same way any other `crates/medulla-core/ontology/*.ttl` is reviewed.
pub const CODEGEN_SELFTEST_V1_CHECKED_IN: &str =
    include_str!("../ontology_codegen_fixtures/codegen_selftest-v1.ttl");

#[cfg(test)]
mod tests {
    use super::*;

    /// MEDULLA-TYPED-PACKS-R090.1: regenerating the Turtle text from the typed vocabulary
    /// twice produces byte-identical output, and that output is byte-identical to
    /// the checked-in fixture.
    // spec: MEDULLA-TYPED-PACKS-R090.1
    #[test]
    fn regeneration_is_byte_identical_across_runs_and_to_checked_in_output() {
        let first = CODEGEN_SELFTEST_V1.to_turtle();
        let second = CODEGEN_SELFTEST_V1.to_turtle();
        assert_eq!(first, second, "regeneration must be deterministic");
        assert_eq!(
            first, CODEGEN_SELFTEST_V1_CHECKED_IN,
            "generated Turtle must be byte-identical to the checked-in artifact"
        );
    }

    /// MEDULLA-TYPED-PACKS-R071.1.3: declared crosswalks render as mapping
    /// triples, and a module without any renders no mapping prefix.
    // spec: MEDULLA-TYPED-PACKS-R071.1.3
    #[test]
    fn declared_crosswalks_render_as_mapping_triples() {
        let mut module = CODEGEN_SELFTEST_V1;
        module.crosswalks = &[CrosswalkDeclaration {
            source_local_name: "CodegenSelfTestArtifact",
            relation: CrosswalkRelation::CloseMatch,
            target_iri: "http://schema.org/CreativeWork",
        }];
        let turtle = module.to_turtle();
        assert!(turtle.contains(
            ":CodegenSelfTestArtifact skos:closeMatch <http://schema.org/CreativeWork> ."
        ));
        assert!(!CODEGEN_SELFTEST_V1.to_turtle().contains("skos:"));
    }

    // spec: MEDULLA-TYPED-PACKS-R071.2
    #[test]
    fn a_module_importing_core_is_refused_with_the_registered_code() {
        let importing = "<http://knuckles.team/kg/x> a owl:Ontology ;\n    owl:imports <http://knuckles.team/kg/a>, <http://knuckles.team/kg/core> .\n";
        let clean = "<http://knuckles.team/kg/x> a owl:Ontology ;\n    owl:imports <http://knuckles.team/kg/a> .\n";
        assert!(imports_core_document(importing));
        assert!(!imports_core_document(clean));
        let refusal = refuse_modules_importing_core([("x", importing)]).unwrap_err();
        assert!(refusal.starts_with("ONTOLOGY_PACK_IMPORTS_CORE: module 'x'"));
        assert!(refuse_modules_importing_core([("x", clean)]).is_ok());
    }

    /// A changed vocabulary (different label) must NOT match the checked-in fixture —
    /// confirms the byte-equality check is not vacuously true.
    // spec: MEDULLA-TYPED-PACKS-R090.1
    #[test]
    fn a_different_module_does_not_match_the_checked_in_fixture() {
        let mut mutated = CODEGEN_SELFTEST_V1;
        mutated.ontology_label = "a different label";
        assert_ne!(mutated.to_turtle(), CODEGEN_SELFTEST_V1_CHECKED_IN);
    }
}
