use super::*;
use crate::SqlParser;

fn code_count(sql: &str, dialect: &str, code: &str) -> usize {
    diagnostic_codes(&sql_inspection_diagnostics(sql, None, dialect))
        .iter()
        .filter(|v| v.as_str() == code)
        .count()
}
#[test]
fn inspection_regions_literals_bodies_and_unfinished_input_cannot_create_mutations() {
    for (dialect, sql) in [
        (
            "postgres",
            "SELECT '; UPDATE users SET id=1; DELETE FROM users;';",
        ),
        (
            "postgres",
            "SELECT $body$; UPDATE users SET id=1; DELETE FROM users;$body$;",
        ),
        (
            "mysql",
            r"SELECT 'it\'s; UPDATE users SET id=1; DELETE FROM users;';",
        ),
        (
            "sqlserver",
            "SELECT N'; UPDATE users SET id=1; DELETE FROM users;';",
        ),
        (
            "clickhouse",
            "SELECT $$; UPDATE users SET id=1; DELETE FROM users;$$;",
        ),
        (
            "hive",
            r"SELECT 'it\'s; UPDATE users SET id=1; DELETE FROM users;';",
        ),
        ("postgres", "SELECT 'unfinished; UPDATE users SET id=1;"),
    ] {
        let d = sql_inspection_diagnostics(sql, None, dialect);
        assert!(d.is_empty(), "{dialect}: {sql}: {d:?}");
    }
}
#[test]
fn inspection_regions_comment_semicolons_and_dialect_comments_do_not_create_code() {
    for (dialect, sql) in [
        (
            "postgres",
            "/* outside; /* inner */ UPDATE users SET id=1; */ SELECT 1;",
        ),
        (
            "sqlserver",
            "/* outside; /* inner */ UPDATE users SET id=1; */ SELECT 1;",
        ),
        ("mysql", "# comment; UPDATE users SET id=1;\nSELECT 1;"),
        (
            "clickhouse",
            "// comment; UPDATE users SET id=1;\nSELECT 1;",
        ),
    ] {
        assert_eq!(code_count(sql, dialect, "OXIDE001"), 0, "{dialect}: {sql}");
    }
}
#[test]
fn inspection_regions_literal_semicolon_does_not_hide_the_actual_where_clause() {
    for dialect in ["postgres", "mysql", "sqlserver", "clickhouse", "hive"] {
        let sql = "UPDATE users SET name='a;b' WHERE id=1;";
        assert_eq!(code_count(sql, dialect, "OXIDE001"), 0, "{dialect}");
    }
}
#[test]
fn inspection_regions_hidden_where_and_newline_mutations_keep_complete_utf16_ranges() {
    let sql = "/*😀; WHERE hidden */\r\nUPDATE\nusers SET name='值; WHERE hidden';";
    let d = sql_inspection_diagnostics(sql, None, "sqlserver");
    let item = d
        .iter()
        .find(|d| {
            d.code
                == Some(tower_lsp::lsp_types::NumberOrString::String(
                    "OXIDE001".into(),
                ))
        })
        .expect("real UPDATE");
    assert_eq!(item.range.start, Position::new(0, 0));
    assert_eq!(item.range.end, lsp_position_at_end(sql));
}
#[test]
fn inspection_regions_only_actual_comment_directives_suppress_their_statement() {
    for sql in [
        "UPDATE users SET name='NOINSPECTION OXIDE001';",
        "UPDATE users SET \"NOINSPECTION OXIDE001\"=1;",
    ] {
        assert_eq!(code_count(sql, "postgres", "OXIDE001"), 1, "{sql}");
    }
    let sql = "/* noInspection\tOXIDE001; reason */ UPDATE users SET id=1; DELETE FROM users;";
    assert_eq!(code_count(sql, "postgres", "OXIDE001"), 1);
}
#[test]
fn inspection_regions_dialect_operators_and_identifier_substrings_are_not_postgres_risks() {
    for (dialect, sql) in [
        ("mysql", "SELECT ilike_count FROM users;"),
        ("sqlserver", "SELECT hierarchyid::GetRoot();"),
        (
            "clickhouse",
            "SELECT name ILIKE '%oxide%', '42'::Int32 FROM users;",
        ),
    ] {
        assert_eq!(code_count(sql, dialect, "OXIDE005"), 0, "{dialect}: {sql}");
    }
    assert_eq!(
        code_count(
            "SELECT name ILIKE 'x', id::integer FROM users;",
            "mysql",
            "OXIDE005"
        ),
        2
    );
}
#[test]
fn inspection_regions_join_action_preserves_semicolons_inside_the_same_projection() {
    let sql = "/*😀*/ SELECT 'a;b',o.id FROM app.orders o JOIN app.customers c WHERE c.id>0;";
    let schema = completion_schema_with_columns();
    let d = sql_inspection_diagnostics(sql, Some(&schema), "postgres");
    let item = d
        .iter()
        .find(|d| {
            d.code
                == Some(tower_lsp::lsp_types::NumberOrString::String(
                    "OXIDE002".into(),
                ))
        })
        .expect("missing JOIN warning");
    let uri = Url::parse("oxide://inspection/action.sql").unwrap();
    let actions = add_join_condition_actions(sql, &uri, item, Some(&schema), "postgres");
    assert!(!actions.is_empty());
    let edit = &code_action_edits(&actions[0], &uri)[0];
    assert_eq!(
        position_to_byte_offset(sql, edit.range.start),
        sql.find(" WHERE").unwrap()
    );
}
#[test]
fn inspection_regions_many_real_warnings_share_coordinate_work() {
    use std::time::{Duration, Instant};
    let sql = (0..4097)
        .map(|_| "UPDATE users SET id=1; ")
        .collect::<String>();
    let started = Instant::now();
    let diagnostics = sql_inspection_diagnostics(&sql, None, "mysql");
    eprintln!("4097 warnings: {:?}", started.elapsed());
    assert_eq!(diagnostics.len(), 4097);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        diagnostics.last().unwrap().range.end,
        lsp_position_at_end(sql.trim_end())
    );
}

