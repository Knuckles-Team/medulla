//! Spec folder node kinds (MEDULLA-ORCH-VOCAB-R012.1).

use medulla_core::spec_folder::SpecNodeKind;

const ONTOLOGY: &str = include_str!("../ontology/sdd-v1.ttl");
const SHAPES: &str = include_str!("../ontology/sdd-v1.shapes.ttl");

// spec: MEDULLA-ORCH-VOCAB-R012.1
#[test]
fn node_kinds_are_closed_and_declared() {
    let names: Vec<&str> = SpecNodeKind::ALL.iter().map(|k| k.wire_name()).collect();
    assert_eq!(names, ["task", "goal", "document", "memory", "policy"]);
    for kind in SpecNodeKind::ALL {
        assert_eq!(SpecNodeKind::from_wire(kind.wire_name()), Some(kind));
    }
    assert_eq!(SpecNodeKind::from_wire("widget"), None);

    for (class, parent) in [
        ("SpecTask", "Action"),
        ("SpecGoal", "Goal"),
        ("SpecMemory", "Memory"),
        ("SpecPolicy", "Policy"),
    ] {
        let declaration = format!(":{class} a owl:Class ;");
        let at = ONTOLOGY
            .find(&declaration)
            .unwrap_or_else(|| panic!("{class} is declared"));
        let block = &ONTOLOGY[at..];
        let block = &block[..block.find(" .\n").unwrap()];
        assert!(block.contains(&format!("rdfs:subClassOf :{parent}")), "{class}");
        assert_eq!(block.matches("rdfs:subClassOf").count(), 1, "{class}");
        assert!(SHAPES.contains(&format!("sh:targetClass :{class} ;")), "{class}");
    }
    assert!(ONTOLOGY.contains(":classifiedByRule a owl:DatatypeProperty"));
    assert_eq!(SHAPES.matches("sh:path :classifiedByRule").count(), 4);
    assert_eq!(SHAPES.matches("sh:maxCount 1").count(), 4);
    assert_eq!(SpecNodeKind::Document.class_name(), "Specification");
}

// proves R012.2: Behaviour: classify a spec file by named rule
// spec: MEDULLA-ORCH-VOCAB-R012.2
#[test]
fn default_rules_classify_each_file_kind() {
    use medulla_core::spec_folder::{
        classify_spec_file, classify_spec_file_by_rule, parse_front_matter, SpecFolderRefusal,
        DEFAULT_SPEC_FILE_RULES,
    };
    use std::collections::BTreeMap;

    let none = BTreeMap::new();
    let names: Vec<&str> = DEFAULT_SPEC_FILE_RULES.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "front-matter-kind",
            "constitution-file",
            "tasks-file",
            "goal-front-matter",
            "memory-folder",
            "spec-document"
        ]
    );

    let with = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    };
    assert_eq!(
        classify_spec_file_by_rule("notes/a.md", &with(&[("kind", "policy")])),
        Ok((SpecNodeKind::Policy, "front-matter-kind"))
    );
    // The front-matter kind beats the file name.
    assert_eq!(
        classify_spec_file("specs/x/tasks.md", &with(&[("kind", "goal")])),
        Ok(SpecNodeKind::Goal)
    );
    // An unknown kind value falls through to the later rules.
    assert_eq!(
        classify_spec_file("specs/x/tasks.md", &with(&[("kind", "widget")])),
        Ok(SpecNodeKind::Task)
    );
    assert_eq!(
        classify_spec_file_by_rule(".specify/memory/constitution.md", &none),
        Ok((SpecNodeKind::Policy, "constitution-file"))
    );
    assert_eq!(
        classify_spec_file_by_rule("specs/x/tasks.md", &none),
        Ok((SpecNodeKind::Task, "tasks-file"))
    );
    assert_eq!(
        classify_spec_file_by_rule("notes/a.md", &with(&[("goal", "ship it")])),
        Ok((SpecNodeKind::Goal, "goal-front-matter"))
    );
    assert_eq!(
        classify_spec_file("notes/b.md", &with(&[("goal", "")])),
        Err(SpecFolderRefusal::UnknownKind)
    );
    assert_eq!(
        classify_spec_file_by_rule(".specify/memory/lesson.md", &none),
        Ok((SpecNodeKind::Memory, "memory-folder"))
    );
    // The folder segment, not the file name, makes memory.
    assert_eq!(
        classify_spec_file("docs/memory", &none),
        Err(SpecFolderRefusal::UnknownKind)
    );
    for name in [
        "spec.md",
        "plan.md",
        "research.md",
        "quickstart.md",
        "data-model.md",
        "requirements.md",
    ] {
        assert_eq!(
            classify_spec_file_by_rule(&format!("specs/x/{name}"), &none),
            Ok((SpecNodeKind::Document, "spec-document"))
        );
    }

    // Front matter: flat key/value block; a malformed block is an empty map.
    let parsed = parse_front_matter("---\nkind: memory\ngoal: g\n---\nbody\n");
    assert_eq!(parsed, with(&[("kind", "memory"), ("goal", "g")]));
    assert!(parse_front_matter("---\nnot a pair\n---\n").is_empty());
    assert!(parse_front_matter("---\nkind: memory\n").is_empty());
    assert!(parse_front_matter("kind: memory\n").is_empty());
    let many: String = (0..65).map(|i| format!("k{i}: v\n")).collect();
    assert!(parse_front_matter(&format!("---\n{many}---\n")).is_empty());
}
