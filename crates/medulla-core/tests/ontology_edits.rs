//! Ontology edit chain rules (MEDULLA-TYPED-PACKS-R651.2).

use std::collections::BTreeSet;

use medulla_core::ontology::edits::{
    append, chain_head, edit_digest, invert, lint_names, revert, EditDraft,
};
use medulla_types::contract::Digest256;
use medulla_types::ontology_edit::{EditKind, NamingMode, NamingSeverity, OntologyEdit};

const SUBJECT: &str = "http://example.org/onto";

fn digest(seed: &str) -> Option<Digest256> {
    Some(Digest256::sha256(seed.as_bytes()))
}

fn push(chain: &mut Vec<OntologyEdit>, kind: EditKind, before: &str, after: &str) {
    let draft = EditDraft {
        kind,
        subject_iri: SUBJECT.to_string(),
        before_digest: (!before.is_empty()).then(|| digest(before)).flatten(),
        after_digest: (!after.is_empty()).then(|| digest(after)).flatten(),
        sealed_by: None,
    };
    let edit = draft.link(chain);
    append(chain, edit).expect("a linked edit appends");
}

fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| item.to_string()).collect()
}

// spec: MEDULLA-TYPED-PACKS-R651.2
#[test]
fn chain_links_invert_is_exact_and_revert_replays_newest_first() {
    let mut chain = Vec::new();
    push(&mut chain, EditKind::Add, "", "a");
    push(&mut chain, EditKind::Change, "a", "b");
    push(&mut chain, EditKind::Change, "b", "c");
    // Each edit links to the digest of the one before it.
    assert_eq!(chain[0].prev_digest, None);
    for pair in chain.windows(2) {
        assert_eq!(pair[1].prev_digest, Some(edit_digest(&pair[0])));
    }
    assert_eq!(chain_head(&chain), Some(edit_digest(&chain[2])));

    // Inversion swaps the digests and flips add and remove, exactly.
    let inverted = invert(&chain, &chain[0]);
    assert_eq!(inverted.kind, EditKind::Remove);
    assert_eq!(inverted.before_digest, chain[0].after_digest);
    assert_eq!(inverted.after_digest, chain[0].before_digest);
    assert_eq!(invert(&chain, &inverted).kind, EditKind::Add);

    // Revert appends inversions newest first and never deletes an edit.
    let appended = revert(&mut chain, SUBJECT, None).expect("revert");
    assert_eq!(chain.len(), 6);
    let afters: Vec<_> = appended.iter().map(|e| e.after_digest).collect();
    assert_eq!(afters, vec![digest("b"), digest("a"), None]);
    assert_eq!(appended[2].kind, EditKind::Remove);
    assert!(revert(&mut chain, SUBJECT, None)
        .expect("idempotent")
        .is_empty());

    // Naming lint: PascalCase classes, camelCase properties.
    let findings = lint_names(
        &names(&["http://e.org/o#Good", "http://e.org/o#bad_Name"]),
        &names(&["http://e.org/o#goodName", "http://e.org/o#BadName"]),
        NamingMode::Warn,
    );
    let flagged: Vec<_> = findings.iter().map(|f| f.subject_iri.as_str()).collect();
    assert_eq!(
        flagged,
        ["http://e.org/o#BadName", "http://e.org/o#bad_Name"]
    );
    assert!(findings.iter().all(|f| f.severity == NamingSeverity::Warn));
}

// spec: MEDULLA-TYPED-PACKS-R651.4
#[test]
fn broken_chain_sealed_revert_and_naming_refuse() {
    use medulla_core::ontology::edits::{refuse_naming_errors, verify_chain};
    use medulla_types::contract::ServerErrorCode;

    let mut chain = Vec::new();
    push(&mut chain, EditKind::Add, "", "a");
    chain[0].sealed_by = Some("lease-1".to_string());
    push(&mut chain, EditKind::Change, "a", "b");
    // A sealed edit refuses a revert and leaves the chain untouched.
    let refusal = revert(&mut chain, SUBJECT, None).expect_err("sealed");
    assert_eq!(refusal.code, ServerErrorCode::EditSealed);
    assert_eq!(chain.len(), 2);
    assert_eq!(
        revert(&mut chain, SUBJECT, Some("lease-2"))
            .expect("approved")
            .len(),
        2
    );
    // A wrong link and an altered edit both break the chain.
    let mut forged = chain[1].clone();
    forged.prev_digest = None;
    let broken = append(&mut chain, forged).expect_err("wrong link");
    assert_eq!(broken.code, ServerErrorCode::EditChainBroken);
    chain[0].subject_iri = "http://example.org/other".to_string();
    let altered = verify_chain(&chain, chain_head(&chain)).expect_err("altered");
    assert_eq!(altered.code, ServerErrorCode::EditChainBroken);
    // Under `refuse` a naming finding is an error and refuses.
    let findings = lint_names(
        &names(&["http://e.org/o#bad_name"]),
        &names(&[]),
        NamingMode::Refuse,
    );
    let refused = refuse_naming_errors(&findings).expect_err("naming");
    assert_eq!(refused.code, ServerErrorCode::NamingViolation);
}
