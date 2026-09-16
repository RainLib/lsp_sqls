use super::*;

#[test]
fn schema_match_score_requires_exact_table_matches() {
    let schema = test_schema("app", &["users", "orders"]);

    assert_eq!(
        calculate_schema_match_score(&["users".to_string()], &schema),
        10
    );
    assert_eq!(
        calculate_schema_match_score(&["users".to_string(), "orders".to_string()], &schema),
        24
    );
    assert_eq!(
        calculate_schema_match_score(&["app.users".to_string()], &schema),
        10
    );
    assert_eq!(
        calculate_schema_match_score(&["other.users".to_string()], &schema),
        0
    );
    assert_eq!(
        calculate_schema_match_score(&["user".to_string()], &schema),
        0
    );
    assert_eq!(
        calculate_schema_match_score(&["users_backup".to_string()], &schema),
        0
    );
}

#[test]
fn schema_inference_uses_unique_highest_score() {
    let manager = SchemaManager::new();
    let app_id = manager.register(test_schema("app", &["users", "orders"]));
    manager.register(test_schema("audit", &["users"]));

    assert_eq!(
        infer_schema_id_from_tables(&["users".to_string(), "orders".to_string()], &manager),
        Some(app_id)
    );
}

#[test]
fn schema_inference_ignores_cte_references_when_scoring_tables() {
    let manager = SchemaManager::new();
    let app_id = manager.register(test_schema("app", &["users"]));
    manager.register(test_schema("scratch", &["recent_users"]));
    let configured = DashMap::new();
    let inferred = DashMap::new();

    assert_eq!(
        schema_id_for_file(
            "file:///cte.sql",
            Some("WITH recent_users AS (SELECT * FROM app.users) SELECT * FROM recent_users"),
            &configured,
            &inferred,
            &manager,
        ),
        Some(app_id)
    );
}

#[test]
fn schema_inference_rejects_ambiguous_equal_scores() {
    let manager = SchemaManager::new();
    manager.register(test_schema("app", &["users"]));
    manager.register(test_schema("audit", &["users"]));

    assert_eq!(
        infer_schema_id_from_tables(&["users".to_string()], &manager),
        None
    );
}

#[test]
fn schema_id_for_file_prefers_configured_mapping_over_inference() {
    let manager = SchemaManager::new();
    let app_id = manager.register(test_schema("app", &["users"]));
    let audit_id = manager.register(test_schema("audit", &["events"]));
    let configured = DashMap::new();
    let inferred = DashMap::new();
    configured.insert("file:///query.sql".to_string(), app_id);
    inferred.insert("file:///query.sql".to_string(), audit_id);

    assert_eq!(
        schema_id_for_file(
            "file:///query.sql",
            Some("SELECT * FROM audit.events"),
            &configured,
            &inferred,
            &manager,
        ),
        Some(app_id)
    );
}

#[test]
fn inferred_schema_cache_can_be_invalidated_after_text_changes() {
    let manager = SchemaManager::new();
    let app_id = manager.register(test_schema("app", &["users"]));
    let audit_id = manager.register(test_schema("audit", &["events"]));
    let configured = DashMap::new();
    let inferred = DashMap::new();
    let uri = "file:///query.sql";

    assert_eq!(
        schema_id_for_file(
            uri,
            Some("SELECT * FROM app.users"),
            &configured,
            &inferred,
            &manager,
        ),
        Some(app_id)
    );
    assert_eq!(
        schema_id_for_file(
            uri,
            Some("SELECT * FROM audit.events"),
            &configured,
            &inferred,
            &manager,
        ),
        Some(app_id),
        "cached inference should be used until the document invalidates it"
    );

    inferred.remove(uri);

    assert_eq!(
        schema_id_for_file(
            uri,
            Some("SELECT * FROM audit.events"),
            &configured,
            &inferred,
            &manager,
        ),
        Some(audit_id)
    );
}

#[test]
fn schema_id_for_file_ignores_missing_configured_schema_ids() {
    let manager = SchemaManager::new();
    let app_id = manager.register(test_schema("app", &["users"]));
    let configured = DashMap::new();
    let inferred = DashMap::new();
    configured.insert("file:///query.sql".to_string(), SchemaId::new());

    assert_eq!(
        schema_id_for_file(
            "file:///query.sql",
            Some("SELECT * FROM app.users"),
            &configured,
            &inferred,
            &manager,
        ),
        Some(app_id)
    );
}

#[test]
fn schema_id_for_file_clears_missing_inferred_schema_ids() {
    let manager = SchemaManager::new();
    let app_id = manager.register(test_schema("app", &["users"]));
    let configured = DashMap::new();
    let inferred = DashMap::new();
    let uri = "file:///query.sql";
    inferred.insert(uri.to_string(), SchemaId::new());

    assert_eq!(
        schema_id_for_file(
            uri,
            Some("SELECT * FROM app.users"),
            &configured,
            &inferred,
            &manager,
        ),
        Some(app_id)
    );
    assert_eq!(inferred.get(uri).map(|entry| *entry.value()), Some(app_id));
}

