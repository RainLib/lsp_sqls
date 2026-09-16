use super::*;

#[test]
fn insert_value_hints_parse_visible_statement_inside_dollar_quoted_routine_body() {
    let sql = concat!(
        "CREATE FUNCTION copy_orders() RETURNS void AS $oxide$\n",
        "BEGIN\n",
        "  INSERT INTO app.orders (id, total) SELECT source_id, total FROM staging;\n",
        "END\n",
        "$oxide$ LANGUAGE plpgsql;"
    );
    let insert = sql.find("INSERT INTO").unwrap();
    let statement_end = sql[insert..].find(';').unwrap() + insert;
    let hints = insert_value_hints(
        sql,
        range_for_offsets(sql, insert, statement_end),
        &completion_schema_with_columns(),
    );

    assert_eq!(
        hints
            .into_iter()
            .filter_map(|hint| match hint.label {
                InlayHintLabel::String(label) => Some(label),
                InlayHintLabel::LabelParts(_) => None,
            })
            .collect::<Vec<_>>(),
        vec!["id:", "total:"]
    );
}

#[test]
fn insert_value_hints_preserve_document_utf16_positions_from_a_bounded_window() {
    let prefix = "SELECT 'emoji \u{1F680}';\n".repeat(4_000);
    let statement = "INSERT INTO app.orders VALUES (1, 2, 3);";
    let sql = format!("{prefix}{statement}");
    let statement_start = prefix.len();
    let value_start = sql[statement_start..].find('1').unwrap() + statement_start;
    let hints = insert_value_hints(
        &sql,
        range_for_offsets(&sql, statement_start, sql.len()),
        &completion_schema_with_columns(),
    );

    assert_eq!(hints.len(), 3);
    assert_eq!(
        hints[0].position,
        range_for_offsets(&sql, value_start, value_start).start
    );
}
