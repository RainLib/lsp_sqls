use super::*;

#[test]
fn extracts_aliases_and_references_from_qualified_selects() {
    let sql = r#"
        SELECT u.id, o.total
        FROM "public"."users" u
        JOIN `shop`.`orders` AS o ON o.user_id = u.id
        WHERE EXISTS (
            SELECT 1 FROM audit.events e WHERE e.user_id = u.id
        )
    "#;
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    let aliases = parser.extract_aliases(tree, sql);
    assert_eq!(aliases.get("u"), Some(&"public.users".to_string()));
    assert_eq!(aliases.get("o"), Some(&"shop.orders".to_string()));
    assert_eq!(aliases.get("e"), Some(&"audit.events".to_string()));

    let references = parser.extract_referenced_tables(tree, sql);
    assert!(references.contains(&"public.users".to_string()));
    assert!(references.contains(&"shop.orders".to_string()));
    assert!(references.contains(&"audit.events".to_string()));
}

#[test]
fn extracts_unquoted_unicode_relation_references() {
    let sql = "SELECT o.金额 FROM 销售.订单 o WHERE o.状态 = '完成'";
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    let aliases = parser.extract_aliases(tree, sql);
    assert_eq!(aliases.get("o"), Some(&"销售.订单".to_string()));
    assert_eq!(
        parser.extract_referenced_tables(tree, sql),
        vec!["销售.订单".to_string()]
    );
}

#[test]
fn extracts_references_from_dml_and_ddl_statements() {
    let sql = r#"
        UPDATE app.users u SET name = 'x' WHERE u.id = 1;
        INSERT INTO app.orders (user_id) VALUES (1);
        DELETE FROM app.sessions s WHERE s.user_id = 1;
        CREATE TABLE IF NOT EXISTS app.invoices (id int);
        DROP VIEW IF EXISTS app.active_users;
    "#;
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    let aliases = parser.extract_aliases(tree, sql);
    assert_eq!(aliases.get("u"), Some(&"app.users".to_string()));
    assert_eq!(aliases.get("s"), Some(&"app.sessions".to_string()));

    let references = parser.extract_referenced_tables(tree, sql);
    assert!(references.contains(&"app.users".to_string()));
    assert!(references.contains(&"app.orders".to_string()));
    assert!(references.contains(&"app.sessions".to_string()));
    assert!(references.contains(&"app.invoices".to_string()));
    assert!(references.contains(&"app.active_users".to_string()));
}

#[test]
fn respects_keyword_boundaries_when_extracting_references() {
    let sql = "SELECT * FROM information_schema.tables";
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    let references = parser.extract_referenced_tables(tree, sql);
    assert_eq!(references, vec!["information_schema.tables".to_string()]);
}

#[test]
fn ignores_keywords_inside_comments_literals_and_quoted_identifiers() {
    let sql = r#"
        -- FROM hidden.comment_table c
        SELECT 'JOIN hidden.literal_table l', "FROM", `JOIN`
        FROM public.users u
        /* UPDATE hidden.block_table b */
        WHERE u.note = 'FROM hidden.note_table n'
    "#;
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    let aliases = parser.extract_aliases(tree, sql);
    assert_eq!(aliases.get("u"), Some(&"public.users".to_string()));
    assert!(!aliases.contains_key("c"));
    assert!(!aliases.contains_key("l"));
    assert!(!aliases.contains_key("b"));
    assert!(!aliases.contains_key("n"));

    let references = parser.extract_referenced_tables(tree, sql);
    assert_eq!(references, vec!["public.users".to_string()]);
}

#[test]
fn ignores_references_inside_dollar_quoted_bodies() {
    let sql = r#"
        CREATE FUNCTION app.hidden_lookup()
        RETURNS integer
        LANGUAGE SQL
        AS $body$ SELECT id FROM hidden.internal WHERE active = true; $body$;
        SELECT * FROM public.users;
    "#;
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    let references = parser.extract_referenced_tables(tree, sql);
    assert_eq!(references, vec!["public.users".to_string()]);
}

