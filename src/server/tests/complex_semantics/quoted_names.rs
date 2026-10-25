use super::*;
fn special(dialect: &str, sql: &str) -> Harness {
    let harness = configured(dialect, sql);
    let server = harness.service.inner();
    let id = *server
        .configured_file_schemas
        .get(harness.uri.as_str())
        .unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    schema.tables[0].columns.extend(
        [
            "display.name",
            "列.名",
            "white space",
            "tick`name",
            "quote\"name",
            "display]name",
            "\"literal\"",
            "[literal]",
            "`literal`",
        ]
        .into_iter()
        .map(|name| Column {
            name: name.into(),
            data_type: "text".into(),
            ..Default::default()
        }),
    );
    schema.tables.push(Table {
        name: "odd.table".into(),
        columns: vec![Column {
            name: "value.part".into(),
            data_type: "bigint".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut spaced = schema.tables.last().unwrap().clone();
    spaced.name = "odd table".into();
    schema.tables.push(spaced);
    assert!(server.schema_manager.update(id, schema));
    harness
}
pub(super) async fn typed(dialect: &str, sql: &str, name: &str, ty: &str) {
    let harness = special(dialect, sql);
    let items = complete_harness(&harness).await;
    let item = items
        .iter()
        .find(|item| item.label == name)
        .unwrap_or_else(|| panic!("{dialect}: {sql}: {items:?}"));
    assert!(
        item.detail
            .as_ref()
            .is_some_and(|detail| detail.contains(ty)),
        "{dialect}: {item:?}"
    );
    let hover = harness
        .service
        .inner()
        .hover(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{dialect}: {sql}: hover"));
    let value = serde_json::to_string(&hover).unwrap();
    assert!(
        value.contains("Column") && value.contains(ty),
        "{dialect}: {sql}: {value}"
    );
}
#[tokio::test]
async fn dotted_alias_names_resolve_as_one_component() {
    for (dialect, alias) in [("postgres", r#""o.x""#), ("mysql", "`o.x`")] {
        let sql = format!("SELECT {alias}.na<cursor>me FROM users AS {alias}");
        typed(dialect, &sql, "name", "text").await;
    }
}
#[tokio::test]
async fn bracket_alias_member_completion_keeps_atomic_name() {
    let harness = special("sqlserver", "SELECT [o.x].na<cursor>me FROM users AS [o.x]");
    assert!(complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "name"));
}
#[tokio::test]
async fn dotted_table_and_column_names_keep_catalog_binding() {
    typed(
        "postgres",
        r#"SELECT o."value.pa<cursor>rt" FROM app."odd.table" o"#,
        "value.part",
        "bigint",
    )
    .await;
    typed(
        "mysql",
        "SELECT o.`value.pa<cursor>rt` FROM app.`odd table` o",
        "value.part",
        "bigint",
    )
    .await;
}
#[tokio::test]
async fn dotted_and_spaced_column_names_keep_hover_types() {
    for (dialect, sql, name) in [
        (
            "postgres",
            r#"SELECT u."display.na<cursor>me" FROM users u"#,
            "display.name",
        ),
        (
            "postgres",
            r#"SELECT u."white sp<cursor>ace" FROM users u"#,
            "white space",
        ),
        (
            "mysql",
            "SELECT u.`display.na<cursor>me` FROM users u",
            "display.name",
        ),
        (
            "mysql",
            "SELECT u.`white sp<cursor>ace` FROM users u",
            "white space",
        ),
    ] {
        typed(dialect, sql, name, "text").await;
    }
}
#[tokio::test]
async fn quoted_alias_requires_quotes_when_name_contains_a_dot() {
    for (dialect, sql) in [
        ("postgres", r#"SELECT o.x.na<cursor>me FROM users AS "o.x""#),
        ("mysql", "SELECT o.x.na<cursor>me FROM users AS `o.x`"),
    ] {
        let harness = special(dialect, sql);
        assert!(
            !complete_harness(&harness)
                .await
                .iter()
                .any(|item| item.label == "name"),
            "{dialect}: {sql}"
        );
        let hover=harness.service.inner().hover(serde_json::from_value(serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position})).unwrap()).await.unwrap();
        assert!(hover.is_none(), "{dialect}: {sql}: {hover:?}");
    }
}
#[tokio::test]
async fn quoted_alias_types_flow_through_lateral_output() {
    typed("postgres",r#"SELECT d.va<cursor>lue FROM users AS "o.x" CROSS JOIN LATERAL(SELECT "o.x".name AS value) d"#,"value","text").await;
}

async fn accepted(dialect: &str, marked: &str, name: &str, expected: &str) {
    let harness = special(dialect, marked);
    let items = complete_harness(&harness).await;
    let item = items
        .iter()
        .find(|item| item.label == name)
        .unwrap_or_else(|| panic!("{dialect}: {marked}: {items:?}"));
    let Some(CompletionTextEdit::Edit(edit)) = item.text_edit.as_ref() else {
        panic!("{dialect}: {item:?}: missing edit");
    };
    let mut sql = marked.replace("<cursor>", "");
    let start = crate::server::completion_context::position_to_byte_offset(&sql, edit.range.start);
    let end = crate::server::completion_context::position_to_byte_offset(&sql, edit.range.end);
    sql.replace_range(start..end, &edit.new_text);
    assert_eq!(sql, expected, "{dialect}: {item:?}");
}
#[tokio::test]
async fn accepting_special_column_completion_replaces_complete_member() {
    for (dialect, marked, name, expected) in [
        (
            "postgres",
            r#"SELECT u."display.na<cursor>me" FROM users u"#,
            "display.name",
            r#"SELECT u."display.name" FROM users u"#,
        ),
        (
            "mysql",
            "SELECT u.`white sp<cursor>ace` FROM users u",
            "white space",
            "SELECT u.`white space` FROM users u",
        ),
        (
            "postgres",
            "SELECT u.na<cursor>me FROM users u",
            "name",
            "SELECT u.name FROM users u",
        ),
        (
            "postgres",
            r#"SELECT "o.x".na<cursor>me FROM users AS "o.x""#,
            "name",
            r#"SELECT "o.x".name FROM users AS "o.x""#,
        ),
        (
            "sqlserver",
            "SELECT [o.x].na<cursor>me FROM users AS [o.x]",
            "name",
            "SELECT [o.x].name FROM users AS [o.x]",
        ),
        (
            "postgres",
            "-- 😀\r\nSELECT u.\"display.na<cursor>me\" FROM users u",
            "display.name",
            "-- 😀\r\nSELECT u.\"display.name\" FROM users u",
        ),
    ] {
        accepted(dialect, marked, name, expected).await;
    }
}
#[tokio::test]
async fn escaped_identifier_contents_keep_types() {
    typed(
        "postgres",
        r#"SELECT u."quote""na<cursor>me" FROM users u"#,
        "quote\"name",
        "text",
    )
    .await;
    typed(
        "mysql",
        "SELECT u.`tick``na<cursor>me` FROM users u",
        "tick`name",
        "text",
    )
    .await;
}
#[tokio::test]
async fn accepting_escaped_identifier_completion_does_not_double_quote() {
    accepted(
        "postgres",
        r#"SELECT u."quote""na<cursor>me" FROM users u"#,
        "quote\"name",
        r#"SELECT u."quote""name" FROM users u"#,
    )
    .await;
    accepted(
        "mysql",
        "SELECT u.`tick``na<cursor>me` FROM users u",
        "tick`name",
        "SELECT u.`tick``name` FROM users u",
    )
    .await;
}
#[tokio::test]
async fn quoted_unaliased_outer_table_completion_uses_actual_dialect() {
    for (dialect, table) in [("postgres", r#""odd.table""#), ("mysql", "`odd table`")] {
        let marked = format!("SELECT (SELECT va<cursor> FROM orders) FROM {table}");
        let expected = format!(
            "SELECT (SELECT {table}.{} FROM orders) FROM {table}",
            if dialect == "mysql" {
                "`value.part`"
            } else {
                r#""value.part""#
            }
        );
        // Outer candidates display the relation prefix; metadata retains the atom.
        let harness = special(dialect, &marked);
        let items = complete_harness(&harness).await;
        let item = items
            .iter()
            .find(|item| {
                item.data
                    .as_ref()
                    .and_then(|data| data.pointer("/oxide/column"))
                    .and_then(serde_json::Value::as_str)
                    == Some("value.part")
            })
            .unwrap_or_else(|| panic!("{dialect}: {items:?}"));
        accepted(dialect, &marked, &item.label, &expected).await;
    }
}

#[tokio::test]
async fn metadata_quote_delimiters_are_not_mistaken_for_written_sql() {
    accepted(
        "postgres",
        "SELECT u.<cursor> FROM users u",
        "\"literal\"",
        r#"SELECT u."""literal""" FROM users u"#,
    )
    .await;
    accepted(
        "mysql",
        "SELECT u.<cursor> FROM users u",
        "`literal`",
        "SELECT u.```literal``` FROM users u",
    )
    .await;
    accepted(
        "sqlserver",
        "SELECT u.<cursor> FROM users u",
        "[literal]",
        "SELECT u.[[literal]]] FROM users u",
    )
    .await;
}
#[test]
fn unfinished_quote_edit_does_not_delete_the_rest_of_the_document() {
    // Unclosed quotes can hide FROM from binding analysis. Test edit safety
    // independently of whether that malformed document has semantic candidates.
    let text = "SELECT u.\"display.na FROM users u";
    let position = crate::position::lsp_position_at_end("SELECT u.\"display.na");
    let mut items = vec![CompletionItem {
        label: "display.name".into(),
        kind: Some(CompletionItemKind::FIELD),
        ..Default::default()
    }];
    crate::server::completion_quotes::apply_column_identifier_edits(
        text, position, "postgres", &mut items,
    );
    let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
        panic!("missing edit");
    };
    assert_eq!(edit.range.end, position);
    let mut applied = text.to_string();
    applied.replace_range(
        edit.range.start.character as usize..edit.range.end.character as usize,
        &edit.new_text,
    );
    assert_eq!(applied, "SELECT u.\"display.name\" FROM users u");
}
#[tokio::test]
async fn quoted_cte_and_derived_aliases_keep_output_types() {
    for (dialect, marked) in [
        ("postgres", r#"WITH "c.x" AS (SELECT name AS "display.name" FROM users) SELECT "c.x"."display.na<cursor>me" FROM "c.x""#),
        ("mysql", "WITH `c.x` AS (SELECT name AS `display.name` FROM users) SELECT `c.x`.`display.na<cursor>me` FROM `c.x`"),
        ("postgres", r#"SELECT "d.x"."display.na<cursor>me" FROM (SELECT name AS "display.name" FROM users) AS "d.x""#),
        ("mysql", "SELECT `d.x`.`display.na<cursor>me` FROM (SELECT name AS `display.name` FROM users) AS `d.x`"),
    ] { typed(dialect, marked, "display.name", "text").await; }
}
#[tokio::test]
async fn unicode_member_completion_edits_preserve_utf16_columns() {
    accepted(
        "postgres",
        "/* 😀 */ SELECT u.\"列.<cursor>名\" FROM users u",
        "列.名",
        "/* 😀 */ SELECT u.\"列.名\" FROM users u",
    )
    .await;
    accepted(
        "mysql",
        "/* 😀 */ SELECT u.`列.<cursor>名` FROM users u",
        "列.名",
        "/* 😀 */ SELECT u.`列.名` FROM users u",
    )
    .await;
}

#[tokio::test]
async fn comments_with_quote_delimiters_do_not_capture_completion_edits() {
    for prefix in [
        "/* ' */ ",
        "/* \" */ ",
        "/* ` */ ",
        "/* outer /* ' */ inner */ ",
        "-- \"quote\r\n",
    ] {
        let marked = format!("{prefix}SELECT u.na<cursor>me FROM users u");
        let expected = format!("{prefix}SELECT u.name FROM users u");
        accepted("postgres", &marked, "name", &expected).await;
    }
}
