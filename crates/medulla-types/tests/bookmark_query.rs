use medulla_types::bookmark_query::{
    BookmarkQueryLimitPolicy, BookmarkRelationsRequest, BookmarkSemanticSearchRequest,
    BOOKMARK_LIMIT_EXCEEDED, BOOKMARK_POLICY_INVALID,
};

fn policy() -> BookmarkQueryLimitPolicy {
    BookmarkQueryLimitPolicy {
        max_hits: 100,
        max_relationships: 500,
        max_response_bytes: 1_048_576,
    }
}

fn search() -> BookmarkSemanticSearchRequest {
    BookmarkSemanticSearchRequest {
        query_embedding: vec![0.25, -0.5, 1.0],
        source_ids: vec!["src-1".to_string()],
        limit_policy: policy(),
        cursor: None,
    }
}

// spec: MEDULLA-MEDIA-VOCAB-R001.102.1
#[test]
fn records_round_trip_and_reject_bad_limits() {
    let request = search();
    assert_eq!(request.validate(), Ok(()));
    let json = serde_json::to_string(&request).unwrap();
    assert_eq!(
        serde_json::from_str::<BookmarkSemanticSearchRequest>(&json).unwrap(),
        request
    );
    let relations = BookmarkRelationsRequest {
        node_ids: vec!["b1".to_string(), "b2".to_string()],
        limit_policy: policy(),
        cursor: Some("c".to_string()),
    };
    assert_eq!(relations.validate(), Ok(()));
    let bytes = rmp_serde::to_vec_named(&relations).unwrap();
    assert_eq!(
        rmp_serde::from_slice::<BookmarkRelationsRequest>(&bytes).unwrap(),
        relations
    );

    let mut zero = search();
    zero.limit_policy.max_hits = 0;
    assert_eq!(zero.validate(), Err(BOOKMARK_POLICY_INVALID));
    let mut over = search();
    over.limit_policy.max_relationships = 501;
    assert_eq!(over.validate(), Err(BOOKMARK_POLICY_INVALID));
    let mut bytes_over = search();
    bytes_over.limit_policy.max_response_bytes = 1_048_577;
    assert_eq!(bytes_over.validate(), Err(BOOKMARK_POLICY_INVALID));
    let mut non_finite = search();
    non_finite.query_embedding = vec![f32::NAN];
    assert_eq!(non_finite.validate(), Err(BOOKMARK_POLICY_INVALID));
    let mut empty = search();
    empty.query_embedding.clear();
    assert_eq!(empty.validate(), Err(BOOKMARK_POLICY_INVALID));

    let mut too_many = relations.clone();
    too_many.limit_policy.max_hits = 1;
    assert_eq!(too_many.validate(), Err(BOOKMARK_LIMIT_EXCEEDED));

    let unknown = r#"{"node_ids":["b"],"limit_policy":{"max_hits":1,"max_relationships":1,"max_response_bytes":1},"cursor":null,"extra":1}"#;
    assert!(serde_json::from_str::<BookmarkRelationsRequest>(unknown).is_err());
}