#[test]
fn accepts_bind_and_template_placeholders_without_syntax_noise() {
    let statements = [
        "SELECT * FROM users WHERE id = $1;",
        "SELECT * FROM users WHERE id = ?;",
        "SELECT * FROM users WHERE id = ?1;",
        "SELECT * FROM users WHERE id = @id;",
        "SELECT * FROM users WHERE id = :id;",
        "SELECT * FROM users WHERE id = {{id}};",
        "SELECT * FROM users WHERE id = ${id};",
        "SELECT * FROM users WHERE id = %(id)s;",
        "SELECT {{column}} FROM {{schema}}.users;",
        "SELECT * FROM {{table}};",
    ];

    for sql in statements {
        let mut parser = SqlParser::new();
        let result = parser.parse(sql);
        assert!(
            result.diagnostics.is_empty(),
            "placeholder SQL should not produce diagnostics for {sql:?}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn placeholder_qualified_relations_keep_static_table_inference() {
    let sql = "SELECT {{column}} FROM {{schema}}.users u WHERE u.id = :id";
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.expect("placeholder SQL should produce a tree");

    assert_eq!(parser.extract_referenced_tables(&tree, sql), vec!["users"]);
    assert_eq!(
        parser
            .extract_aliases(&tree, sql)
            .get("u")
            .map(String::as_str),
        Some("users")
    );
}

#[test]
fn placeholders_inside_protected_regions_are_not_treated_as_sql_templates() {
    let sql = r#"
        SELECT ':id', "{{column}}", payload ?| array['${value}'];
        -- {{comment}}
        /* %(comment)s */
        DO $body$ BEGIN RAISE NOTICE '@inside'; END $body$;
    "#;
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);

    assert!(
        result
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("{{")
                && !diagnostic.message.contains("${")
                && !diagnostic.message.contains("%(")),
        "protected placeholder-like text must not be rewritten: {:?}",
        result.diagnostics
    );
}

#[test]
fn ignores_cte_names_when_extracting_references() {
    let sql = r#"
        WITH RECURSIVE recent_orders AS MATERIALIZED (
            SELECT * FROM app.orders WHERE created_at > now() - interval '7 days'
        ),
        user_rollup(user_id, total) AS NOT MATERIALIZED (
            SELECT user_id, count(*) FROM recent_orders GROUP BY user_id
        )
        SELECT u.id, r.total
        FROM user_rollup r
        JOIN app.users u ON u.id = r.user_id
        JOIN public.recent_orders archived ON archived.user_id = u.id;
    "#;
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    let references = parser.extract_referenced_tables(tree, sql);
    assert!(references.contains(&"app.orders".to_string()));
    assert!(references.contains(&"app.users".to_string()));
    assert!(references.contains(&"public.recent_orders".to_string()));
    assert!(!references.contains(&"recent_orders".to_string()));
    assert!(!references.contains(&"user_rollup".to_string()));
}

#[test]
fn extracts_table_function_aliases_after_arguments_and_ordinality() {
    for (sql, alias) in [
        (
            "SELECT * FROM generate_series(1, 3) g(value) WHERE g.value > 1",
            "g",
        ),
        (
            "SELECT * FROM generate_series(1, 3) WITH ORDINALITY AS series(value, ord)",
            "series",
        ),
    ] {
        let mut parser = SqlParser::new();
        let result = parser.parse(sql);
        let tree = result.tree.as_ref().expect("SQL should parse");
        let aliases = parser.extract_aliases(tree, sql);

        assert_eq!(
            aliases.get(alias).map(String::as_str),
            Some("generate_series")
        );
        assert_eq!(
            parser.extract_referenced_tables(tree, sql),
            vec!["generate_series".to_string()]
        );
    }
}

#[test]
fn extracts_lateral_and_apply_row_sources_without_reserving_lateral_as_a_table_name() {
    let cases = [
        (
            "SELECT * FROM users u, LATERAL generate_series(1, 3) g(value), orders o WHERE o.id = u.id",
            vec!["users", "generate_series", "orders"],
            vec![("u", "users"), ("g", "generate_series"), ("o", "orders")],
        ),
        (
            "SELECT * FROM users u CROSS APPLY json_each(u.payload) j WHERE j.value IS NOT NULL",
            vec!["users", "json_each"],
            vec![("u", "users"), ("j", "json_each")],
        ),
        (
            "SELECT * FROM lateral l WHERE l.id = 1",
            vec!["lateral"],
            vec![("l", "lateral")],
        ),
    ];

    for (sql, expected_references, expected_aliases) in cases {
        let mut parser = SqlParser::new();
        let result = parser.parse(sql);
        let tree = result.tree.as_ref().expect("SQL should parse");
        let aliases = parser.extract_aliases(tree, sql);

        assert_eq!(
            parser.extract_referenced_tables(tree, sql),
            expected_references
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        );
        for (alias, reference) in expected_aliases {
            assert_eq!(aliases.get(alias).map(String::as_str), Some(reference));
        }
    }
}

#[test]
fn resolves_only_the_requested_outer_alias_inside_a_correlated_subquery() {
    let sql = "SELECT * FROM app.users outer_user WHERE EXISTS (SELECT 1 FROM app.orders inner_order WHERE outer_user.)";
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");
    let cursor_position = position_at_end(sql.trim_end_matches(')'));
    let aliases = parser.extract_aliases_at_position(tree, sql, cursor_position);

    assert_eq!(
        aliases.get("outer_user").map(String::as_str),
        Some("app.users")
    );
    assert_eq!(
        aliases.get("inner_order").map(String::as_str),
        Some("app.orders")
    );
    assert_eq!(
        parser.extract_referenced_tables_at_position(tree, sql, cursor_position),
        vec!["app.orders".to_string()]
    );
}

