use super::*;

#[tokio::test]
async fn array_and_nested_function_input_keeps_builtin_candidates() {
    for (dialect, sql, name) in [
        ("postgres", "SELECT ARRAY[coa<cursor>]", "COALESCE"),
        ("postgres", "SELECT ARRAY [concat(coa<cursor>)]", "COALESCE"),
        (
            "postgres",
            "SELECT u.name ILIKE ANY(ARRAY[con<cursor>]) FROM users u",
            "CONCAT",
        ),
        ("clickhouse", "SELECT [arrayM<cursor>]", "arrayMap"),
        ("hive", "SELECT array(con<cursor>)", "CONCAT"),
        ("mysql", "SELECT JSON_ARRAY(con<cursor>)", "CONCAT"),
    ] {
        let harness = configured(dialect, sql);
        let items = complete_harness(&harness).await;
        let item = items
            .iter()
            .find(|item| item.label.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("{dialect}: {sql}: {items:?}"));
        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert!(
            item.insert_text
                .as_deref()
                .is_some_and(|text| text.contains("${")),
            "{item:?}"
        );
    }
}

#[tokio::test]
async fn array_input_keeps_window_snippet_candidates() {
    let harness = configured("postgres", "SELECT ARRAY[row_n<cursor>]");
    let items = complete_harness(&harness).await;
    let item = items
        .iter()
        .find(|item| item.label == "ROW_NUMBER")
        .unwrap_or_else(|| panic!("{items:?}"));
    assert!(
        item.insert_text
            .as_deref()
            .is_some_and(|text| text.contains("OVER")),
        "{item:?}"
    );
}

#[tokio::test]
async fn signatures_inside_arrays_and_grouped_arguments_select_nearest_routine() {
    for (dialect, sql, name, parameter) in [
        (
            "postgres",
            "SELECT ARRAY[COALESCE('x,y', <cursor>'fallback')]",
            "COALESCE",
            1,
        ),
        (
            "postgres",
            "SELECT COALESCE((1+<cursor>2),3)",
            "COALESCE",
            0,
        ),
        (
            "postgres",
            "SELECT CONCAT(ARRAY[1,2], <cursor>'tail')",
            "CONCAT",
            1,
        ),
        ("clickhouse", "SELECT [toInt64(<cursor>'1')]", "toInt64", 0),
        (
            "hive",
            "SELECT array(concat('x', <cursor>'y'))",
            "concat",
            1,
        ),
    ] {
        let harness = configured(dialect, sql);
        let result = harness.service.inner().signature_help(serde_json::from_value(serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position})).unwrap()).await.unwrap().unwrap_or_else(|| panic!("{dialect}: {sql}: no signature"));
        assert!(
            result.signatures[0]
                .label
                .to_lowercase()
                .starts_with(&name.to_lowercase()),
            "{dialect}: {sql}: {result:?}"
        );
        assert_eq!(
            result.active_parameter,
            Some(parameter),
            "{dialect}: {sql}: {result:?}"
        );
    }
}

#[tokio::test]
async fn function_candidates_stay_out_of_array_strings_comments_and_bracket_names() {
    for (dialect, sql) in [
        ("postgres", "SELECT ARRAY['coa<cursor>']"),
        ("postgres", "SELECT ARRAY[$q$ coa<cursor> $q$]"),
        ("postgres", "SELECT ARRAY[/* coa<cursor> */1]"),
        ("sqlserver", "SELECT [coa<cursor>] FROM users"),
        ("sqlite", "SELECT [coa<cursor>] FROM users"),
        ("clickhouse", "SELECT ['it\\'s coa<cursor>']"),
        ("clickhouse", "SELECT $tag$ coa<cursor> $tag$"),
        ("clickhouse", "SELECT 1 // coa<cursor>"),
        ("clickhouse", "SELECT 1 #! coa<cursor>"),
        ("hive", "SELECT array(\"coa<cursor>\")"),
    ] {
        let harness = configured(dialect, sql);
        let items = complete_harness(&harness).await;
        assert!(
            items
                .iter()
                .all(|item| item.kind != Some(CompletionItemKind::FUNCTION)),
            "{dialect}: {sql}: {items:?}"
        );
    }
}

#[tokio::test]
async fn generated_function_edits_replace_suffix_and_preserve_existing_calls() {
    for marked in [
        "SELECT ARRAY[coa<cursor>lesce(NULL,1)]",
        "/* 😀 */ SELECT ARRAY[coa<cursor>lesce /* c */ (NULL,1)]",
        "SELECT 1\n\nSELECT ARRAY[coa<cursor>lesce(NULL,1)]",
    ] {
        let harness = configured("postgres", marked);
        let items = complete_harness(&harness).await;
        let item = items
            .iter()
            .find(|item| item.label.eq_ignore_ascii_case("COALESCE"))
            .unwrap_or_else(|| panic!("{marked}: {items:?}"));
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("{marked}: {item:?}");
        };
        assert_eq!(
            edit.range,
            Range {
                start: lsp_position_at_end(&sql[..offset - 3]),
                end: lsp_position_at_end(&sql[..offset + 5])
            },
            "{marked}"
        );
        assert_eq!(edit.new_text, "COALESCE");
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
    }
}

#[tokio::test]
async fn window_edits_preserve_existing_over_clause_and_new_snippets_keep_delimiters() {
    for (marked, name, existing) in [
        (
            "SELECT ARRAY[row_n<cursor>umber() OVER(ORDER BY u.id)] FROM users u",
            "ROW_NUMBER",
            true,
        ),
        (
            "SELECT ARRAY[row_n<cursor>umber] FROM users u",
            "ROW_NUMBER",
            false,
        ),
        ("SELECT ARRAY[coa<cursor>lesce]", "COALESCE", false),
    ] {
        let harness = configured("postgres", marked);
        let items = complete_harness(&harness).await;
        let item = items
            .iter()
            .find(|item| item.label.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("{marked}: {items:?}"));
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("{item:?}");
        };
        let start = marked
            .find(if name == "ROW_NUMBER" { "row_n" } else { "coa" })
            .unwrap();
        let sql = marked.replace("<cursor>", "");
        assert_eq!(
            edit.range,
            Range {
                start: lsp_position_at_end(&sql[..start]),
                end: lsp_position_at_end(&sql[..start + name.len()])
            }
        );
        if existing {
            assert_eq!(edit.new_text, name);
        } else {
            assert!(edit.new_text.contains("${"), "{item:?}");
        }
    }
}
