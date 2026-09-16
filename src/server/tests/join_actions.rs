use super::*;

#[test]
fn missing_join_condition_quick_fix_uses_unique_foreign_key_metadata() {
    let mut schema = completion_schema_with_columns();
    schema.tables[0].constraints.push(Constraint {
        name: "orders_customer_fk".to_string(),
        constraint_type: "FOREIGN KEY".to_string(),
        columns: vec!["customer_id".to_string()],
        referenced_schema: Some("app".to_string()),
        referenced_table: Some("customers".to_string()),
        referenced_columns: vec!["id".to_string()],
        definition: None,
    });
    let sql = "SELECT o.id FROM app.orders o JOIN app.customers c WHERE c.name <> '';";
    let diagnostic = sql_inspection_diagnostics(sql, Some(&schema), "postgres")
            .into_iter()
            .find(|diagnostic| {
                diagnostic.code.as_ref().is_some_and(|code| {
                    matches!(code, tower_lsp::lsp_types::NumberOrString::String(code) if code == "OXIDE002")
                })
            })
            .expect("missing JOIN diagnostic");
    let uri = Url::parse("file:///query.sql").unwrap();

    let actions = add_join_condition_actions(sql, &uri, &diagnostic, Some(&schema), "postgres");

    assert_eq!(actions.len(), 2);
    assert_eq!(
        actions[0].title,
        "Add JOIN condition via orders_customer_fk"
    );
    assert_eq!(actions[0].is_preferred, Some(true));
    assert_eq!(
        code_action_replacement(&actions[0], &uri),
        " ON o.customer_id = c.id"
    );
    let edit = &code_action_edits(&actions[0], &uri)[0];
    assert_eq!(
        position_to_byte_offset(sql, edit.range.start),
        sql.find(" WHERE").unwrap()
    );
    assert_eq!(actions[1].title, "Add non-matching JOIN safety guard");
}

#[test]
fn missing_join_condition_quick_fix_keeps_ambiguous_foreign_keys_as_choices() {
    let mut schema = completion_schema_with_columns();
    schema.tables[0].constraints.extend([
        Constraint {
            name: "orders_customer_fk".to_string(),
            constraint_type: "FOREIGN KEY".to_string(),
            columns: vec!["customer_id".to_string()],
            referenced_table: Some("customers".to_string()),
            referenced_columns: vec!["id".to_string()],
            ..Default::default()
        },
        Constraint {
            name: "orders_owner_fk".to_string(),
            constraint_type: "FOREIGN KEY".to_string(),
            columns: vec!["id".to_string()],
            referenced_table: Some("customers".to_string()),
            referenced_columns: vec!["id".to_string()],
            ..Default::default()
        },
    ]);
    let sql = "SELECT * FROM app.orders o JOIN app.customers c;";
    let diagnostic = sql_inspection_diagnostics(sql, Some(&schema), "mysql")
        .into_iter()
        .find(|diagnostic| diagnostic.message.starts_with("JOIN has no"))
        .expect("missing JOIN diagnostic");
    let uri = Url::parse("file:///query.sql").unwrap();

    let actions = add_join_condition_actions(sql, &uri, &diagnostic, Some(&schema), "mysql");

    assert_eq!(actions.len(), 3);
    assert!(actions[..2]
        .iter()
        .all(|action| action.is_preferred == Some(false)));
    assert_eq!(
        position_to_byte_offset(sql, code_action_edits(&actions[0], &uri)[0].range.start),
        sql.len() - 1,
    );
}

#[test]
fn missing_join_condition_quick_fix_falls_back_without_proven_metadata() {
    let schema = completion_schema_with_columns();
    let sql = "SELECT * FROM app.orders o JOIN app.customers c";
    let diagnostic = sql_inspection_diagnostics(sql, Some(&schema), "postgres")
        .into_iter()
        .find(|diagnostic| diagnostic.message.starts_with("JOIN has no"))
        .expect("missing JOIN diagnostic");
    let uri = Url::parse("file:///query.sql").unwrap();

    let actions = add_join_condition_actions(sql, &uri, &diagnostic, Some(&schema), "postgres");

    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].title, "Add non-matching JOIN safety guard");
    assert_eq!(
        code_action_replacement(&actions[0], &uri),
        " ON 1 = 0 /* replace join condition before executing */"
    );
}