#[test]
fn correlated_alias_entries_preserve_quoted_outer_sql() {
    let sql = "SELECT * FROM app.users AS \"Outer User\" WHERE EXISTS (SELECT 1 FROM app.orders inner_order WHERE \"Outer User\".)";
    let cursor_position = position_at_end(sql.trim_end_matches(')'));
    let aliases = SqlParser::relation_aliases_at_position(sql, cursor_position);

    let outer = aliases
        .iter()
        .find(|alias| alias.name == "Outer User")
        .expect("quoted outer alias");
    assert_eq!(outer.sql, "\"Outer User\"");
    assert_eq!(outer.relation, "app.users");
    assert!(aliases
        .iter()
        .any(|alias| { alias.name == "inner_order" && alias.relation == "app.orders" }));
}

#[test]
fn resolves_unquoted_relation_aliases_case_insensitively() {
    let aliases = vec![RelationAlias {
        name: "UserAlias".to_string(),
        sql: "UserAlias".to_string(),
        relation: "app.users".to_string(),
    }];

    let resolved = SqlParser::resolve_relation_alias(&aliases, "useralias")
        .expect("unquoted alias should resolve case-insensitively");
    assert_eq!(resolved.relation, "app.users");
    assert_eq!(resolved.sql, "UserAlias");
    assert!(SqlParser::resolve_relation_alias(&aliases, "app.useralias").is_none());
}

#[test]
fn extracts_comma_separated_sources_after_join_expressions() {
    let sql = "SELECT * FROM users u JOIN orders o ON o.user_id = u.id, audit_log a, generate_series(1, 3) g(value) WHERE a.user_id = u.id";
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");
    let aliases = parser.extract_aliases(tree, sql);

    assert_eq!(aliases.get("a").map(String::as_str), Some("audit_log"));
    assert_eq!(
        aliases.get("g").map(String::as_str),
        Some("generate_series")
    );
    assert_eq!(
        parser.extract_referenced_tables(tree, sql),
        vec![
            "users".to_string(),
            "orders".to_string(),
            "audit_log".to_string(),
            "generate_series".to_string(),
        ]
    );
}

#[test]
fn extracts_on_relation_for_index_trigger_and_policy_ddl() {
    let sql = r#"
        CREATE INDEX users_email_idx ON app.users (email);
        DROP INDEX old_users_idx ON app.users;
        CREATE TRIGGER audit_users AFTER INSERT ON app.users
            FOR EACH ROW EXECUTE FUNCTION audit_user_change();
        CREATE POLICY users_tenant_policy ON app.users USING (tenant_id = current_setting('app.tenant_id')::int);
        SELECT * FROM app.users u JOIN app.orders o ON o.user_id = u.id;
    "#;
    let mut parser = SqlParser::new();
    let result = parser.parse(sql);
    let tree = result.tree.as_ref().expect("SQL should parse");

    let references = parser.extract_referenced_tables(tree, sql);
    assert!(references.contains(&"app.users".to_string()));
    assert!(references.contains(&"app.orders".to_string()));
    assert!(!references.contains(&"o.user_id".to_string()));
}

#[test]
fn column_qualifier_stops_before_other_expressions_and_queries() {
    for (marked, expected) in [
        ("SELECT to<cursor>tal FROM app.orders o WHERE o.id=1", None),
        ("SELECT to<cursor>tal + u.id FROM app.orders u", None),
        (r#"SELECT "field.with.dot<cursor>" FROM app.users"#, None),
        (
            "SELECT u.na<cursor>me FROM app.users u WHERE EXISTS(SELECT x.id FROM other.x)",
            Some("u"),
        ),
        (
            "SELECT app.users.na<cursor>me FROM app.users",
            Some("app.users"),
        ),
    ] {
        let at = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        let mut parser = SqlParser::new();
        let tree = parser.parse_syntax_tree(&sql).unwrap();
        let position = crate::position::byte_position_at_end(&sql[..at]);
        let node = parser.get_node_at_position(&tree, position).unwrap();
        assert_eq!(
            parser.get_table_name_for_column(node, &sql).as_deref(),
            expected,
            "{marked}"
        );
    }
}

#[test]
fn active_row_sources_include_derived_aliases_without_their_body_tables() {
    for marked in [
        "SELECT <cursor> FROM (SELECT id FROM hidden) d JOIN users u ON u.id=d.id",
        "SELECT <cursor> FROM users u, LATERAL (SELECT u.id FROM hidden h) AS d",
    ] {
        let at = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        let mut parser = SqlParser::new();
        let tree = parser.parse_syntax_tree(&sql).unwrap();
        let position = crate::position::byte_position_at_end(&sql[..at]);
        let sources = parser.extract_row_sources_at_position(&tree, &sql, position);
        assert!(
            sources.iter().any(|name| name == "d"),
            "{marked}: {sources:?}"
        );
        assert!(
            sources.iter().any(|name| name == "users"),
            "{marked}: {sources:?}"
        );
        assert!(
            !sources.iter().any(|name| name == "hidden"),
            "{marked}: {sources:?}"
        );
    }
}
