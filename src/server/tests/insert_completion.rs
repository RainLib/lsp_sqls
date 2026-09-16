use super::*;

#[test]
fn insert_all_columns_completion_adds_values_tab_stops() {
    let sql = "INSERT INTO app.orders (";
    let mut items = Vec::new();
    add_insert_all_columns_completion(
        sql,
        lsp_position_at_end(sql),
        Some(&completion_schema_with_columns()),
        &mut items,
        &CompletionPreferences {
            keyword_case: KeywordCase::Lower,
            function_case: KeywordCase::Preserve,
            table_alias: TableAliasStyle::None,
        },
        "postgres",
    );

    let item = items
        .iter()
        .find(|item| item.label == "orders.*")
        .expect("all-column completion");
    assert_eq!(
        item.insert_text.as_deref(),
        Some("id, customer_id, total) values (${1:value}, ${2:value}, ${3:value})")
    );
    assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));

    let nested_expression = "INSERT INTO app.orders (COALESCE(";
    let mut nested_items = Vec::new();
    add_insert_all_columns_completion(
        nested_expression,
        lsp_position_at_end(nested_expression),
        Some(&completion_schema_with_columns()),
        &mut nested_items,
        &CompletionPreferences::default(),
        "postgres",
    );
    assert!(nested_items.is_empty());
}

#[test]
fn insert_completion_prefers_required_columns_and_skips_database_owned_columns() {
    let mut schema = completion_schema_with_columns();
    schema.tables[0].columns = vec![
        Column {
            name: "id".to_string(),
            data_type: "bigint".to_string(),
            auto_increment: true,
            ..Default::default()
        },
        Column {
            name: "customer_id".to_string(),
            data_type: "bigint".to_string(),
            ..Default::default()
        },
        Column {
            name: "note".to_string(),
            data_type: "text".to_string(),
            nullable: true,
            ..Default::default()
        },
        Column {
            name: "created_at".to_string(),
            data_type: "timestamp".to_string(),
            default_value: Some("CURRENT_TIMESTAMP".to_string()),
            ..Default::default()
        },
        Column {
            name: "search_vector".to_string(),
            data_type: "tsvector".to_string(),
            generated: true,
            ..Default::default()
        },
        Column {
            name: "period_start".to_string(),
            data_type: "timestamp".to_string(),
            hidden: true,
            ..Default::default()
        },
    ];

    let sql = "INSERT INTO app.orders (";
    let mut items = Vec::new();
    add_insert_all_columns_completion(
        sql,
        lsp_position_at_end(sql),
        Some(&schema),
        &mut items,
        &CompletionPreferences::default(),
        "postgres",
    );

    let required = items
        .iter()
        .find(|item| item.label == "orders.required")
        .expect("required-column completion");
    assert_eq!(
        required.insert_text.as_deref(),
        Some("customer_id) VALUES (${1:value})")
    );
    let writable = items
        .iter()
        .find(|item| item.label == "orders.*")
        .expect("writable-column completion");
    assert_eq!(
        writable.insert_text.as_deref(),
        Some("customer_id, note, created_at) VALUES (${1:value}, ${2:value}, ${3:value})")
    );
    assert!(!writable
        .insert_text
        .as_deref()
        .unwrap()
        .contains("search_vector"));
    assert!(!writable
        .insert_text
        .as_deref()
        .unwrap()
        .contains("period_start"));
}

#[test]
fn insert_statement_snippets_are_table_specific_and_database_owned_safe() {
    let mut schema = completion_schema_with_columns();
    schema.tables[0].columns = vec![
        Column {
            name: "id".to_string(),
            data_type: "bigint".to_string(),
            auto_increment: true,
            ..Default::default()
        },
        Column {
            name: "customer_id".to_string(),
            data_type: "bigint".to_string(),
            ..Default::default()
        },
        Column {
            name: "order note".to_string(),
            data_type: "text".to_string(),
            nullable: true,
            ..Default::default()
        },
        Column {
            name: "created_at".to_string(),
            data_type: "timestamp".to_string(),
            default_value: Some("CURRENT_TIMESTAMP".to_string()),
            ..Default::default()
        },
        Column {
            name: "search_vector".to_string(),
            data_type: "tsvector".to_string(),
            generated: true,
            ..Default::default()
        },
    ];
    schema.tables.push(Table {
        name: "order_view".to_string(),
        object_type: Some("VIEW".to_string()),
        columns: vec![Column {
            name: "id".to_string(),
            data_type: "bigint".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    });

    let sql = "ins";
    let mut items = Vec::new();
    add_insert_statement_snippets(
        sql,
        lsp_position_at_end(sql),
        Some(&schema),
        &mut items,
        &CompletionPreferences::default(),
        "postgres",
    );

    let required = items
        .iter()
        .find(|item| item.label == "insert orders required")
        .expect("required table INSERT snippet");
    assert_eq!(
        required.insert_text.as_deref(),
        Some("INSERT INTO app.orders (\n  customer_id\n)\nVALUES (\n  ${1:customer_id_value}\n);")
    );
    let complete = items
        .iter()
        .find(|item| item.label == "insert orders")
        .expect("complete table INSERT snippet");
    let complete_sql = complete.insert_text.as_deref().unwrap();
    assert!(complete_sql.contains("\"order note\""));
    assert!(complete_sql.contains("${2:order_note_value}"));
    assert!(!complete_sql.contains("search_vector"));
    assert!(!complete_sql.contains("\n  id"));
    assert!(!items.iter().any(|item| item.label.contains("order_view")));

    let mut after_keyword = Vec::new();
    add_insert_statement_snippets(
        "INSERT ",
        lsp_position_at_end("INSERT "),
        Some(&schema),
        &mut after_keyword,
        &CompletionPreferences::default(),
        "postgres",
    );
    assert!(after_keyword.is_empty());
}

#[test]
fn generated_column_lists_quote_reserved_and_unicode_identifiers() {
    let mut schema = completion_schema_with_columns();
    schema.tables.push(Table {
        name: "事件".to_string(),
        columns: vec![
            Column {
                name: "id".to_string(),
                data_type: "text".to_string(),
                ..Default::default()
            },
            Column {
                name: "order".to_string(),
                data_type: "text".to_string(),
                ..Default::default()
            },
            Column {
                name: "显示名称".to_string(),
                data_type: "text".to_string(),
                ..Default::default()
            },
            Column {
                name: "隐藏值".to_string(),
                data_type: "text".to_string(),
                hidden: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let insert_sql = "INSERT INTO app.事件 (";
    let mut items = Vec::new();
    add_insert_all_columns_completion(
        insert_sql,
        lsp_position_at_end(insert_sql),
        Some(&schema),
        &mut items,
        &CompletionPreferences::default(),
        "mysql",
    );
    assert_eq!(
        items[0].insert_text.as_deref(),
        Some("id, `order`, `显示名称`) VALUES (${1:value}, ${2:value}, ${3:value})")
    );

    let select_sql = "SELECT e.* FROM app.事件 e";
    let uri = Url::parse("file:///query.sql").unwrap();
    let star = select_sql.find('*').unwrap();
    let action = expand_select_star_action(
        select_sql,
        &uri,
        range_for_offsets(select_sql, star, star + 1),
        Some(schema),
        "mysql",
    )
    .expect("quoted star expansion");
    let edit = &action.edit.unwrap().changes.unwrap()[&uri][0];
    assert_eq!(edit.new_text, "id, e.`order`, e.`显示名称`");
}
