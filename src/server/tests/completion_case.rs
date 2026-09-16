use super::*;

#[test]
fn completion_preferences_case_generated_functions_without_rewriting_live_metadata() {
    let mut items = vec![
        CompletionItem {
            label: "COALESCE".to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            sort_text: Some("1:builtin:coalesce".to_string()),
            filter_text: Some("COALESCE".to_string()),
            insert_text: Some("COALESCE(${1:value})".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "ROW_NUMBER".to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            sort_text: Some("1:window-function:row_number".to_string()),
            insert_text: Some(
                "ROW_NUMBER() OVER (PARTITION BY ${1:column} ORDER BY ${2:column})".to_string(),
            ),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range::default(),
                new_text: " ROW_NUMBER() OVER (PARTITION BY ${1:column} ORDER BY ${2:column})"
                    .to_string(),
            })),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "CURRENT_DATE".to_string(),
            kind: Some(CompletionItemKind::VALUE),
            sort_text: Some("1:builtin-value:current_date".to_string()),
            insert_text: Some("CURRENT_DATE".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "TABLE".to_string(),
            kind: Some(CompletionItemKind::CLASS),
            sort_text: Some("1:oracle-table-function:table".to_string()),
            insert_text: Some("TABLE(${1:function_call})".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "JSON_TABLE".to_string(),
            kind: Some(CompletionItemKind::CLASS),
            sort_text: Some("1:oracle-table-function:json_table".to_string()),
            insert_text: Some("JSON_TABLE(${1:expr}, ${2:path})".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "MixedCaseRoutine".to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("Function: MixedCaseRoutine()".to_string()),
            sort_text: Some("0:oracle-table-routine:mixedcaseroutine".to_string()),
            insert_text: Some("MixedCaseRoutine()".to_string()),
            ..CompletionItem::default()
        },
    ];

    apply_completion_preferences(
        "SELECT ",
        Position::new(0, 7),
        &mut items,
        &CompletionPreferences {
            keyword_case: KeywordCase::Lower,
            function_case: KeywordCase::Lower,
            table_alias: TableAliasStyle::None,
        },
    );

    assert_eq!(items[0].label, "coalesce");
    assert_eq!(
        items[0].insert_text.as_deref(),
        Some("coalesce(${1:value})")
    );
    assert_eq!(items[1].label, "row_number");
    assert_eq!(
        items[1].insert_text.as_deref(),
        Some("row_number() over (partition by ${1:column} order by ${2:column})")
    );
    let Some(CompletionTextEdit::Edit(edit)) = items[1].text_edit.as_ref() else {
        panic!("window completion should keep its text edit");
    };
    assert_eq!(
        edit.new_text,
        " row_number() over (partition by ${1:column} order by ${2:column})"
    );
    assert_eq!(items[2].label, "current_date");
    assert_eq!(items[3].label, "table");
    assert_eq!(
        items[3].insert_text.as_deref(),
        Some("table(${1:function_call})")
    );
    assert_eq!(items[4].label, "json_table");
    assert_eq!(items[5].label, "MixedCaseRoutine");
    assert_eq!(items[5].insert_text.as_deref(), Some("MixedCaseRoutine()"));
}

#[test]
fn completion_preferences_add_initial_alias_only_for_relation_context() {
    let preferences = CompletionPreferences {
        keyword_case: KeywordCase::Upper,
        function_case: KeywordCase::Preserve,
        table_alias: TableAliasStyle::Initials,
    };
    let mut from_items = vec![CompletionItem {
        label: "app.user_accounts".to_string(),
        kind: Some(CompletionItemKind::CLASS),
        insert_text: Some("app.user_accounts".to_string()),
        ..CompletionItem::default()
    }];
    apply_completion_preferences(
        "SELECT * FROM user",
        Position::new(0, 18),
        &mut from_items,
        &preferences,
    );
    assert_eq!(
        from_items[0].insert_text.as_deref(),
        Some("app.user_accounts ${1:ua}")
    );

    let mut select_items = from_items.clone();
    select_items[0].insert_text = Some("app.user_accounts".to_string());
    apply_completion_preferences(
        "SELECT user",
        Position::new(0, 11),
        &mut select_items,
        &preferences,
    );
    assert_eq!(
        select_items[0].insert_text.as_deref(),
        Some("app.user_accounts")
    );
    assert_eq!(
        table_alias_initials("audit.userAccounts"),
        Some("ua".to_string())
    );

    let mut table_function_items = vec![CompletionItem {
        label: "JSON_TABLE".to_string(),
        kind: Some(CompletionItemKind::CLASS),
        insert_text: Some("JSON_TABLE(${1:expr}, ${2:path})".to_string()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..CompletionItem::default()
    }];
    apply_completion_preferences(
        "SELECT * FROM json_",
        Position::new(0, 19),
        &mut table_function_items,
        &preferences,
    );
    assert_eq!(
        table_function_items[0].insert_text.as_deref(),
        Some("JSON_TABLE(${1:expr}, ${2:path}) ${3:jt}")
    );
}
