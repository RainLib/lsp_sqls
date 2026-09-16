use super::*;

#[test]
fn lsp_position_offsets_use_utf16_characters_and_byte_boundaries() {
    let text = "éabc\n😀z\r\ntail";

    assert_eq!(
        position_to_byte_offset(
            text,
            tower_lsp::lsp_types::Position {
                line: 0,
                character: 1,
            },
        ),
        "é".len()
    );
    assert_eq!(
        position_to_byte_offset(
            text,
            tower_lsp::lsp_types::Position {
                line: 1,
                character: 1,
            },
        ),
        "éabc\n".len(),
        "positions inside a surrogate pair should stay on the emoji boundary"
    );
    assert_eq!(
        position_to_byte_offset(
            text,
            tower_lsp::lsp_types::Position {
                line: 1,
                character: 2,
            },
        ),
        "éabc\n😀".len()
    );
    assert_eq!(
        position_to_byte_offset(
            text,
            tower_lsp::lsp_types::Position {
                line: 2,
                character: 4,
            },
        ),
        text.len()
    );
    assert_eq!(
        position_to_byte_offset(
            text,
            tower_lsp::lsp_types::Position {
                line: 99,
                character: 0,
            },
        ),
        text.len()
    );
}

#[test]
fn detects_completed_sql_context_keyword_without_consuming_trailing_space() {
    let from_sql = "SELECT * from";
    assert_eq!(
        completed_sql_context_keyword_at_position(
            from_sql,
            Position {
                line: 0,
                character: from_sql.len() as u32,
            },
        )
        .as_deref(),
        Some("from")
    );

    let where_space_sql = "SELECT * FROM users where ";
    assert_eq!(
        completed_sql_context_keyword_at_position(
            where_space_sql,
            Position {
                line: 0,
                character: where_space_sql.len() as u32,
            },
        ),
        None
    );
}

#[test]
fn completion_after_completed_keyword_uses_zero_width_text_edit() {
    let sql = "SELECT * from";
    let position = Position {
        line: 0,
        character: sql.len() as u32,
    };
    let mut items = vec![CompletionItem {
        label: "public.users".to_string(),
        kind: Some(CompletionItemKind::CLASS),
        insert_text: Some("public.users".to_string()),
        ..Default::default()
    }];

    apply_completed_sql_context_completion_edits(sql, position, &mut items);

    assert_eq!(items[0].filter_text.as_deref(), Some("from"));
    let Some(CompletionTextEdit::Edit(text_edit)) = &items[0].text_edit else {
        panic!("completion should use a plain text edit");
    };
    assert_eq!(text_edit.range.start, position);
    assert_eq!(text_edit.range.end, position);
    assert_eq!(text_edit.new_text, " public.users");
}

#[test]
fn completion_preferences_rewrite_keyword_case_and_text_edit() {
    let mut items = vec![CompletionItem {
        label: "SELECT".to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        insert_text: Some("SELECT".to_string()),
        ..CompletionItem::default()
    }];
    apply_completed_sql_context_completion_edits("WHERE", Position::new(0, 5), &mut items);
    apply_completion_preferences(
        "WHERE",
        Position::new(0, 5),
        &mut items,
        &CompletionPreferences {
            keyword_case: KeywordCase::Lower,
            function_case: KeywordCase::Preserve,
            table_alias: TableAliasStyle::None,
        },
    );

    assert_eq!(items[0].label, "select");
    assert_eq!(items[0].insert_text.as_deref(), Some("select"));
    let Some(CompletionTextEdit::Edit(edit)) = items[0].text_edit.as_ref() else {
        panic!("keyword completion should keep its text edit");
    };
    assert_eq!(edit.new_text, " select");
}

