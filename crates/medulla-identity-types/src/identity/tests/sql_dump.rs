//! MEDULLA-IDENTITY-R707: the identity catalog's DDL dump round-trips and
//! never carries a password hash, a session hash or a sealed value.

use super::auth::set_password_op;
use super::*;

#[test]
fn dump_round_trips_and_redacts_secrets() {
    let mut store = store_in(AuthMode::Local);
    let principal = create(&mut store, "dana", UserKind::Human).expect("create");
    let (op, stamp) = set_password_op(&store, &principal, "$argon2id$v=19$secret-hash");
    apply_kept(&mut store, &op, &stamp, NOW).expect("set password");
    open_session(&mut store, "dana", &principal, "dana-session-hash");

    let relations = store.sql_relations();
    let dump = render_dump(&relations);
    for secret in ["argon2", "secret-hash", "dana-session-hash"] {
        assert!(!dump.contains(secret), "dump leaked {secret}: {dump}");
    }
    assert!(dump.contains("CREATE TABLE"));
    assert!(dump.contains("identity.users"));

    let rows = parse_dump(&dump).expect("the dump parses back");
    let dumped: Vec<_> = relations
        .iter()
        .filter(|relation| !["sessions", "throttle"].contains(&relation.name))
        .collect();
    let expected: usize = dumped.iter().map(|relation| relation.rows.len()).sum();
    assert_eq!(rows.len(), expected, "one decoded row per dumped row");
    for relation in dumped {
        let mut decoded = rows
            .iter()
            .filter(|row| row.relation == relation.name)
            .map(|row| row.values.clone())
            .collect::<Vec<_>>();
        let mut original = relation.rows.clone();
        decoded.sort_by_key(|row| serde_json::to_string(row).unwrap());
        original.sort_by_key(|row| serde_json::to_string(row).unwrap());
        assert_eq!(decoded, original, "relation {} round-trips", relation.name);
    }
    let dana = rows
        .iter()
        .find(|row| row.relation == "users" && row.text("username").as_deref() == Some("dana"))
        .expect("dana is dumped");
    assert_eq!(dana.get("has_password"), &serde_json::Value::Bool(true));
    assert!(rows.iter().all(|row| row.relation != "sessions"));
}
