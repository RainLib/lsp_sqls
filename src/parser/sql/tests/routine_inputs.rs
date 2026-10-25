use super::*;

#[test]
fn vendor_array_constructors_keep_complete_trees_and_postgres_limits() {
    for sql in [
        "SELECT [arrayMap((x,y)->x+y, [1,2], [3,4])]",
        "SELECT array(CONCAT('x','y'))",
    ] {
        let result = SqlParser::new().parse(sql);
        assert!(
            result.diagnostics.is_empty(),
            "{sql}: {:?}",
            result.diagnostics
        );
        assert!(!result.tree.unwrap().root_node().has_error(), "{sql}");
    }
    let mut postgres = SqlParser::new_with_placeholder_dialect(
        crate::placeholder::SqlPlaceholderDialect::Postgres,
    );
    for sql in ["SELECT ARRAY[COALESCE(NULL,1)]", "SELECT ARRAY(SELECT 1)"] {
        assert!(postgres.parse(sql).diagnostics.is_empty(), "{sql}");
    }
    // Terminate invalid statements so the existing unfinished-EOF editor
    // policy does not defer diagnostics for the final expression.
    for sql in ["SELECT [1,2];", "SELECT ARRAY(1,2);"] {
        assert!(
            postgres
                .parse(sql)
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code
                    == Some(tower_lsp::lsp_types::NumberOrString::String(
                        "UNSUPPORTED_DIALECT_SYNTAX".into()
                    ))),
            "{sql}"
        );
    }
    assert!(!SqlParser::new()
        .parse("SELECT [1,,2];")
        .diagnostics
        .is_empty());
}

#[test]
fn named_masks_preserve_array_expressions_and_identifier_dialect_boundaries() {
    let sql = "/* 😀 */\r\nSELECT ARRAY[COALESCE('x,)', 1)]";
    for dialect in ["postgres", "clickhouse", "hive"] {
        let masked = SqlParser::mask_sql_noise_for_dialect(sql, dialect);
        assert_eq!(masked.len(), sql.len());
        assert!(
            masked.contains("ARRAY[COALESCE(     , 1)]"),
            "{dialect}: {masked}"
        );
        assert_eq!(masked.find("COALESCE"), sql.find("COALESCE"));
    }
    for dialect in ["sqlserver", "sqlite"] {
        let masked = SqlParser::mask_sql_noise_for_dialect("SELECT [COALESCE(,)]", dialect);
        assert!(!masked.contains("COALESCE"), "{dialect}: {masked}");
    }
    assert_eq!(
        SqlParser::identifier_path_token_for_sql_dialect("SELECT [toInt", "clickhouse"),
        Some("toInt")
    );
    assert_eq!(
        SqlParser::identifier_path_token_for_sql_dialect("SELECT [odd]]na", "sqlserver"),
        Some("[odd]]na")
    );
}

#[test]
fn named_masks_respect_postgres_escape_strings_and_standard_backslashes() {
    let ordinary = r"SELECT 'path\' || COALESCE(";
    for dialect in ["postgres", "sqlserver", "sqlite"] {
        assert!(
            SqlParser::mask_sql_noise_for_dialect(ordinary, dialect).contains("COALESCE"),
            "{dialect}"
        );
    }
    assert!(!SqlParser::mask_sql_noise_for_dialect(ordinary, "mysql").contains("COALESCE"));
    assert!(
        SqlParser::mask_sql_noise_for_dialect(r"SELECT E'it\'s' || COALESCE(", "postgres")
            .contains("COALESCE")
    );
    assert!(
        !SqlParser::mask_sql_noise_for_dialect(r"SELECT E'it\'s COALESCE(", "postgres")
            .contains("COALESCE")
    );
}

#[test]
fn named_masks_keep_nested_comments_heredocs_and_hash_operators_distinct() {
    for dialect in ["postgres", "sqlserver", "clickhouse"] {
        let masked = SqlParser::mask_sql_noise_for_dialect(
            "SELECT /* outer /* inner */ still ,( */ COALESCE(",
            dialect,
        );
        assert!(!masked.contains("still"), "{dialect}: {masked}");
        assert!(masked.ends_with("COALESCE("));
    }
    assert!(
        SqlParser::mask_sql_noise_for_dialect("SELECT 1 # 2, COALESCE(", "postgres")
            .contains("# 2, COALESCE(")
    );
    assert!(
        !SqlParser::mask_sql_noise_for_dialect("SELECT 1 # COALESCE(", "mysql")
            .contains("COALESCE")
    );
    for sql in [
        "SELECT 1 // COALESCE(",
        "SELECT 1 #! COALESCE(",
        "SELECT 1 # COALESCE(",
        "SELECT $tag$ COALESCE(,) $tag$",
    ] {
        assert!(
            !SqlParser::mask_sql_noise_for_dialect(sql, "clickhouse").contains("COALESCE"),
            "{sql}"
        );
    }
    let operand = SqlParser::mask_sql_operand_context_for_dialect(
        "/* outer /* inner */ still */",
        "postgres",
    );
    assert!(operand.trim().is_empty());
    assert!(
        !SqlParser::mask_sql_operand_context_for_dialect("'value'", "postgres")
            .trim()
            .is_empty()
    );
}