#[test]
fn completion_deduplicates_keywords_without_collapsing_overloads() {
    let mut items = vec![
        CompletionItem {
            label: "TRUE".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("Boolean value for enabled".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "true".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("PostgreSQL keyword".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "calculate".to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("calculate(integer)".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "calculate".to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("calculate(text)".to_string()),
            ..CompletionItem::default()
        },
    ];

    deduplicate_simple_completion_items(&mut items);

    assert_eq!(
        items
            .iter()
            .filter(|item| item.kind == Some(CompletionItemKind::KEYWORD))
            .count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item.kind == Some(CompletionItemKind::FUNCTION))
            .count(),
        2
    );
    assert_eq!(
        items[0].detail.as_deref(),
        Some("Boolean value for enabled")
    );
}

#[test]
fn completion_ranks_high_frequency_keywords_within_their_semantic_group() {
    let mut items = vec![
        CompletionItem {
            label: "WHEN".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            sort_text: Some("0:when".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "where".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            sort_text: Some("0where".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "WHERE".to_string(),
            kind: Some(CompletionItemKind::FIELD),
            sort_text: Some("2:where".to_string()),
            ..CompletionItem::default()
        },
    ];

    apply_high_frequency_keyword_ranking(&mut items);

    assert_eq!(items[0].sort_text.as_deref(), Some("0:b:when"));
    assert_eq!(items[1].sort_text.as_deref(), Some("0:a:where"));
    assert!(items[1].sort_text < items[0].sort_text);
    assert_eq!(items[2].sort_text.as_deref(), Some("2:where"));
}

#[test]
fn completion_frequency_rank_does_not_cross_semantic_groups() {
    let mut items = vec![
        CompletionItem {
            label: "WHERE".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            sort_text: Some("2:where".to_string()),
            ..CompletionItem::default()
        },
        CompletionItem {
            label: "WHEN".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            sort_text: Some("1:when".to_string()),
            ..CompletionItem::default()
        },
    ];

    apply_high_frequency_keyword_ranking(&mut items);

    assert!(items[0].sort_text > items[1].sort_text);
}

#[test]
fn completion_after_from_keyword_drops_non_relation_items() {
    let sql = "SELECT * from";
    let position = Position {
        line: 0,
        character: sql.len() as u32,
    };
    let mut items = vec![
        CompletionItem {
            label: "public.webhook".to_string(),
            kind: Some(CompletionItemKind::CLASS),
            insert_text: Some("public.webhook".to_string()),
            ..Default::default()
        },
        CompletionItem {
            label: "form_background_url".to_string(),
            kind: Some(CompletionItemKind::FIELD),
            insert_text: Some("form_background_url".to_string()),
            ..Default::default()
        },
        CompletionItem {
            label: "FROM".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            insert_text: Some("FROM".to_string()),
            ..Default::default()
        },
    ];

    apply_completed_sql_context_completion_edits(sql, position, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "public.webhook");
    assert_eq!(items[0].filter_text.as_deref(), Some("from"));
}

#[test]
fn completion_after_where_keyword_drops_relation_and_operator_items() {
    let sql = "SELECT * FROM public.webhook where";
    let position = Position {
        line: 0,
        character: sql.len() as u32,
    };
    let mut items = vec![
        CompletionItem {
            label: "owner".to_string(),
            kind: Some(CompletionItemKind::FIELD),
            insert_text: Some("owner".to_string()),
            ..Default::default()
        },
        CompletionItem {
            label: "public.form".to_string(),
            kind: Some(CompletionItemKind::CLASS),
            insert_text: Some("public.form".to_string()),
            ..Default::default()
        },
        CompletionItem {
            label: "LIKE".to_string(),
            kind: Some(CompletionItemKind::OPERATOR),
            insert_text: Some("LIKE".to_string()),
            ..Default::default()
        },
    ];

    apply_completed_sql_context_completion_edits(sql, position, &mut items);

    assert_eq!(
        items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["owner"]
    );
    assert!(items
        .iter()
        .all(|item| item.filter_text.as_deref() == Some("where")));
}

#[test]
fn completion_after_ddl_on_keyword_keeps_relation_items() {
    let sql = "CREATE INDEX webhook_owner_idx ON";
    let position = Position {
        line: 0,
        character: sql.len() as u32,
    };
    let mut items = vec![
        CompletionItem {
            label: "public.webhook".to_string(),
            kind: Some(CompletionItemKind::CLASS),
            insert_text: Some("public.webhook".to_string()),
            ..Default::default()
        },
        CompletionItem {
            label: "owner".to_string(),
            kind: Some(CompletionItemKind::FIELD),
            insert_text: Some("owner".to_string()),
            ..Default::default()
        },
    ];

    apply_completed_sql_context_completion_edits(sql, position, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "public.webhook");
    assert_eq!(items[0].filter_text.as_deref(), Some("on"));
}

#[test]
fn completion_after_keyword_with_existing_space_keeps_original_insert_text() {
    let sql = "SELECT * FROM users where ";
    let position = Position {
        line: 0,
        character: sql.len() as u32,
    };
    let mut items = vec![CompletionItem {
        label: "id".to_string(),
        filter_text: Some("id".to_string()),
        insert_text: Some("id".to_string()),
        ..Default::default()
    }];

    apply_completed_sql_context_completion_edits(sql, position, &mut items);

    assert_eq!(items[0].filter_text.as_deref(), Some("id"));
    assert!(items[0].text_edit.is_none());
    assert_eq!(items[0].insert_text.as_deref(), Some("id"));
}

#[test]
fn completion_recovers_the_second_top_level_query_without_a_semicolon() {
    let sql = "SELECT *\nFROM first_table\n\nSELECT ";
    let position = crate::position::lsp_position_at_end(sql);
    let (scope, local_position) =
        completion_statement_prefix(sql, position).expect("second query should be isolated");

    assert_eq!(scope, "SELECT ");
    assert_eq!(local_position, crate::position::lsp_position_at_end(scope));
}

#[test]
fn completion_keeps_wrapped_and_nested_queries_in_one_scope() {
    for sql in [
        "WITH recent AS (\n  SELECT * FROM events\n)\n\nSELECT ",
        "INSERT INTO archive (id)\n\nSELECT ",
        "EXPLAIN\n\nSELECT ",
        "SELECT * FROM (\n\n  SELECT ",
        "SELECT 1\nUNION ALL\n\nSELECT ",
    ] {
        assert!(
            completion_statement_prefix(sql, crate::position::lsp_position_at_end(sql)).is_none(),
            "{sql:?} must remain one completion scope"
        );
    }
}

#[test]
fn completion_ignores_statement_keywords_inside_sql_noise() {
    let sql =
        "SELECT '-- not a boundary'\n\n-- SELECT ignored\n/* SELECT ignored too */\nFROM logs";

    assert!(completion_statement_prefix(sql, crate::position::lsp_position_at_end(sql)).is_none());
}
