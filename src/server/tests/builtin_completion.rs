use super::*;

#[test]
fn builtin_function_completion_is_expression_scoped_and_snippet_aware() {
    let sql = "SELECT con";
    let mut items = vec![CompletionItem {
        label: "CONCAT".to_string(),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some("String function: concatenate".to_string()),
        ..Default::default()
    }];
    add_builtin_function_completions(sql, lsp_position_at_end(sql), "mysql", None, &mut items);
    let concat = items
        .iter()
        .find(|item| item.label == "CONCAT")
        .expect("catalog CONCAT completion");
    assert_eq!(
        concat.insert_text.as_deref(),
        Some("CONCAT(${1:value}, ${2:values})")
    );
    assert_eq!(concat.insert_text_format, Some(InsertTextFormat::SNIPPET));
    assert_eq!(
        items.iter().filter(|item| item.label == "CONCAT").count(),
        1,
        "the richer catalog item should replace a legacy hard-coded item"
    );

    let relation_sql = "SELECT * FROM con";
    let mut relation_items = Vec::new();
    add_builtin_function_completions(
        relation_sql,
        lsp_position_at_end(relation_sql),
        "mysql",
        None,
        &mut relation_items,
    );
    assert!(relation_items.is_empty());

    let literal_sql = "SELECT 'con";
    let mut literal_items = Vec::new();
    add_builtin_function_completions(
        literal_sql,
        lsp_position_at_end(literal_sql),
        "mysql",
        None,
        &mut literal_items,
    );
    assert!(literal_items.is_empty());
}

#[test]
fn clickhouse_dbx_catalog_supports_aliases_combinators_and_table_functions() {
    let sql = "SELECT arraym";
    let mut items = Vec::new();
    add_builtin_function_completions(
        sql,
        lsp_position_at_end(sql),
        "clickhouse",
        None,
        &mut items,
    );
    assert_eq!(
        items
            .iter()
            .find(|item| item.label == "arrayMap")
            .and_then(|item| item.insert_text.as_deref()),
        Some("arrayMap(${1:lambda}, ${2:array}, ${3:arrays})")
    );

    let alias_sql = "SELECT uca";
    let mut alias_items = Vec::new();
    add_builtin_function_completions(
        alias_sql,
        lsp_position_at_end(alias_sql),
        "clickhouse",
        None,
        &mut alias_items,
    );
    let upper = alias_items
        .iter()
        .find(|item| item.label == "upper")
        .expect("alias lookup returns the canonical function");
    assert_eq!(upper.filter_text.as_deref(), Some("uca upper"));

    let combinator_sql = "SELECT sumarrayif";
    let mut combinator_items = Vec::new();
    add_builtin_function_completions(
        combinator_sql,
        lsp_position_at_end(combinator_sql),
        "clickhouse",
        None,
        &mut combinator_items,
    );
    assert_eq!(
        combinator_items
            .iter()
            .find(|item| item.label == "sumArrayIf")
            .and_then(|item| item.insert_text.as_deref()),
        Some("sumArrayIf(${1:array}, ${2:condition})")
    );

    let relation_sql = "SELECT * FROM num";
    let mut relation_items = Vec::new();
    add_clickhouse_table_function_completions(
        relation_sql,
        lsp_position_at_end(relation_sql),
        "clickhouse",
        &mut relation_items,
    );
    let numbers = relation_items
        .iter()
        .find(|item| item.label == "numbers")
        .expect("ClickHouse numbers table function");
    assert_eq!(numbers.kind, Some(CompletionItemKind::CLASS));
    assert_eq!(numbers.insert_text.as_deref(), Some("numbers(${1:count})"));
}

#[test]
fn dbx_window_function_blocks_are_expression_scoped_and_product_aware() {
    let row_sql = "SELECT row_";
    let mut postgres_items = Vec::new();
    add_window_function_completions(
        row_sql,
        lsp_position_at_end(row_sql),
        "postgres",
        &mut postgres_items,
    );
    assert_eq!(
        postgres_items
            .iter()
            .find(|item| item.label == "ROW_NUMBER")
            .and_then(|item| item.insert_text.as_deref()),
        Some("ROW_NUMBER() OVER (PARTITION BY ${1:column} ORDER BY ${2:column})")
    );

    let lag_sql = "SELECT la";
    let mut lag_items = Vec::new();
    add_window_function_completions(
        lag_sql,
        lsp_position_at_end(lag_sql),
        "mysql",
        &mut lag_items,
    );
    assert_eq!(
        lag_items
            .iter()
            .find(|item| item.label == "LAG")
            .and_then(|item| item.insert_text.as_deref()),
        Some("LAG(${1:value}) OVER (PARTITION BY ${2:column} ORDER BY ${3:column})")
    );

    let relation_sql = "SELECT * FROM row_";
    let mut relation_items = Vec::new();
    add_window_function_completions(
        relation_sql,
        lsp_position_at_end(relation_sql),
        "postgres",
        &mut relation_items,
    );
    assert!(relation_items.is_empty());

    let mut clickhouse_items = Vec::new();
    add_window_function_completions(
        row_sql,
        lsp_position_at_end(row_sql),
        "clickhouse",
        &mut clickhouse_items,
    );
    assert!(clickhouse_items.is_empty());

    let mut mongo_items = Vec::new();
    add_window_function_completions(
        row_sql,
        lsp_position_at_end(row_sql),
        "mongodb-json",
        &mut mongo_items,
    );
    assert!(mongo_items.is_empty());
}

