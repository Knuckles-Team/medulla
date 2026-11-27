//! The `:formalizes` edge of the core ontology (MEDULLA-DECISION-ENGINE-R652.12):
//! declared once, in core, with the entity domain and range, so a connector pack
//! uses it without redeclaring it.

use std::path::Path;

const DECLARATION: &str = ":formalizes a owl:ObjectProperty ;";
const CORE: &str = "core-foundation-v1.ttl";

fn ontology_files() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ontology");
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("the ontology directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ttl"))
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).expect("an ontology file is readable");
            (name, text)
        })
        .collect();
    files.sort();
    files
}

// spec: MEDULLA-DECISION-ENGINE-R652.12
#[test]
fn formalizes_is_declared_once_in_core_with_entity_domain_and_range() {
    let files = ontology_files();
    let declaring: Vec<&str> = files
        .iter()
        .filter(|(_, text)| text.contains(DECLARATION))
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(declaring, [CORE], "only core declares :formalizes");

    let core = &files.iter().find(|(name, _)| name == CORE).unwrap().1;
    assert_eq!(
        core.matches(DECLARATION).count(),
        1,
        "declared once in core"
    );
    let start = core.find(DECLARATION).unwrap();
    let block = &core[start
        ..core[start..]
            .find(" .\n")
            .map_or(core.len(), |end| start + end)];
    assert!(block.contains("rdfs:label \"formalizes\""), "{block}");
    assert!(block.contains("rdfs:comment"), "{block}");
    // The entity class is bfo:0000001 for both ends, like :derivedFrom.
    assert!(block.contains("rdfs:domain bfo:0000001"), "{block}");
    assert!(block.contains("rdfs:range bfo:0000001"), "{block}");
    // The edge adds no class: no `:formalizes` subclass declaration exists.
    assert!(!core.contains(":formalizes a owl:Class"));
}
