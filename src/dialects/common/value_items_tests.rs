use super::{
    predicate_column_before_value, sql_type_value_template_kind, Position, SqlValueTemplateKind,
};

#[test]
fn postgres_catalog_integer_aliases_have_numeric_value_and_truth_policies() {
    for ty in [
        "int2",
        "INT4",
        "pg_catalog.int4",
        "\"pg_catalog\" . \"int8\"",
        "pg_catalog.numeric(12,-2)",
    ] {
        assert_eq!(
            sql_type_value_template_kind(ty, "postgres"),
            Some(SqlValueTemplateKind::Numeric),
            "{ty}"
        );
        assert_eq!(
            super::type_supports_truth_predicates(ty, "postgres"),
            Some(false),
            "{ty}"
        );
    }
}

#[test]
fn postgres_qualified_boolean_and_array_identities_stay_distinct() {
    for ty in ["pg_catalog.bool", "\"pg_catalog\".\"bool\"", "\"bool\""] {
        assert_eq!(
            super::type_supports_truth_predicates(ty, "postgres"),
            Some(true),
            "{ty}"
        );
    }
    for ty in [
        "pg_catalog._bool",
        "pg_catalog.bool [2]",
        "\"pg_catalog\".\"_int4\"",
        "pg_catalog.inet",
        "pg_catalog.bytea",
    ] {
        assert_eq!(
            super::type_supports_truth_predicates(ty, "postgres"),
            Some(false),
            "{ty}"
        );
    }
}

#[test]
fn postgres_type_identity_does_not_cross_schema_case_or_profile_boundaries() {
    for ty in [
        "public.bool",
        "\"PG_CATALOG\".bool",
        "pg_catalog.\"BOOL\"",
        "\"pg_catalog.bool\"",
        "public.int4",
        "pg_catalog.int4 trailing",
    ] {
        assert_eq!(sql_type_value_template_kind(ty, "postgres"), None, "{ty}");
    }
    for profile in [
        "mysql",
        "sqlserver",
        "sqlite",
        "duckdb",
        "clickhouse",
        "hive",
    ] {
        assert_eq!(
            sql_type_value_template_kind("pg_catalog.bool", profile),
            None,
            "{profile}"
        );
    }
}

#[test]
fn dense_trailing_comments_preserve_the_value_reference_and_report_scan_cost() {
    for count in [500usize, 2000, 8000] {
        let sql = format!(
            "SELECT * FROM jobs j WHERE j.\"字段\" {} = ",
            "/* 😀 BETWEEN AND */\n".repeat(count)
        );
        let started = std::time::Instant::now();
        let reference =
            predicate_column_before_value(&sql, Position::new(count as u32, 3), "", "postgres")
                .unwrap();
        assert_eq!(reference.column, "字段");
        assert_eq!(reference.qualifier.as_deref(), Some("j"));
        println!(
            "trailing-comments count={count} bytes={} elapsed_us={}",
            sql.len(),
            started.elapsed().as_micros()
        );
    }
}

#[test]
fn huge_declared_bit_widths_keep_completion_payloads_small() {
    use super::add_typed_value_template_items;
    for width in [65, 1_000_000, u32::MAX] {
        let mut items = Vec::new();
        add_typed_value_template_items(
            &mut items,
            "flags",
            SqlValueTemplateKind::BinaryBits(Some(width)),
            "",
        );
        assert_eq!(items.len(), 2);
        assert!(items
            .iter()
            .all(|item| item.label.len() < 64 && item.insert_text.as_ref().unwrap().len() < 64));
        assert!(items[1].label.contains(&format!("bit({width})")));
    }
}

#[test]
fn value_context_budget_limits_do_not_publish_guessed_types() {
    let sql = format!("{}SELECT * FROM jobs WHERE flags = ", " ".repeat(524289));
    assert_eq!(
        predicate_column_before_value(&sql, position(&sql), "", "postgres"),
        None
    );
    let sql = format!(
        "SELECT * FROM jobs WHERE flags BETWEEN {}1 ",
        "CASE WHEN 1=1 THEN ".repeat(130)
    );
    assert_eq!(
        predicate_column_before_value(&sql, position(&sql), "", "postgres"),
        None
    );
}

#[test]
fn value_policy_preserves_fields_and_domain_values_named_like_keywords() {
    use super::{apply_value_completion_policy, CompletionItem, CompletionItemKind};
    let mut items = vec![
        CompletionItem {
            label: "TRUE".into(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        },
        CompletionItem {
            label: "TRUE".into(),
            kind: Some(CompletionItemKind::FIELD),
            ..Default::default()
        },
        CompletionItem {
            label: "FALSE".into(),
            kind: Some(CompletionItemKind::ENUM_MEMBER),
            ..Default::default()
        },
    ];
    apply_value_completion_policy(&mut items, None, "sqlserver");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].kind, Some(CompletionItemKind::FIELD));
    assert_eq!(items[1].kind, Some(CompletionItemKind::ENUM_MEMBER));
}

