use sql_lsp::dialect::Dialect;
use sql_lsp::dialects::{
    ClickHouseDialect, HiveDialect, MysqlDialect, PostgresDialect, SqliteDialect,
};
use sql_lsp::schema::{Column, Schema, SchemaId, Table};
use tower_lsp::lsp_types::{CompletionItemKind, InsertTextFormat, Position};

async fn bit_literals(profile: &str, data_type: &str, expected: &[&str]) {
    let dialect = sql_lsp::dialects::DialectRegistry::new()
        .get_by_name(profile)
        .unwrap();
    let mut catalog = schema();
    catalog.tables[0].columns.push(Column {
        name: "flags".into(),
        data_type: data_type.into(),
        ..Default::default()
    });
    for sql in [
        "SELECT * FROM jobs WHERE flags = ",
        "UPDATE jobs SET flags = ",
        "SELECT * FROM jobs WHERE flags IN (",
    ] {
        let items = dialect
            .completion(sql, Position::new(0, sql.len() as u32), Some(&catalog))
            .await;
        for label in expected {
            assert!(
                items.iter().any(|item| item.label == *label),
                "{profile} {data_type}: missing {label}: {items:?}"
            );
        }
        assert!(
            !items
                .iter()
                .any(|item| item.kind == Some(CompletionItemKind::KEYWORD)
                    && ["TRUE", "FALSE"].contains(&item.label.as_str())),
            "{profile} {data_type} must not offer Boolean keywords for bit storage"
        );
    }
}

#[tokio::test]
async fn sqlserver_bit_literals_are_numeric() {
    bit_literals("sqlserver", "bit", &["0", "1", "NULL"]).await;
}
#[tokio::test]
async fn postgres_fixed_bit_literals_keep_the_declared_width() {
    bit_literals("postgres", "bit(3)", &["B'000'", "B'001'", "NULL"]).await;
}
#[tokio::test]
async fn postgres_varbit_literals_keep_variable_width() {
    bit_literals("postgres", "bit varying(8)", &["B'0'", "B'1'"]).await;
}
#[tokio::test]
async fn mysql_bit_literals_are_binary_values() {
    bit_literals("mysql", "BIT(3)", &["B'000'", "B'001'"]).await;
}
#[tokio::test]
async fn duckdb_bit_literals_use_an_explicit_cast_on_the_existing_baseline() {
    bit_literals("duckdb", "BIT", &["'0'::BIT", "'1'::BIT"]).await;
}
#[tokio::test]
async fn sqlite_bit_affinity_uses_numeric_literals() {
    bit_literals("sqlite", "BIT", &["0", "1"]).await;
}
#[tokio::test]
async fn postgres_boolean_arrays_do_not_inherit_scalar_truth_keywords() {
    let dialect = PostgresDialect::new();
    let mut catalog = schema();
    catalog.tables[0].columns[3].data_type = "boolean[]".into();
    let sql = "SELECT * FROM jobs WHERE enabled = ";
    let items = dialect
        .completion(sql, Position::new(0, sql.len() as u32), Some(&catalog))
        .await;
    assert!(items.iter().any(|item| item.label == "NULL"));
    assert!(!items
        .iter()
        .any(|item| item.kind == Some(CompletionItemKind::KEYWORD)
            && ["TRUE", "FALSE"].contains(&item.label.as_str())));
}

#[tokio::test]
async fn typed_values_do_not_leak_from_a_completed_predicate_into_new_operands() {
    let dialect = PostgresDialect::new();
    for sql in [
        "SELECT * FROM jobs WHERE title = 'ready' AND ",
        "SELECT * FROM jobs WHERE attempts = 1 OR ",
        "SELECT (enabled = TRUE) FROM jobs WHERE ",
    ] {
        let items = dialect
            .completion(sql, Position::new(0, sql.len() as u32), Some(&schema()))
            .await;
        assert!(
            items
                .iter()
                .any(|item| item.label == "TRUE" && item.kind == Some(CompletionItemKind::KEYWORD)),
            "{sql}"
        );
        assert!(
            !items.iter().any(|item| item
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains(" for title")
                    || detail.contains(" for attempts")
                    || detail.contains(" for enabled"))),
            "{sql}: {items:?}"
        );
    }
}
#[tokio::test]
async fn in_list_and_between_bounds_keep_the_numeric_field_type() {
    let dialect = PostgresDialect::new();
    for sql in [
        "SELECT * FROM jobs WHERE attempts IN (1, ",
        "SELECT * FROM jobs WHERE attempts BETWEEN ",
        "SELECT * FROM jobs WHERE attempts BETWEEN 1 AND ",
    ] {
        let items = dialect
            .completion(sql, Position::new(0, sql.len() as u32), Some(&schema()))
            .await;
        assert!(
            items.iter().any(|item| item.label == "0"),
            "{sql}: {items:?}"
        );
        assert!(
            !items
                .iter()
                .any(|item| item.label == "TRUE" && item.kind == Some(CompletionItemKind::KEYWORD)),
            "{sql}"
        );
    }
}

