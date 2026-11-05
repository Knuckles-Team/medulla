//! Section-tree model and behaviour (MEDULLA-REPO-INGEST-R652.1, R652.2).

use medulla_document::processing::{
    build_section_tree, chunk_text, section_for, verify_section_tree, ChunkingConfig,
    ProcessedDocument, SectionNode, SectionSource, SectionTreeCheck,
};

// spec: MEDULLA-REPO-INGEST-R652.1
#[test]
fn types_round_trip() {
    let node = SectionNode {
        section_id: 1,
        parent_id: Some(0),
        title: "Part A".into(),
        span_start: 10,
        span_end: 42,
        depth: 2,
    };
    let encoded = serde_json::to_string(&node).unwrap();
    assert_eq!(serde_json::from_str::<SectionNode>(&encoded).unwrap(), node);
    for check in [
        SectionTreeCheck::Valid,
        SectionTreeCheck::Overlap,
        SectionTreeCheck::Gap,
        SectionTreeCheck::OutOfBounds,
    ] {
        let encoded = serde_json::to_string(&check).unwrap();
        assert_eq!(
            serde_json::from_str::<SectionTreeCheck>(&encoded).unwrap(),
            check
        );
    }
    assert_eq!(
        serde_json::to_string(&SectionTreeCheck::OutOfBounds).unwrap(),
        "\"out_of_bounds\""
    );
    let document = ProcessedDocument {
        document_id: "doc-1".into(),
        content_digest: format!("sha256:{}", "a".repeat(64)),
        chunk_count: 3,
        section_count: 2,
        coverage_milli: 1000,
        tree_check: SectionTreeCheck::Valid,
    };
    let encoded = serde_json::to_string(&document).unwrap();
    assert_eq!(
        serde_json::from_str::<ProcessedDocument>(&encoded).unwrap(),
        document
    );
    assert!(serde_json::from_str::<SectionNode>(
        r#"{"section_id":0,"parent_id":null,"title":"","span_start":0,"span_end":0,"depth":1,"extra":1}"#
    )
    .is_err());
}

/// A small deterministic generator (xorshift) so the property runs without a
/// new dependency and replays exactly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn generated_markdown(rng: &mut Rng) -> String {
    let mut text = String::new();
    if rng.below(2) == 0 {
        text.push_str("Preamble before any heading.\n\n");
    }
    for _ in 0..rng.below(25) {
        match rng.below(4) {
            0 => {
                let level = 1 + rng.below(6) as usize;
                text.push_str(&"#".repeat(level));
                text.push_str(&format!(" Heading {}\n", rng.below(1000)));
            }
            1 => text.push('\n'),
            2 => text.push_str("Ünïcode paragraph text, with a sentence. And more.\n"),
            _ => text.push_str("#not-a-heading and a [link](https://example.invalid)\n"),
        }
    }
    text
}

fn assert_tree_rule(text: &str, nodes: &[SectionNode]) {
    assert_eq!(
        verify_section_tree(nodes, text.len()),
        SectionTreeCheck::Valid,
        "{text:?} -> {nodes:?}"
    );
    for node in nodes {
        if let Some(parent) = node.parent_id {
            let p = &nodes[parent as usize];
            assert!(p.span_start <= node.span_start && node.span_end <= p.span_end);
            assert_eq!(node.depth, p.depth + 1);
        }
    }
}

// spec: MEDULLA-REPO-INGEST-R652.2
#[test]
fn tree_nests_and_covers_text_property() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..500 {
        let text = generated_markdown(&mut rng);
        let nodes = build_section_tree(&text, SectionSource::Markdown);
        assert_eq!(nodes.is_empty(), text.is_empty());
        assert_tree_rule(&text, &nodes);
        // Each chunk belongs to its deepest section: a leaf containing its start.
        for chunk in chunk_text(&text, &ChunkingConfig::default()).unwrap() {
            let id = section_for(&nodes, chunk.start_byte).expect("chunk has a section");
            assert!(
                !nodes.iter().any(|n| n.parent_id == Some(id)),
                "deepest section is a leaf"
            );
        }
    }

    let paged = "page one\x0cpage two\x0cpage three";
    let pages = build_section_tree(paged, SectionSource::Paged);
    assert_eq!(pages.len(), 3);
    assert_tree_rule(paged, &pages);

    // The default window stays 1,200 bytes with 150 bytes of overlap.
    assert_eq!(
        ChunkingConfig::default(),
        ChunkingConfig {
            max_bytes: 1_200,
            overlap_bytes: 150
        }
    );

    // Broken trees are named.
    let text = "# A\nbody\n## B\nmore\n";
    let mut nodes = build_section_tree(text, SectionSource::Markdown);
    assert_tree_rule(text, &nodes);
    let mut out_of_bounds = nodes.clone();
    out_of_bounds.last_mut().unwrap().span_end = text.len() + 1;
    assert_eq!(
        verify_section_tree(&out_of_bounds, text.len()),
        SectionTreeCheck::OutOfBounds
    );
    let mut gap = nodes.clone();
    gap.last_mut().unwrap().span_start += 1;
    assert_eq!(verify_section_tree(&gap, text.len()), SectionTreeCheck::Gap);
    nodes.last_mut().unwrap().span_start -= 1;
    assert_eq!(
        verify_section_tree(&nodes, text.len()),
        SectionTreeCheck::Overlap
    );
}