#[test]
fn between_case_expressions_keep_the_outer_value_reference() {
    for sql in [
        "SELECT * FROM jobs WHERE attempts BETWEEN CASE WHEN enabled THEN 1 ELSE 2 END AND ",
        "SELECT * FROM jobs WHERE attempts BETWEEN CASE WHEN enabled THEN CASE WHEN x THEN 1 ELSE 2 END ELSE 3 END AND ",
        "SELECT * FROM jobs WHERE attempts BETWEEN COALESCE(CASE WHEN enabled THEN 1 END, 0) AND ",
    ] {
        assert_eq!(predicate_column_before_value(sql, position(sql), "", "postgres").map(|v|v.column),Some("attempts".into()),"{sql}");
    }
}

#[test]
fn value_reference_preserves_escaped_qualifiers_and_ignores_comment_noise() {
    for (sql, profile, column, qualifier) in [
        (
            "SELECT * FROM jobs j WHERE j.\"odd\"\"名\" /* = IN */ = ",
            "postgres",
            "odd\"名",
            "j",
        ),
        (
            "SELECT * FROM jobs [odd.alias] WHERE [odd.alias].[字段]]名] NOT IN (1, ",
            "sqlserver",
            "字段]名",
            "\"odd.alias\"",
        ),
        (
            "SELECT * FROM jobs j WHERE j.attempts BETWEEN 1 /* AND 0 */ AND ",
            "mysql",
            "attempts",
            "j",
        ),
    ] {
        let value = predicate_column_before_value(sql, position(sql), "", profile).unwrap();
        assert_eq!(value.column, column);
        assert_eq!(value.qualifier.as_deref(), Some(qualifier));
    }
}

#[test]
fn bit_widths_are_bounded_and_profile_specific() {
    for invalid in ["bit(0)", "bit(4294967296)", "bit(-1)", "bit(abc)"] {
        assert_eq!(sql_type_value_template_kind(invalid, "postgres"), None);
    }
    assert_eq!(
        sql_type_value_template_kind("BIT(64)", "mysql"),
        Some(SqlValueTemplateKind::BinaryBits(Some(64)))
    );
    assert_eq!(sql_type_value_template_kind("BIT(65)", "mysql"), None);
    assert_eq!(
        sql_type_value_template_kind("bit(1000000)", "postgres"),
        Some(SqlValueTemplateKind::BinaryBits(Some(1000000)))
    );
    assert_eq!(
        sql_type_value_template_kind("Nullable(LowCardinality(Array(Boolean)))", "clickhouse"),
        Some(SqlValueTemplateKind::NullOnly)
    );
}

fn position(sql: &str) -> Position {
    // Dialect helpers receive byte positions converted by the LSP layer.
    Position::new(0, sql.len() as u32)
}

#[test]
fn mismatched_prefix_lengths_never_slice_inside_unicode() {
    for sql in [
        "SELECT * FROM jobs WHERE \"用户名称\"",
        "SELECT * FROM jobs WHERE \"😀字段\"",
        "SELECT * FROM jobs WHERE 用户名称",
    ] {
        for length in 1..=sql.len() + 2 {
            assert_eq!(
                predicate_column_before_value(sql, position(sql), &"a".repeat(length), "sqlserver"),
                None
            );
        }
    }
}

#[test]
fn unicode_predicate_columns_keep_their_complete_name() {
    for reference in [
        "用户名称",
        "\"用户名称\"",
        "`用户名称`",
        "[用户名称]",
        "t.\"用户名称\"",
    ] {
        let sql = format!("SELECT * FROM jobs t WHERE {reference} = ");
        assert_eq!(
            predicate_column_before_value(&sql, position(&sql), "", "sqlserver")
                .map(|value| value.column),
            Some("用户名称".to_string())
        );
    }
}

#[test]
fn unicode_value_prefixes_are_removed_only_at_valid_matching_boundaries() {
    for value in ["中文", "中文😀", "a中文"] {
        let sql = format!("SELECT * FROM jobs WHERE \"用户名称\" = '{value}");
        assert_eq!(
            predicate_column_before_value(&sql, position(&sql), value, "sqlserver")
                .map(|value| value.column),
            Some("用户名称".to_string())
        );
    }
}
