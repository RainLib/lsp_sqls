use super::*;

#[test]
fn qualify_identifier_actions_use_the_unique_visible_source() {
    let sql = "SELECT total FROM app.orders o";
    let uri = Url::parse("file:///query.sql").unwrap();
    let end = "SELECT total".len();
    let actions = qualify_identifier_actions(
        sql,
        &uri,
        range_for_offsets(sql, end, end),
        Some(&completion_schema_with_columns()),
        "postgres",
    );

    assert_eq!(actions.len(), 1);
    assert_eq!(code_action_replacement(&actions[0], &uri), "o.total");
    assert_eq!(actions[0].is_preferred, Some(true));
}

#[test]
fn qualify_identifier_actions_remove_only_an_unambiguous_qualifier() {
    let uri = Url::parse("file:///query.sql").unwrap();
    let unique_sql = "SELECT o.total FROM app.orders o";
    let unique_end = "SELECT o.total".len();
    let actions = qualify_identifier_actions(
        unique_sql,
        &uri,
        range_for_offsets(unique_sql, unique_end, unique_end),
        Some(&completion_schema_with_columns()),
        "postgres",
    );
    assert_eq!(actions.len(), 1);
    assert_eq!(code_action_replacement(&actions[0], &uri), "total");

    let ambiguous_sql =
        "SELECT o.id FROM app.orders o JOIN app.customers c ON c.id = o.customer_id";
    let ambiguous_end = "SELECT o.id".len();
    assert!(qualify_identifier_actions(
        ambiguous_sql,
        &uri,
        range_for_offsets(ambiguous_sql, ambiguous_end, ambiguous_end),
        Some(&completion_schema_with_columns()),
        "postgres",
    )
    .is_empty());
}

#[test]
fn qualify_identifier_actions_offer_each_metadata_backed_ambiguous_source() {
    let sql = "SELECT id FROM app.orders o JOIN app.customers c ON c.id = o.customer_id";
    let uri = Url::parse("file:///query.sql").unwrap();
    let end = "SELECT id".len();
    let actions = qualify_identifier_actions(
        sql,
        &uri,
        range_for_offsets(sql, end, end),
        Some(&completion_schema_with_columns()),
        "postgres",
    );
    let replacements = actions
        .iter()
        .map(|action| code_action_replacement(action, &uri))
        .collect::<Vec<_>>();

    assert_eq!(replacements, vec!["c.id", "o.id"]);
    assert!(actions
        .iter()
        .all(|action| action.is_preferred == Some(false)));
}

#[test]
fn qualify_identifier_actions_preserve_quoted_aliases_and_columns() {
    let sql = "SELECT \"total\" FROM app.orders AS \"Order Alias\"";
    let uri = Url::parse("file:///query.sql").unwrap();
    let end = "SELECT \"total\"".len();
    let actions = qualify_identifier_actions(
        sql,
        &uri,
        range_for_offsets(sql, end, end),
        Some(&completion_schema_with_columns()),
        "postgres",
    );

    assert_eq!(actions.len(), 1);
    assert_eq!(
        code_action_replacement(&actions[0], &uri),
        "\"Order Alias\".\"total\""
    );

    let mysql_sql = "SELECT `total` FROM app.orders AS `Order Alias`";
    let mysql_end = "SELECT `total`".len();
    let mysql_actions = qualify_identifier_actions(
        mysql_sql,
        &uri,
        range_for_offsets(mysql_sql, mysql_end, mysql_end),
        Some(&completion_schema_with_columns()),
        "mysql",
    );
    assert_eq!(mysql_actions.len(), 1);
    assert_eq!(
        code_action_replacement(&mysql_actions[0], &uri),
        "`Order Alias`.`total`"
    );

    let mysql_qualified = "SELECT `o`.`total` FROM app.orders AS o";
    let mysql_qualified_end = "SELECT `o`.`total`".len();
    let mysql_unqualify = qualify_identifier_actions(
        mysql_qualified,
        &uri,
        range_for_offsets(mysql_qualified, mysql_qualified_end, mysql_qualified_end),
        Some(&completion_schema_with_columns()),
        "mysql",
    );
    assert_eq!(mysql_unqualify.len(), 1);
    assert_eq!(
        code_action_replacement(&mysql_unqualify[0], &uri),
        "`total`"
    );
}

