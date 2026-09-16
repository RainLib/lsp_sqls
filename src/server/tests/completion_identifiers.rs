use super::*;

#[test]
fn completion_includes_visible_relation_alias_as_a_first_class_item() {
    let sql = "SELECT us FROM app.users AS us WHERE us.id > 0";
    let position = Position::new(0, "SELECT us".len() as u32);
    let mut items = Vec::new();

    add_referenced_alias_completions(sql, position, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "us");
    assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
    assert_eq!(items[0].insert_text.as_deref(), Some("us"));
    assert_eq!(items[0].detail.as_deref(), Some("Table alias · app.users"));
}

#[test]
fn relation_alias_completion_preserves_quoted_alias_sql() {
    let sql = "SELECT ua FROM app.users AS \"User Alias\"";
    let position = Position::new(0, "SELECT ua".len() as u32);
    let mut items = Vec::new();

    add_referenced_alias_completions(sql, position, &mut items);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "User Alias");
    assert_eq!(items[0].insert_text.as_deref(), Some("\"User Alias\""));
}

#[test]
fn relation_alias_completion_keeps_quoted_collisions_and_fuzzy_matches() {
    let collision_sql = "SELECT u FROM app.users u JOIN app.audit AS \"U\" ON TRUE";
    let collision_position = Position::new(0, "SELECT u".len() as u32);
    let mut collision_items = Vec::new();

    add_referenced_alias_completions(collision_sql, collision_position, &mut collision_items);

    assert_eq!(collision_items.len(), 2);
    assert!(collision_items
        .iter()
        .any(|item| { item.label == "u" && item.insert_text.as_deref() == Some("u") }));
    assert!(collision_items
        .iter()
        .any(|item| { item.label == "U" && item.insert_text.as_deref() == Some("\"U\"") }));

    let fuzzy_sql = "SELECT cmo FROM app.customer_orders customerOrder";
    let fuzzy_position = Position::new(0, "SELECT cmo".len() as u32);
    let mut fuzzy_items = Vec::new();
    add_referenced_alias_completions(fuzzy_sql, fuzzy_position, &mut fuzzy_items);

    let alias = fuzzy_items
        .iter()
        .find(|item| item.label == "customerOrder")
        .expect("ordered-subsequence alias completion");
    assert_eq!(alias.insert_text.as_deref(), Some("customerOrder"));
    assert_eq!(alias.sort_text.as_deref(), Some("-2:alias:3:customerorder"));
}

#[test]
fn relation_alias_completion_stays_out_of_relation_targets_and_sql_noise() {
    for (sql, position) in [
        ("SELECT * FROM us", Position::new(0, 16)),
        ("SELECT '-- us' FROM app.users us", Position::new(0, 12)),
    ] {
        let mut items = Vec::new();
        add_referenced_alias_completions(sql, position, &mut items);
        assert!(
            items.is_empty(),
            "unexpected alias completion for {sql}: {items:?}"
        );
    }
}

#[test]
fn completion_after_qualified_identifier_replaces_full_identifier_path() {
    let sql = "SELECT * FROM public.us";
    let position = Position {
        line: 0,
        character: sql.len() as u32,
    };
    let mut items = vec![CompletionItem {
        label: "public.users".to_string(),
        insert_text: Some("public.users".to_string()),
        ..Default::default()
    }];

    apply_qualified_identifier_completion_edits(sql, position, &mut items);

    let Some(CompletionTextEdit::Edit(text_edit)) = &items[0].text_edit else {
        panic!("qualified completion should use a text edit");
    };
    assert_eq!(
        text_edit.range.start,
        Position {
            line: 0,
            character: "SELECT * FROM ".len() as u32,
        }
    );
    assert_eq!(text_edit.range.end, position);
    assert_eq!(text_edit.new_text, "public.users");
}

#[test]
fn qualified_completion_edit_preserves_function_call_insert_text() {
    let sql = "SELECT test_db.calc";
    let position = Position {
        line: 0,
        character: sql.len() as u32,
    };
    let mut items = vec![CompletionItem {
        label: "test_db.calculate_score".to_string(),
        insert_text: Some("test_db.calculate_score()".to_string()),
        ..Default::default()
    }];

    apply_qualified_identifier_completion_edits(sql, position, &mut items);

    let Some(CompletionTextEdit::Edit(text_edit)) = &items[0].text_edit else {
        panic!("qualified function completion should use a text edit");
    };
    assert_eq!(text_edit.new_text, "test_db.calculate_score()");
}

#[test]
fn qualified_completion_edit_does_not_replace_alias_member_columns() {
    let sql = "SELECT * FROM users u WHERE u.";
    let position = Position {
        line: 0,
        character: sql.len() as u32,
    };
    let mut items = vec![CompletionItem {
        label: "id".to_string(),
        insert_text: Some("id".to_string()),
        ..Default::default()
    }];

    apply_qualified_identifier_completion_edits(sql, position, &mut items);

    assert!(items[0].text_edit.is_none());
}
