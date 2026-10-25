use super::*;

#[test]
fn routine_context_counts_array_commas_and_keeps_nested_literal_arguments() {
    for (dialect, sql, name, parameter) in [
        ("postgres", "SELECT CONCAT(ARRAY[1,2], 'x,y', ", "CONCAT", 2),
        ("postgres", "SELECT ARRAY[COALESCE('x,)', ", "COALESCE", 1),
        (
            "clickhouse",
            "SELECT [arrayMap(x -> (x, x), [1,2], ",
            "arrayMap",
            2,
        ),
        ("hive", "SELECT array(concat('x,y', ", "concat", 1),
        (
            "sqlserver",
            "SELECT [dbo].[计算]('x,y', ",
            "[dbo].[计算]",
            1,
        ),
    ] {
        let call = routine_call_at_position(sql, lsp_position_at_end(sql), dialect)
            .unwrap_or_else(|| panic!("{dialect}: {sql}"));
        assert_eq!(call.name, name, "{sql}");
        assert_eq!(call.active_parameter, parameter, "{sql}");
    }
}

#[test]
fn routine_context_skips_trailing_comments_and_anonymous_groups() {
    for sql in [
        "SELECT COALESCE /* comment */ (",
        "SELECT COALESCE /* outer /* inner */ still */ (",
        "SELECT COALESCE -- comment\r\n(",
        "SELECT COALESCE((1+",
    ] {
        let call = routine_call_at_position(sql, lsp_position_at_end(sql), "postgres")
            .unwrap_or_else(|| panic!("{sql}"));
        assert_eq!(call.name, "COALESCE", "{sql}");
        assert_eq!(call.active_parameter, 0);
    }
}

#[test]
fn routine_context_preserves_parametric_groups_and_empty_commented_slots() {
    let sql = "SELECT [quantilesTDigest(0.5,0.9) /* c */ (value, ";
    let call = routine_call_at_position(sql, lsp_position_at_end(sql), "clickhouse").unwrap();
    assert_eq!(call.name, "quantilesTDigest");
    assert_eq!(call.active_group, 1);
    assert_eq!(call.active_parameter, 1);
    for dialect in ["postgres", "sqlserver", "clickhouse"] {
        let sql = "SELECT calculate(/* outer /* inner */ still */ ";
        let call = routine_call_at_position(sql, lsp_position_at_end(sql), dialect).unwrap();
        assert!(!call.current_argument_has_content, "{dialect}: {call:?}");
    }
}

#[test]
fn routine_context_keeps_bounded_windows_and_deep_grouping_work() {
    let sql = format!("SELECT COALESCE({}", "(".repeat(20_000));
    let started = std::time::Instant::now();
    let call = routine_call_at_position(&sql, lsp_position_at_end(&sql), "postgres").unwrap();
    assert_eq!(call.name, "COALESCE");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let oversized = format!("SELECT COALESCE({}", " ".repeat(70_000));
    assert!(
        routine_call_at_position(&oversized, lsp_position_at_end(&oversized), "postgres").is_none()
    );
}