#[test]
fn qualify_identifier_actions_keep_quoted_aliases_distinct_from_ordinary_aliases() {
    let uri = Url::parse("file:///query.sql").unwrap();
    let schema = completion_schema_with_columns();

    let collision_sql =
        "SELECT id FROM app.orders u JOIN app.customers AS \"U\" ON \"U\".id = u.customer_id";
    let collision_end = "SELECT id".len();
    let replacements = qualify_identifier_actions(
        collision_sql,
        &uri,
        range_for_offsets(collision_sql, collision_end, collision_end),
        Some(&schema),
        "postgres",
    )
    .iter()
    .map(|action| code_action_replacement(action, &uri))
    .collect::<Vec<_>>();
    assert_eq!(replacements, vec!["\"U\".id", "u.id"]);

    let wrong_quote_sql = "SELECT U.total FROM app.orders AS \"U\"";
    let wrong_quote_end = "SELECT U.total".len();
    assert!(qualify_identifier_actions(
        wrong_quote_sql,
        &uri,
        range_for_offsets(wrong_quote_sql, wrong_quote_end, wrong_quote_end),
        Some(&schema),
        "postgres",
    )
    .is_empty());

    let quoted_sql = "SELECT \"U\".total FROM app.orders AS \"U\"";
    let quoted_end = "SELECT \"U\".total".len();
    let actions = qualify_identifier_actions(
        quoted_sql,
        &uri,
        range_for_offsets(quoted_sql, quoted_end, quoted_end),
        Some(&schema),
        "postgres",
    );
    assert_eq!(actions.len(), 1);
    assert_eq!(code_action_replacement(&actions[0], &uri), "total");
}

#[test]
fn qualify_identifier_actions_ignore_alias_declarations_and_sql_noise() {
    let uri = Url::parse("file:///query.sql").unwrap();
    for (sql, end) in [
        (
            "SELECT total AS amount FROM app.orders o",
            "SELECT total AS amount".len(),
        ),
        ("SELECT 'total' FROM app.orders o", "SELECT 'total".len()),
        (
            "SELECT total -- amount\nFROM app.orders o",
            "SELECT total -- amount".len(),
        ),
    ] {
        assert!(
            qualify_identifier_actions(
                sql,
                &uri,
                range_for_offsets(sql, end, end),
                Some(&completion_schema_with_columns()),
                "postgres",
            )
            .is_empty(),
            "unexpected intention for {sql}"
        );
    }
}

#[test]
fn batch_qualify_identifier_action_edits_only_unique_metadata_columns() {
    let sql = "SELECT total, name, id, COUNT(id), 'total' FROM app.orders o JOIN app.customers c ON c.id = o.customer_id";
    let uri = Url::parse("file:///query.sql").unwrap();
    let start = "SELECT ".len();
    let end = sql.find(" FROM").unwrap();
    let actions = qualify_identifier_actions(
        sql,
        &uri,
        range_for_offsets(sql, start, end),
        Some(&completion_schema_with_columns()),
        "postgres",
    );

    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].title, "Qualify 2 selected columns");
    assert_eq!(
        code_action_edits(&actions[0], &uri)
            .into_iter()
            .map(|edit| edit.new_text)
            .collect::<Vec<_>>(),
        vec!["o.total", "c.name"]
    );
}

#[test]
fn qualify_identifier_actions_include_unaliased_cte_row_sources() {
    let mut schema = completion_schema_with_columns();
    schema.tables.push(Table {
        name: "recent".to_string(),
        columns: vec![Column {
            name: "total".to_string(),
            data_type: "bigint".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let sql = "WITH recent AS (SELECT total FROM app.orders) SELECT total FROM recent";
    let uri = Url::parse("file:///query.sql").unwrap();
    let end = sql.rfind("total").unwrap() + "total".len();
    let actions = qualify_identifier_actions(
        sql,
        &uri,
        range_for_offsets(sql, end, end),
        Some(&schema),
        "postgres",
    );

    assert_eq!(actions.len(), 1);
    assert_eq!(code_action_replacement(&actions[0], &uri), "recent.total");
}

#[test]
fn select_star_expansion_qualifies_columns_from_multiple_sources() {
    let sql = "SELECT * FROM app.orders o JOIN app.customers c ON c.id = o.customer_id";
    let uri = Url::parse("file:///query.sql").unwrap();
    let star = sql.find('*').unwrap();
    let action = expand_select_star_action(
        sql,
        &uri,
        range_for_offsets(sql, star, star + 1),
        Some(completion_schema_with_columns()),
        "postgres",
    )
    .expect("star expansion");
    let edit = &action.edit.unwrap().changes.unwrap()[&uri][0];
    assert_eq!(edit.new_text, "o.id, o.customer_id, o.total, c.id, c.name");
}

#[test]
fn qualified_star_expansion_preserves_the_typed_alias() {
    let sql = "SELECT o.* FROM app.orders AS o";
    let uri = Url::parse("file:///query.sql").unwrap();
    let star = sql.find('*').unwrap();
    let action = expand_select_star_action(
        sql,
        &uri,
        range_for_offsets(sql, star, star + 1),
        Some(completion_schema_with_columns()),
        "postgres",
    )
    .expect("qualified star expansion");
    let edit = &action.edit.unwrap().changes.unwrap()[&uri][0];
    assert_eq!(edit.new_text, "id, o.customer_id, o.total");
}