fn schema() -> Schema {
    Schema {
        id: SchemaId::new(),
        server: None,
        namespace: None,
        catalog: None,
        database: "app".to_string(),
        server_version: None,
        tables: vec![Table {
            name: "jobs".to_string(),
            columns: vec![
                Column {
                    name: "status".to_string(),
                    data_type: "enum('active','paused','can''t-run')".to_string(),
                    ..Default::default()
                },
                Column {
                    name: "title".to_string(),
                    data_type: "TEXT".to_string(),
                    ..Default::default()
                },
                Column {
                    name: "attempts".to_string(),
                    data_type: "INTEGER".to_string(),
                    ..Default::default()
                },
                Column {
                    name: "enabled".to_string(),
                    data_type: "BOOLEAN".to_string(),
                    ..Default::default()
                },
                Column {
                    name: "created_at".to_string(),
                    data_type: "TIMESTAMP".to_string(),
                    ..Default::default()
                },
                Column {
                    name: "payload".to_string(),
                    data_type: "JSON".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
        functions: vec![],
        source_uri: None,
    }
}

#[tokio::test]
async fn value_reference_uses_the_qualified_relation_and_keeps_unknown_qualifiers_conservative() {
    let mut catalog = schema();
    catalog.tables.push(Table {
        name: "metrics".into(),
        columns: vec![Column {
            name: "title".into(),
            data_type: "integer".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let dialect = PostgresDialect::new();
    for (sql, expected, absent) in [
        (
            "SELECT * FROM jobs j CROSS JOIN metrics m WHERE j.title = ",
            Some("''"),
            Some("0"),
        ),
        (
            "SELECT * FROM jobs j CROSS JOIN metrics m WHERE m.title = ",
            Some("0"),
            Some("''"),
        ),
        (
            "SELECT * FROM jobs j CROSS JOIN metrics m WHERE missing.title = ",
            None,
            Some("''"),
        ),
        (
            "SELECT * FROM jobs j WHERE EXISTS (SELECT 1 FROM metrics m WHERE j.title = ",
            Some("''"),
            Some("0"),
        ),
    ] {
        let items = dialect
            .completion(sql, Position::new(0, sql.len() as u32), Some(&catalog))
            .await;
        if let Some(label) = expected {
            assert!(
                items.iter().any(|item| item.label == label),
                "{sql}: {items:?}"
            );
        }
        if let Some(label) = absent {
            assert!(
                !items.iter().any(|item| item.label == label),
                "{sql}: {items:?}"
            );
        }
    }
}

#[tokio::test]
async fn domain_value_prefix_search_happens_before_the_candidate_limit() {
    let mut catalog = schema();
    catalog.tables[0].columns[0].data_type = format!(
        "enum({})",
        (0..100)
            .map(|i| format!("'value{i:03}'"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let sql = "SELECT * FROM jobs WHERE status = value099";
    let items = MysqlDialect::new()
        .completion(sql, Position::new(0, sql.len() as u32), Some(&catalog))
        .await;
    assert!(items
        .iter()
        .any(|item| item.label == "value099" && item.insert_text.as_deref() == Some("'value099'")));
}

async fn assert_domain_completion(name: &str, dialect: &dyn Dialect) {
    let sql = "SELECT * FROM jobs WHERE status = ";
    let items = dialect
        .completion(sql, Position::new(0, sql.len() as u32), Some(&schema()))
        .await;
    let active = items
        .iter()
        .find(|item| item.label == "active")
        .unwrap_or_else(|| panic!("{name} must suggest enum values in predicates: {items:#?}"));
    assert_eq!(active.kind, Some(CompletionItemKind::ENUM_MEMBER));
    assert_eq!(active.insert_text.as_deref(), Some("'active'"));
    assert!(items.iter().any(|item| item.label == "paused"));

    let prefixed_sql = "SELECT * FROM jobs WHERE status = a";
    let prefixed_items = dialect
        .completion(
            prefixed_sql,
            Position::new(0, prefixed_sql.len() as u32),
            Some(&schema()),
        )
        .await;
    assert!(prefixed_items.iter().any(|item| item.label == "active"));
    assert!(!prefixed_items.iter().any(|item| item.label == "paused"));

    let in_sql = "SELECT * FROM jobs j WHERE j.status IN (";
    let in_items = dialect
        .completion(
            in_sql,
            Position::new(0, in_sql.len() as u32),
            Some(&schema()),
        )
        .await;
    assert!(in_items.iter().any(|item| {
        item.label == "can't-run" && item.insert_text.as_deref() == Some("'can''t-run'")
    }));

    for (column, label, insert_text) in [
        ("title", "''", "'${1:value}'"),
        ("attempts", "0", "${1:0}"),
        (
            "created_at",
            "'YYYY-MM-DD HH:MM:SS'",
            "'${1:YYYY-MM-DD HH:MM:SS}'",
        ),
        ("payload", "'{}'", r#"'{"${1:key}":"${2:value}"}'"#),
    ] {
        let sql = format!("SELECT * FROM jobs WHERE {column} = ");
        let items = dialect
            .completion(&sql, Position::new(0, sql.len() as u32), Some(&schema()))
            .await;
        let template = items
            .iter()
            .find(|item| item.label == label)
            .unwrap_or_else(|| panic!("{name} must suggest {label} for {column}: {items:#?}"));
        assert_eq!(template.kind, Some(CompletionItemKind::SNIPPET));
        assert_eq!(template.insert_text.as_deref(), Some(insert_text));
        assert_eq!(template.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert!(items.iter().any(|item| item.label == "NULL"));
    }

    let boolean_sql = "SELECT * FROM jobs WHERE enabled = ";
    let boolean_items = dialect
        .completion(
            boolean_sql,
            Position::new(0, boolean_sql.len() as u32),
            Some(&schema()),
        )
        .await;
    assert!(boolean_items.iter().any(|item| item.label == "TRUE"));
    assert!(boolean_items.iter().any(|item| item.label == "FALSE"));

    for sql in [
        "SELECT enabled, count(*) FROM jobs GROUP BY enabled HAVING enabled = ",
        "SELECT * FROM jobs j JOIN jobs k ON k.enabled = ",
        "UPDATE jobs SET enabled = ",
    ] {
        let items = dialect
            .completion(sql, Position::new(0, sql.len() as u32), Some(&schema()))
            .await;
        assert!(
            items.iter().any(|item| item.label == "TRUE"),
            "{name} must offer typed values in comparison contexts: {sql}: {items:#?}",
        );
    }

    let mut ambiguous_schema = schema();
    ambiguous_schema.tables[0].columns.push(Column {
        name: "shared_value".to_string(),
        data_type: "TEXT".to_string(),
        ..Default::default()
    });
    ambiguous_schema.tables.push(Table {
        name: "metrics".to_string(),
        columns: vec![Column {
            name: "shared_value".to_string(),
            data_type: "INTEGER".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let ambiguous_sql = "SELECT * FROM jobs CROSS JOIN metrics WHERE shared_value = ";
    let ambiguous_items = dialect
        .completion(
            ambiguous_sql,
            Position::new(0, ambiguous_sql.len() as u32),
            Some(&ambiguous_schema),
        )
        .await;
    assert!(ambiguous_items.iter().any(|item| item.label == "NULL"));
    assert!(!ambiguous_items.iter().any(|item| item.label == "''"));
    assert!(!ambiguous_items.iter().any(|item| item.label == "0"));
}

#[tokio::test]
async fn relational_dialects_offer_bounded_metadata_value_completions() {
    let dialects: Vec<(&str, Box<dyn Dialect>)> = vec![
        ("postgres", Box::new(PostgresDialect::new())),
        ("mysql", Box::new(MysqlDialect::new())),
        ("hive", Box::new(HiveDialect::new())),
        ("clickhouse", Box::new(ClickHouseDialect::new())),
        ("sqlite", Box::new(SqliteDialect::new())),
    ];
    for (name, dialect) in dialects {
        assert_domain_completion(name, dialect.as_ref()).await;
    }
}
