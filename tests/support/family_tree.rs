//! The synthetic family tree of the native-hosting pilot
//! (MEDULLA-UNIFIED-DATA-PLANE-R026.2.2): in-repo, deterministic, built in memory.
//!
//! Nothing here is a committed corpus and no other repository is read. Every name and year is
//! drawn from `SplitMix64::new(26)` in a fixed order, so two calls give the same tree.

#![allow(dead_code)]

use medulla_compute::SplitMix64;

/// The gate stops with this code when a phase's exit artifact is invalid.
pub const PHASE_EXIT_INVALID: &str = "PHASE_EXIT_INVALID";

/// The RDF namespace of the imported tree (prefix `gramps:`).
pub const NAMESPACE: &str = "https://example.invalid/gramps#";
const SUBJECT_BASE: &str = "https://example.invalid/gramps";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

const FIRST_NAMES: [&str; 8] = ["Ada", "Ben", "Cora", "Dev", "Elin", "Finn", "Gia", "Hugo"];
const SURNAMES: [&str; 6] = ["Archer", "Brandt", "Castro", "Dunne", "Ellis", "Frost"];

pub const PEOPLE: usize = 120;
pub const FAMILIES: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub handle: String,
    pub gid: String,
    pub first_name: &'static str,
    pub surname: &'static str,
    /// 1 male, 0 female.
    pub gender: u8,
    pub birth_year: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Family {
    pub handle: String,
    pub gid: String,
    pub father: String,
    pub mother: String,
    /// The handles of the children of this family.
    pub children: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyTree {
    pub people: Vec<Person>,
    pub families: Vec<Family>,
}

/// The tree: 120 people, 60 families and 59 child links.
pub fn family_tree() -> FamilyTree {
    let mut rng = SplitMix64::new(26);
    let people: Vec<Person> = (0..PEOPLE)
        .map(|index| {
            let first_name = FIRST_NAMES[rng.below(FIRST_NAMES.len())];
            let surname = SURNAMES[rng.below(SURNAMES.len())];
            let birth_year = 1800 + rng.below(150) as u32;
            Person {
                handle: format!("person-{index:03}"),
                gid: format!("I{index:04}"),
                first_name,
                surname,
                gender: if index % 2 == 0 { 1 } else { 0 },
                birth_year,
            }
        })
        .collect();
    let mut families: Vec<Family> = (0..FAMILIES)
        .map(|index| Family {
            handle: format!("family-{index:03}"),
            gid: format!("F{index:04}"),
            father: format!("person-{:03}", 2 * index),
            mother: format!("person-{:03}", 2 * index + 1),
            children: Vec::new(),
        })
        .collect();
    for index in 1..FAMILIES {
        families[(index - 1) / 2]
            .children
            .push(format!("person-{:03}", 2 * index));
    }
    FamilyTree { people, families }
}

fn person_iri(handle: &str) -> String {
    format!("{SUBJECT_BASE}/person/{handle}")
}

fn family_iri(handle: &str) -> String {
    format!("{SUBJECT_BASE}/family/{handle}")
}

impl FamilyTree {
    /// Every triple of the tree as one N-Triples line: 6 per person, 4 per family and one per
    /// child link (1,019 in all).
    pub fn triples(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for person in &self.people {
            let subject = person_iri(&person.handle);
            let iri =
                |predicate: &str, object: &str| format!("<{subject}> <{predicate}> <{object}> .");
            let text =
                |name: &str, value: &str| format!("<{subject}> <{NAMESPACE}{name}> \"{value}\" .");
            let number = |name: &str, value: u32| {
                format!("<{subject}> <{NAMESPACE}{name}> \"{value}\"^^<{XSD_INTEGER}> .")
            };
            lines.push(iri(RDF_TYPE, &format!("{NAMESPACE}Person")));
            lines.push(text("gid", &person.gid));
            lines.push(text("firstName", person.first_name));
            lines.push(text("surname", person.surname));
            lines.push(number("gender", u32::from(person.gender)));
            lines.push(number("birthYear", person.birth_year));
        }
        for family in &self.families {
            let subject = family_iri(&family.handle);
            let iri =
                |predicate: &str, object: &str| format!("<{subject}> <{predicate}> <{object}> .");
            lines.push(iri(RDF_TYPE, &format!("{NAMESPACE}Family")));
            lines.push(format!("<{subject}> <{NAMESPACE}gid> \"{}\" .", family.gid));
            lines.push(iri(
                &format!("{NAMESPACE}father"),
                &person_iri(&family.father),
            ));
            lines.push(iri(
                &format!("{NAMESPACE}mother"),
                &person_iri(&family.mother),
            ));
            for child in &family.children {
                lines.push(iri(&format!("{NAMESPACE}child"), &person_iri(child)));
            }
        }
        lines
    }

    /// The triples as N-Triples documents of at most `batch` lines each.
    pub fn ntriples_batches(&self, batch: usize) -> Vec<String> {
        self.triples()
            .chunks(batch)
            .map(|lines| {
                let mut document = lines.join("\n");
                document.push('\n');
                document
            })
            .collect()
    }
}

/// The 14 statements of the P1 and P2 corpus, in order.
pub fn family_tree_corpus(tree: &FamilyTree) -> Vec<String> {
    let people = tree
        .people
        .iter()
        .map(|p| {
            format!(
                "('{}', '{}', '{}', '{}', {}, {})",
                p.handle, p.gid, p.first_name, p.surname, p.gender, p.birth_year
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let families = tree
        .families
        .iter()
        .map(|f| {
            format!(
                "('{}', '{}', '{}', '{}')",
                f.handle, f.gid, f.father, f.mother
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let child_refs = tree
        .families
        .iter()
        .flat_map(|f| {
            f.children
                .iter()
                .map(move |child| format!("('{}', '{}')", f.handle, child))
        })
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        "CREATE TABLE person (handle TEXT PRIMARY KEY, gid TEXT NOT NULL, first_name TEXT NOT NULL, surname TEXT NOT NULL, gender INTEGER NOT NULL, birth_year INTEGER NOT NULL)".to_string(),
        "CREATE TABLE family (handle TEXT PRIMARY KEY, gid TEXT NOT NULL, father TEXT NOT NULL REFERENCES person(handle), mother TEXT NOT NULL REFERENCES person(handle))".to_string(),
        "CREATE TABLE child_ref (family TEXT NOT NULL REFERENCES family(handle), child TEXT NOT NULL REFERENCES person(handle), PRIMARY KEY (family, child))".to_string(),
        format!("INSERT INTO person VALUES {people}"),
        format!("INSERT INTO family VALUES {families}"),
        format!("INSERT INTO child_ref VALUES {child_refs}"),
        "SELECT count(*) FROM person".to_string(),
        "SELECT surname, count(*) FROM person GROUP BY surname ORDER BY surname".to_string(),
        "SELECT f.gid, p.first_name FROM family f JOIN child_ref c ON c.family = f.handle JOIN person p ON p.handle = c.child ORDER BY f.gid, p.gid LIMIT 20".to_string(),
        "SELECT p.gid FROM person p WHERE p.birth_year = (SELECT min(birth_year) FROM person) ORDER BY p.gid".to_string(),
        "SELECT gender, avg(birth_year) FROM person GROUP BY gender ORDER BY gender".to_string(),
        "WITH RECURSIVE d(handle) AS (SELECT child FROM child_ref WHERE family = 'family-000' UNION SELECT c.child FROM child_ref c JOIN d ON c.family = 'family-' || lpad(((substr(d.handle, 8)::int) / 2)::text, 3, '0') ) SELECT count(*) FROM d".to_string(),
        "SELECT gid FROM person WHERE gid ~ '^I001' ORDER BY gid LIMIT 5".to_string(),
        "CREATE EXTENSION pg_trgm".to_string(),
    ]
}

/// The approved R2RML mapping of the `person` table (MEDULLA-UNIFIED-DATA-PLANE-R026.3.2): the
/// reviewed text the P5 `SparqlVirtual` question reads.
pub const PERSON_MAPPING: &str = r#"
@prefix rr: <http://www.w3.org/ns/r2rml#> .
@prefix gramps: <https://example.invalid/gramps#> .

<#PersonMap> a rr:TriplesMap ;
    rr:logicalTable [ rr:tableName "person" ] ;
    rr:subjectMap [
        rr:template "https://example.invalid/gramps/person/{handle}" ;
        rr:class gramps:Person
    ] ;
    rr:predicateObjectMap [ rr:predicate gramps:gid ; rr:objectMap [ rr:column "gid" ] ] ;
    rr:predicateObjectMap [ rr:predicate gramps:firstName ; rr:objectMap [ rr:column "first_name" ] ] ;
    rr:predicateObjectMap [ rr:predicate gramps:surname ; rr:objectMap [ rr:column "surname" ] ] ;
    rr:predicateObjectMap [ rr:predicate gramps:gender ; rr:objectMap [ rr:column "gender" ] ] ;
    rr:predicateObjectMap [ rr:predicate gramps:birthYear ; rr:objectMap [ rr:column "birth_year" ] ] .
"#;

/// The table digest of an engine state (MEDULLA-UNIFIED-DATA-PLANE-R026.3.2): the lowercase hex
/// SHA-256 of the tab-separated text lines of `SELECT * FROM person ORDER BY handle`,
/// `SELECT * FROM family ORDER BY handle` and `SELECT * FROM child_ref ORDER BY family, child`,
/// each line ending in a newline and each result followed by one empty line. The record count
/// is the number of rows over the three results (239 for the tree).
pub fn table_digest(
    person: &[String],
    family: &[String],
    child_ref: &[String],
) -> medulla_catalog_types::gramps_baseline_digest::GrampsBaselineDigest {
    use sha2::{Digest, Sha256};
    let mut text = String::new();
    for result in [person, family, child_ref] {
        for line in result {
            text.push_str(line);
            text.push('\n');
        }
        text.push('\n');
    }
    medulla_catalog_types::gramps_baseline_digest::GrampsBaselineDigest {
        record_count: (person.len() + family.len() + child_ref.len()) as u64,
        digest_hex: hex::encode(Sha256::digest(text.as_bytes())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R026.2.2
    #[test]
    fn tree_has_the_stated_counts_and_is_deterministic() {
        let tree = family_tree();
        assert_eq!(tree.people.len(), 120);
        assert_eq!(tree.families.len(), 60);
        assert_eq!(
            tree.families
                .iter()
                .map(|f| f.children.len())
                .sum::<usize>(),
            59
        );
        assert_eq!(tree.triples().len(), 1_019);
        assert_eq!(tree, family_tree());
        assert_eq!(tree.triples(), family_tree().triples());
        assert_eq!(family_tree_corpus(&tree).len(), 14);
        // Person 2i is male, 2i+1 female; person 2i is a child of family (i-1)/2 for i >= 1.
        assert_eq!(tree.people[2].gender, 1);
        assert_eq!(tree.people[3].gender, 0);
        assert_eq!(tree.families[0].children, vec!["person-002", "person-004"]);
        // Batches of 500 lines cover every triple once.
        let batches = tree.ntriples_batches(500);
        assert_eq!(batches.len(), 3);
        assert_eq!(
            batches.iter().map(|b| b.lines().count()).sum::<usize>(),
            1_019
        );
    }
}