#[test]
fn extracts_schema_qualifier_at_completion_position() {
    assert_eq!(
        schema_qualifier_at_position(
            "SELECT audit.calculate",
            tower_lsp::lsp_types::Position {
                line: 0,
                character: 22,
            },
        )
        .as_deref(),
        Some("audit")
    );
    assert_eq!(
        schema_qualifier_at_position(
            r#"SELECT "App Schema".calculate"#,
            tower_lsp::lsp_types::Position {
                line: 0,
                character: 29,
            },
        )
        .as_deref(),
        Some("App Schema")
    );
}

#[test]
fn finds_schema_by_qualified_identifier_prefix() {
    let manager = SchemaManager::new();
    manager.register(test_schema("app", &["users"]));
    let audit_id = manager.register(test_schema("audit", &["events"]));

    assert_eq!(
        find_schema_by_qualifier(&manager, "audit").map(|schema| schema.id),
        Some(audit_id)
    );
    assert!(find_schema_by_qualifier(&manager, "missing").is_none());
}

#[test]
fn finds_schema_by_table_reference_without_random_ambiguous_match() {
    let manager = SchemaManager::new();
    manager.register(test_schema("app", &["users"]));
    let audit_id = manager.register(test_schema("audit", &["users", "events"]));

    assert_eq!(
        find_schema_by_table_reference(&manager, "audit.users").map(|schema| schema.id),
        Some(audit_id)
    );
    assert!(find_schema_by_table_reference(&manager, "users").is_none());
}

#[test]
fn finds_catalog_qualified_schema_without_cross_catalog_leakage() {
    let manager = SchemaManager::new();
    let mut app = test_schema("dbo", &["users"]);
    app.catalog = Some("AppDb".to_string());
    let app_id = manager.register(app);
    let mut audit = test_schema("dbo", &["users"]);
    audit.catalog = Some("AuditDb".to_string());
    let audit_id = manager.register(audit);

    assert_eq!(
        find_schema_by_qualifier(&manager, "AppDb.dbo").map(|schema| schema.id),
        Some(app_id)
    );
    assert_eq!(
        find_schema_by_table_reference(&manager, "[ServerOne].[AuditDb].[dbo].[users]")
            .map(|schema| schema.id),
        Some(audit_id)
    );
    assert_eq!(
        find_schema_by_table_reference(&manager, "AppDb..users").map(|schema| schema.id),
        Some(app_id)
    );
    assert!(find_schema_by_qualifier(&manager, "dbo").is_none());
    assert!(find_schema_by_table_reference(&manager, "dbo.users").is_none());
}

#[test]
fn uses_alias_table_reference_to_select_schema_at_column_completion() {
    let manager = SchemaManager::new();
    manager.register(test_schema("app", &["users"]));
    let audit_id = manager.register(test_schema("audit", &["users"]));
    let sql = "SELECT '😀', u. FROM audit.users u";
    let before_cursor = "SELECT '😀', u.";

    assert_eq!(
        schema_for_table_column_at_position(
            &manager,
            sql,
            tower_lsp::lsp_types::Position {
                line: 0,
                character: before_cursor.encode_utf16().count() as u32,
            },
        )
        .map(|schema| schema.id),
        Some(audit_id)
    );
}

#[test]
fn uses_trailing_alias_member_to_select_schema_at_column_completion() {
    let manager = SchemaManager::new();
    manager.register(test_schema("app", &["users"]));
    let audit_id = manager.register(test_schema("audit", &["events"]));
    let sql = "SELECT * FROM audit.events e WHERE e.";

    assert_eq!(
        schema_for_table_column_at_position(
            &manager,
            sql,
            tower_lsp::lsp_types::Position {
                line: 0,
                character: sql.encode_utf16().count() as u32,
            },
        )
        .map(|schema| schema.id),
        Some(audit_id)
    );
}

#[test]
fn resolves_mixed_case_unquoted_aliases_for_column_schema_selection() {
    let manager = SchemaManager::new();
    manager.register(test_schema("app", &["users"]));
    let audit_id = manager.register(test_schema("audit", &["events"]));
    let sql = "SELECT * FROM audit.events EventAlias WHERE eventalias.";

    assert_eq!(
        schema_for_table_column_at_position(
            &manager,
            sql,
            tower_lsp::lsp_types::Position {
                line: 0,
                character: sql.encode_utf16().count() as u32,
            },
        )
        .map(|schema| schema.id),
        Some(audit_id)
    );
}
