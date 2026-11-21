//! Media memory planning and recall matching (`MEDULLA-MEDIA-VOCAB-R650.2`).

use medulla_modality::media_memory::{
    match_derivation, plan_media_write, DerivationInput, DerivationKind, MediaBlobRef,
    MediaRefusal,
};

fn blob() -> MediaBlobRef {
    MediaBlobRef {
        digest: "ab".repeat(32),
        mime: "image/png".to_string(),
        byte_len: 1024,
    }
}

fn input(kind: DerivationKind, text: &str) -> DerivationInput {
    DerivationInput {
        kind,
        text: text.to_string(),
        embedding: vec![1.0, 0.0],
    }
}

// spec: MEDULLA-MEDIA-VOCAB-R650.2
#[test]
fn blob_written_once_and_recall_is_deterministic() {
    let derivations = [
        input(DerivationKind::Transcript, "hello there"),
        input(DerivationKind::Caption, "a red door"),
        input(DerivationKind::Caption, "a red door"),
    ];
    let first = plan_media_write(&blob(), false, &derivations).unwrap();
    assert_eq!(first.blob, Some(blob()));
    assert_eq!(first.derivations.len(), 2, "a duplicated input is one node");
    let ids: Vec<&str> = first.derivations.iter().map(|d| d.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "the output is sorted by id");
    assert!(first.derivations.iter().all(|d| d.derived_from == blob().digest));

    let second = plan_media_write(&blob(), true, &derivations).unwrap();
    assert_eq!(second.blob, None, "a second write of the digest adds no blob");
    assert_eq!(second.derivations, first.derivations, "the same ids");

    // Equal scores order by id, whatever the thread count.
    let candidates: Vec<(String, Vec<f32>)> = ["d", "b", "e", "a", "c"]
        .iter()
        .map(|id| (id.to_string(), vec![1.0, 0.0]))
        .collect();
    let expected = match_derivation(&[1.0, 0.0], &candidates, 4);
    let ids: Vec<&str> = expected.iter().map(|hit| hit.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c", "d"]);
    for threads in [1usize, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let got = pool.install(|| match_derivation(&[1.0, 0.0], &candidates, 4));
        assert_eq!(got, expected, "{threads} thread(s)");
    }
}

// spec: MEDULLA-MEDIA-VOCAB-R650.2
#[test]
fn refusals_and_skips() {
    let mut bad = blob();
    bad.digest = "AB".repeat(32);
    assert_eq!(
        plan_media_write(&bad, false, &[]),
        Err(MediaRefusal::DigestMalformed)
    );
    bad.digest = "ab".repeat(31);
    assert_eq!(
        plan_media_write(&bad, false, &[]),
        Err(MediaRefusal::DigestMalformed)
    );
    assert_eq!(
        plan_media_write(&blob(), false, &[input(DerivationKind::Caption, "")]),
        Err(MediaRefusal::DerivationTextEmpty)
    );
    let candidates = vec![
        ("short".to_string(), vec![1.0]),
        ("zero".to_string(), vec![0.0, 0.0]),
        ("ok".to_string(), vec![0.0, 1.0]),
    ];
    let hits = match_derivation(&[0.0, 1.0], &candidates, 10);
    assert_eq!(hits.len(), 1, "a length mismatch and a zero norm are not hits");
    assert_eq!(hits[0].id, "ok");
}

// spec: MEDULLA-MEDIA-VOCAB-R650.4
#[test]
fn sniff_table_names_each_supported_type() {
    use medulla_modality::media_memory::sniff_mime;
    let riff = |form: &[u8]| {
        let mut head = b"RIFF\0\0\0\0".to_vec();
        head.extend_from_slice(form);
        head
    };
    let mut mp4 = vec![0, 0, 0, 0x20];
    mp4.extend_from_slice(b"ftypM4A ");
    for (head, mime) in [
        (b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(), "image/png"),
        (b"\xff\xd8\xff\xe0\0\x10JFIF\0".to_vec(), "image/jpeg"),
        (riff(b"WEBPVP8 "), "image/webp"),
        (b"GIF89a\x01\0\x01\0".to_vec(), "image/gif"),
        (b"ID3\x04\0\0\0\0\0\0".to_vec(), "audio/mpeg"),
        (b"OggS\0\x02\0\0\0\0\0\0".to_vec(), "audio/ogg"),
        (riff(b"WAVEfmt "), "audio/wav"),
        (mp4, "audio/mp4"),
        (b"%PDF-1.7\n%\xe2\xe3".to_vec(), "application/pdf"),
    ] {
        assert_eq!(sniff_mime(&head), Some(mime));
    }
    // An unlisted type, a short head and a form tag without the RIFF tag name nothing.
    assert_eq!(sniff_mime(b"MZ\x90\0\x03\0\0\0\x04\0\0\0\xff\xff\0\0"), None);
    assert_eq!(sniff_mime(b"\xff"), None);
    assert_eq!(sniff_mime(b"XXXX\0\0\0\0WEBPVP8 "), None);
}
