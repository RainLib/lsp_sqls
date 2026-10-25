use super::*;
#[test]
fn duckdb_uri_and_language_keep_the_registered_identity_before_configuration() {
    for uri in ["file:///q.duckdb", "oxide://query/q.duckdb.sql"] {
        assert_eq!(
            infer_dialect_from_uri_and_language(uri, "mysql", "postgres"),
            "duckdb"
        );
    }
    assert_eq!(
        infer_dialect_from_uri_and_language("untitled://1", "duckdb", "mysql"),
        "duckdb"
    );
}

#[test]
fn infers_dialect_from_uri_before_language_id() {
    assert_eq!(
        infer_dialect_from_uri_and_language("file:///query.mysql.sql", "postgres", "postgres"),
        "mysql"
    );
    assert_eq!(
        infer_dialect_from_uri_and_language("file:///query.psql", "mysql", "mysql"),
        "postgres"
    );
    assert_eq!(
        infer_dialect_from_uri_and_language("file:///query.mongo.json", "sql", "postgres"),
        "mongodb"
    );
    assert_eq!(
        infer_dialect_from_uri_and_language("oxide://query/tab.duckdb.sqlite", "sql", "postgres"),
        "sqlite"
    );
    for (product, expected) in PRODUCT_SQL_FILE_SUFFIX_DIALECTS {
        let uri = format!("file:///query.audit.{product}.sql");
        assert_eq!(
            infer_dialect_from_uri_and_language(&uri, "sql", "mysql"),
            *expected,
            "{uri} should not fall back before configuration sync"
        );
        let extension_uri = format!("file:///query.audit.{product}");
        assert_eq!(
            infer_dialect_from_uri_and_language(&extension_uri, "sql", "mysql"),
            *expected,
            "{extension_uri} should not fall back before configuration sync"
        );
    }
}

#[test]
fn infers_dialect_from_language_id_before_default() {
    assert_eq!(
        infer_dialect_from_uri_and_language("untitled://1", "pgsql", "mysql"),
        "postgres"
    );
    assert_eq!(
        infer_dialect_from_uri_and_language("untitled://1", "mysql-sql", "postgres"),
        "mysql"
    );
    assert_eq!(
        infer_dialect_from_uri_and_language("untitled://1", "json", "postgres"),
        "mongodb"
    );
    assert_eq!(
        infer_dialect_from_uri_and_language("untitled://1", "sqlite", "mysql"),
        "sqlite"
    );
}

#[test]
fn falls_back_to_configured_default_dialect() {
    assert_eq!(
        infer_dialect_from_uri_and_language("untitled://1", "sql", "postgres"),
        "postgres"
    );
    assert_eq!(
        infer_dialect_from_uri_and_language("untitled://1", "sql", "clickhouse"),
        "clickhouse"
    );
}

#[test]
fn sqlserver_language_and_uri_aliases_do_not_fall_back_to_default() {
    for alias in ["sqlserver", "mssql", "tsql"] {
        for uri in [
            format!("file:///q.{alias}"),
            format!("file:///q.{alias}.sql"),
        ] {
            assert_eq!(
                infer_dialect_from_uri_and_language(&uri, "postgres", "mysql"),
                "sqlserver"
            );
        }
        assert_eq!(
            infer_dialect_from_uri_and_language("untitled://1", alias, "mysql"),
            "sqlserver"
        );
    }
    assert_eq!(
        infer_dialect_from_uri_and_language("untitled://1", "transact-sql", "postgres"),
        "sqlserver"
    );
    assert_eq!(
        infer_dialect_from_uri_and_language("file:///q.mysql.sql", "tsql", "sqlserver"),
        "mysql"
    );
}
