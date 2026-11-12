//! Live MySQL and MariaDB catalog read and type map (MEDULLA-UNIFIED-DATA-PLANE-R014.2.2).
//!
//! Needs one MySQL and one MariaDB server: `MEDULLA_TEST_MYSQL_DSN` and `MEDULLA_TEST_MARIADB_DSN`
//! name a `mysql://` account that may create and drop the schema `medulla_probe_fixture`. The
//! nightly lane starts both containers and runs `cargo test --test attached_mysql_served --
//! --ignored`; a pull request runs the typed unit tests of `medulla_plan::mysql_catalog`.

use medulla_catalog_types::attached_catalog::{AttachedCatalogGraph, CatalogConstraintKind};
use medulla_federation::mysql_introspect::run_statements;
use medulla_plan::mysql_catalog::{read_catalog, MysqlCatalogRead};
use medulla_types::attached_source_dialect::SqlEngineKind;

const SCHEMA: &str = "medulla_probe_fixture";

const FIXTURE: &[&str] = &[
    "DROP DATABASE IF EXISTS medulla_probe_fixture",
    "CREATE DATABASE medulla_probe_fixture",
    "CREATE TABLE medulla_probe_fixture.customers (\
        id BIGINT UNSIGNED NOT NULL PRIMARY KEY, \
        name VARCHAR(80) NOT NULL, \
        active TINYINT(1) NOT NULL, \
        balance DECIMAL(12,2) NULL, \
        tier ENUM('a','b','c') NULL, \
        attrs JSON NULL, \
        spot POINT NULL, \
        KEY idx_customers_name (name))",
    "CREATE TABLE medulla_probe_fixture.orders (\
        id BIGINT UNSIGNED NOT NULL PRIMARY KEY, \
        customer_id BIGINT UNSIGNED NOT NULL, \
        KEY idx_orders_customer (customer_id), \
        CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) \
            REFERENCES medulla_probe_fixture.customers (id))",
    "CREATE VIEW medulla_probe_fixture.v_totals AS \
        SELECT c.id AS customer_id, COUNT(o.id) AS orders \
        FROM medulla_probe_fixture.customers c \
        LEFT JOIN medulla_probe_fixture.orders o ON o.customer_id = c.id GROUP BY c.id",
];

fn dsn(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("{var} must name the containerised server"))
}

async fn read(dsn: &str) -> MysqlCatalogRead {
    run_statements(dsn, FIXTURE).await.expect("fixture schema");
    read_catalog(dsn, &[SCHEMA.to_string()], 4)
        .await
        .expect("live catalog read")
}

fn type_of(graph: &AttachedCatalogGraph, table: &str, column: &str) -> String {
    graph
        .tables
        .iter()
        .find(|t| t.name == table)
        .and_then(|t| t.columns.iter().find(|c| c.name == column))
        .unwrap_or_else(|| panic!("column {table}.{column}"))
        .type_name
        .clone()
}

fn check_types(read: &MysqlCatalogRead) {
    let g = &read.graph;
    assert_eq!(type_of(g, "customers", "id"), "bigint unsigned");
    assert_eq!(type_of(g, "customers", "name"), "varchar(80)");
    assert_eq!(type_of(g, "customers", "active"), "boolean");
    assert_eq!(type_of(g, "customers", "balance"), "decimal(12,2)");
    assert_eq!(type_of(g, "customers", "tier"), "enum('a','b','c')");
    assert_eq!(type_of(g, "customers", "attrs"), "json");
    assert_eq!(type_of(g, "customers", "spot"), "opaque:point");
    let orders = g.tables.iter().find(|t| t.name == "orders").unwrap();
    let fk = orders
        .constraints
        .iter()
        .find(|c| c.kind == CatalogConstraintKind::ForeignKey)
        .expect("foreign key");
    assert_eq!(fk.references.as_ref().unwrap().table, "customers");
    assert!(orders
        .constraints
        .iter()
        .any(|c| c.kind == CatalogConstraintKind::Index && c.name == "idx_orders_customer"));
    assert_eq!(g.views.len(), 1);
    assert!(g.views[0]
        .definition_sql
        .to_lowercase()
        .contains("customers"));
    assert_eq!(read.capture, "debezium");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.2
#[tokio::test]
#[ignore = "nightly lane: needs containerised MySQL and MariaDB"]
async fn catalog_and_types_match_native() {
    let mysql = read(&dsn("MEDULLA_TEST_MYSQL_DSN")).await;
    let maria = read(&dsn("MEDULLA_TEST_MARIADB_DSN")).await;
    assert_eq!(mysql.engine, SqlEngineKind::MySql);
    assert_eq!(maria.engine, SqlEngineKind::MariaDb);
    check_types(&mysql);
    check_types(&maria);
    // Both engines hold one schema, so their typed tables hash to one value. A view's stored
    // text is each engine's own rendering and is compared by presence above, not by hash.
    let without_views = |mut g: AttachedCatalogGraph| {
        g.views.clear();
        g.content_hash()
    };
    assert_eq!(
        without_views(mysql.graph.clone()),
        without_views(maria.graph.clone())
    );
    // A second read of an unchanged server hashes identically.
    let again = read(&dsn("MEDULLA_TEST_MYSQL_DSN")).await;
    assert_eq!(again.graph.content_hash(), mysql.graph.content_hash());
}