#[test]
fn live_routine_completion_takes_precedence_over_builtin_catalog() {
    let sql = "SELECT con";
    let live = CompletionItem {
        label: "CONCAT".to_string(),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some("Function: CONCAT(value text) -> text".to_string()),
        insert_text: Some("CONCAT()".to_string()),
        ..Default::default()
    };
    let mut items = vec![live.clone()];
    add_builtin_function_completions(sql, lsp_position_at_end(sql), "mysql", None, &mut items);
    assert_eq!(
        items.iter().filter(|item| item.label == "CONCAT").count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .find(|item| item.label == "CONCAT")
            .and_then(|item| item.detail.as_deref()),
        live.detail.as_deref()
    );
}

#[test]
fn mysql_long_tail_functions_emit_dbx_aligned_snippets() {
    for (sql, label, expected) in [
        (
            "SELECT json_ins",
            "JSON_INSERT",
            "JSON_INSERT(${1:json}, ${2:path}, ${3:value}, ${4:path_value_pairs})",
        ),
        (
            "SELECT make",
            "MAKEDATE",
            "MAKEDATE(${1:year}, ${2:day_of_year})",
        ),
        ("SELECT uuid_s", "UUID_SHORT", "UUID_SHORT()"),
    ] {
        let mut items = Vec::new();
        add_builtin_function_completions(sql, lsp_position_at_end(sql), "mysql", None, &mut items);
        let item = items
            .iter()
            .find(|item| item.label == label)
            .unwrap_or_else(|| panic!("missing MySQL completion {label}"));
        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(item.insert_text.as_deref(), Some(expected));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    let sql = "SELECT uuid_s";
    let mut postgres_items = Vec::new();
    add_builtin_function_completions(
        sql,
        lsp_position_at_end(sql),
        "postgres",
        None,
        &mut postgres_items,
    );
    assert!(!postgres_items.iter().any(|item| item.label == "UUID_SHORT"));
}

#[test]
fn oracle_system_value_completion_is_bare_and_expression_scoped() {
    let sql = "SELECT sys";
    let mut items = Vec::new();
    add_builtin_function_completions(sql, lsp_position_at_end(sql), "oracle", None, &mut items);
    let sysdate = items
        .iter()
        .find(|item| item.label == "SYSDATE")
        .expect("Oracle SYSDATE completion");
    assert_eq!(sysdate.kind, Some(CompletionItemKind::VALUE));
    assert_eq!(sysdate.insert_text.as_deref(), Some("SYSDATE"));
    assert_eq!(
        sysdate.insert_text_format,
        Some(InsertTextFormat::PLAIN_TEXT)
    );

    let relation_sql = "SELECT * FROM sys";
    let mut relation_items = Vec::new();
    add_builtin_function_completions(
        relation_sql,
        lsp_position_at_end(relation_sql),
        "oracle",
        None,
        &mut relation_items,
    );
    assert!(relation_items.is_empty());

    let mut postgres_items = Vec::new();
    add_builtin_function_completions(
        sql,
        lsp_position_at_end(sql),
        "postgres",
        None,
        &mut postgres_items,
    );
    assert!(!postgres_items.iter().any(|item| item.label == "SYSDATE"));
}

#[test]
fn oracle_relation_blocks_and_table_routine_context_align_dbx() {
    let relation_sql = "SELECT * FROM json_";
    let mut relation_items = Vec::new();
    add_oracle_table_function_completions(
        relation_sql,
        lsp_position_at_end(relation_sql),
        "oracle",
        None,
        &mut relation_items,
    );
    let json_table = relation_items
        .iter()
        .find(|item| item.label == "JSON_TABLE")
        .expect("Oracle JSON_TABLE relation block");
    assert_eq!(json_table.kind, Some(CompletionItemKind::CLASS));
    assert_eq!(
        json_table.insert_text.as_deref(),
        Some("JSON_TABLE(${1:expr}, ${2:path})")
    );

    let mut postgres_items = Vec::new();
    add_oracle_table_function_completions(
        relation_sql,
        lsp_position_at_end(relation_sql),
        "postgres",
        None,
        &mut postgres_items,
    );
    assert!(postgres_items.is_empty());

    let routine_sql = "SELECT * FROM TABLE(pkg.fe";
    let context = oracle_table_routine_context(routine_sql, lsp_position_at_end(routine_sql))
        .expect("Oracle TABLE routine context");
    assert_eq!(context.prefix, "fe");
    assert_eq!(context.qualifier.as_deref(), Some("pkg"));

    let mut schema = test_schema("APP", &[]);
    schema.functions = vec![
        Function {
            name: "fetch_rows".to_string(),
            routine_type: Some("function".to_string()),
            parameters: vec![FunctionParameter {
                name: "limit".to_string(),
                data_type: "NUMBER".to_string(),
                optional: false,
            }],
            return_type: "ROWSET".to_string(),
            description: Some("Fetch rows".to_string()),
        },
        Function {
            name: "refresh_rows".to_string(),
            routine_type: Some("procedure".to_string()),
            parameters: Vec::new(),
            return_type: "void".to_string(),
            description: None,
        },
    ];
    let mut routine_items = vec![CompletionItem {
        label: "orders".to_string(),
        kind: Some(CompletionItemKind::CLASS),
        ..Default::default()
    }];
    add_oracle_table_function_completions(
        routine_sql,
        lsp_position_at_end(routine_sql),
        "oceanbase-oracle",
        Some(&schema),
        &mut routine_items,
    );
    assert_eq!(routine_items.len(), 1);
    assert_eq!(routine_items[0].label, "fetch_rows");
    assert_eq!(
        routine_items[0].insert_text.as_deref(),
        Some("pkg.fetch_rows(${1:limit})")
    );
    apply_qualified_identifier_completion_edits(
        routine_sql,
        lsp_position_at_end(routine_sql),
        &mut routine_items,
    );
    let CompletionTextEdit::Edit(edit) = routine_items[0]
        .text_edit
        .as_ref()
        .expect("qualified routine replacement edit")
    else {
        panic!("expected a plain text edit");
    };
    assert_eq!(edit.new_text, "pkg.fetch_rows(${1:limit})");
}

#[test]
fn db2_registers_and_dameng_functions_preserve_product_syntax() {
    let sql = "SELECT sys";
    let mut db2_items = Vec::new();
    add_builtin_function_completions(sql, lsp_position_at_end(sql), "db2", None, &mut db2_items);
    let db2_sysdate = db2_items
        .iter()
        .find(|item| item.label == "SYSDATE")
        .expect("Db2 SYSDATE special register");
    assert_eq!(db2_sysdate.kind, Some(CompletionItemKind::VALUE));
    assert_eq!(db2_sysdate.insert_text.as_deref(), Some("SYSDATE"));

    let mut dameng_items = Vec::new();
    add_builtin_function_completions(
        sql,
        lsp_position_at_end(sql),
        "dameng",
        None,
        &mut dameng_items,
    );
    let dameng_sysdate = dameng_items
        .iter()
        .find(|item| item.label == "SYSDATE")
        .expect("Dameng SYSDATE function");
    assert_eq!(dameng_sysdate.kind, Some(CompletionItemKind::FUNCTION));
    assert_eq!(dameng_sysdate.insert_text.as_deref(), Some("SYSDATE()"));

    let add_sql = "SELECT add_";
    let mut add_items = Vec::new();
    add_builtin_function_completions(
        add_sql,
        lsp_position_at_end(add_sql),
        "db2",
        None,
        &mut add_items,
    );
    assert_eq!(
        add_items
            .iter()
            .find(|item| item.label == "ADD_DAYS")
            .and_then(|item| item.insert_text.as_deref()),
        Some("ADD_DAYS(${1:datetime}, ${2:days})")
    );
}

#[test]
fn manticore_catalog_and_cloudflare_d1_now_boundary_are_expression_scoped() {
    let poly_sql = "SELECT pol";
    let mut manticore_items = Vec::new();
    add_builtin_function_completions(
        poly_sql,
        lsp_position_at_end(poly_sql),
        "manticoresearch",
        None,
        &mut manticore_items,
    );
    let poly2d = manticore_items
        .iter()
        .find(|item| item.label == "POLY2D")
        .expect("Manticore POLY2D completion");
    assert_eq!(poly2d.insert_text.as_deref(), Some("POLY2D(${1:points})"));

    let now_sql = "SELECT now";
    let mut sqlite_items = Vec::new();
    add_builtin_function_completions(
        now_sql,
        lsp_position_at_end(now_sql),
        "sqlite",
        None,
        &mut sqlite_items,
    );
    assert!(sqlite_items
        .iter()
        .any(|item| { item.label == "NOW" && item.insert_text.as_deref() == Some("NOW()") }));

    let mut d1_items = Vec::new();
    add_builtin_function_completions(
        now_sql,
        lsp_position_at_end(now_sql),
        "cloudflare-d1",
        None,
        &mut d1_items,
    );
    assert!(!d1_items.iter().any(|item| item.label == "NOW"));
}