#[test]
fn inspection_regions_join_word_boundaries_and_natural_modifiers_do_not_warn() {
    for sql in [
        "SELECT rejoined FROM users;",
        "SELECT * FROM users NATURAL LEFT OUTER JOIN orders;",
        "SELECT * FROM users CROSS\nJOIN orders;",
    ] {
        assert_eq!(code_count(sql, "postgres", "OXIDE002"), 0, "{sql}");
    }
    assert_eq!(
        code_count(
            "SELECT * FROM users u LEFT JOIN orders o;",
            "postgres",
            "OXIDE002"
        ),
        1
    );
    assert_eq!(code_count("SELECT 1::integer;", "sqlserver", "OXIDE005"), 1);
}

#[test]
fn inspection_regions_multiple_comment_directives_do_not_leak_to_the_next_statement() {
    let sql="/* noinspection OXIDE003, OXIDE005 */ SELECT * FROM users WHERE name ILIKE 'x'; SELECT * FROM users WHERE name ILIKE 'y';";
    assert_eq!(code_count(sql, "mysql", "OXIDE003"), 1);
    assert_eq!(code_count(sql, "mysql", "OXIDE005"), 1);
    let sql="/* noinspection OXIDE003 noinspection OXIDE005 */ SELECT * FROM users WHERE name ILIKE 'x';";
    assert!(sql_inspection_diagnostics(sql, None, "mysql").is_empty());
}

#[test]
fn inspection_regions_comment_callback_skips_opaque_identifiers_and_literals() {
    let sql = "/*😀*/ SELECT [-- fake;], N'/*not comment*/',N'-- not comment'; -- real\r\n";
    let mut comments = Vec::new();
    let mask = SqlParser::mask_sql_noise_and_comments_for_dialect(sql, "sqlserver", &mut |range| {
        comments.push(range)
    });
    assert_eq!(
        mask,
        SqlParser::mask_sql_noise_for_dialect(sql, "sqlserver")
    );
    assert_eq!(comments.len(), 2);
    assert_eq!(&sql[comments[0].clone()], "/*😀*/");
    assert!(sql[comments[1].clone()].starts_with("-- real"));
}
