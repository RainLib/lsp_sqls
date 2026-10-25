//! WITH output inference owns an ordered local catalog. Bodies and the final
//! query share the caller's branch and recursion budgets, including recursion.
use super::{
    install_local_table, local_keyword_boundary, local_relation_table,
    matching_parenthesis_in_searchable,
    query_output::{Context, Output},
    read_local_identifier_after, recursive_ctes, skip_optional_keyword_sequence, skip_sql_trivia,
    split_top_level_ranges_in_searchable,
};

pub(super) fn infer(
    context: &Context<'_>,
    query: &str,
    upper: &str,
    start: usize,
    depth: usize,
    budget: &mut usize,
) -> Output {
    let mut cursor = skip_sql_trivia(query, start + "WITH".len());
    let recursive = upper[cursor..].starts_with("RECURSIVE")
        && local_keyword_boundary(upper, cursor, "RECURSIVE");
    if recursive {
        cursor = skip_sql_trivia(query, cursor + "RECURSIVE".len());
    }
    let mut working = context.schema.clone();
    loop {
        let Some((name, name_start, name_end)) = read_local_identifier_after(query, cursor) else {
            return Output::default();
        };
        cursor = skip_sql_trivia(query, name_end);
        let mut names = Vec::new();
        if upper.as_bytes().get(cursor) == Some(&b'(') {
            let Some(close) = matching_parenthesis_in_searchable(upper, cursor) else {
                return Output::default();
            };
            for (start, end) in split_top_level_ranges_in_searchable(upper, cursor + 1, close) {
                let Some((name, _, name_end)) = read_local_identifier_after(query, start) else {
                    return Output::default();
                };
                if skip_sql_trivia(query, name_end) < end {
                    return Output::default();
                }
                names.push(name);
            }
            cursor = skip_sql_trivia(query, close + 1);
        }
        if !upper[cursor..].starts_with("AS") || !local_keyword_boundary(upper, cursor, "AS") {
            return Output::default();
        }
        cursor = skip_sql_trivia(query, cursor + 2);
        cursor = skip_optional_keyword_sequence(upper, cursor, &["NOT", "MATERIALIZED"]);
        cursor = skip_optional_keyword_sequence(upper, cursor, &["MATERIALIZED"]);
        if upper.as_bytes().get(cursor) != Some(&b'(') {
            return Output::default();
        }
        let Some(close) = matching_parenthesis_in_searchable(upper, cursor) else {
            return Output::default();
        };
        let body = &query[cursor + 1..close];
        let body_upper = &upper[cursor + 1..close];
        let line = context
            .line
            .saturating_add(query[..cursor + 1].matches('\n').count() as u32);
        let shape = if recursive || recursive_ctes::implicit(context.dialect) {
            recursive_ctes::infer_with_budget(
                &recursive_ctes::Definition {
                    name: &name,
                    quoted: matches!(query.as_bytes()[name_start], b'"' | b'`' | b'['),
                    names: &names,
                    body,
                    upper: body_upper,
                    line,
                },
                &working,
                context.uri,
                context.dialect,
                depth + 1,
                budget,
                context.environment,
            )
        } else {
            None
        };
        let columns = shape.map(|shape| shape.columns).unwrap_or_else(|| {
            let inner = Context {
                schema: &working,
                uri: context.uri,
                line,
                dialect: context.dialect,
                environment: context.environment,
            };
            let output = inner.query(body, body_upper, depth + 1, budget);
            inner.finish(output, &names)
        });
        install_local_table(
            &mut working,
            local_relation_table(
                name,
                "COMMON TABLE EXPRESSION",
                columns,
                context.uri,
                context
                    .line
                    .saturating_add(query[..name_start].matches('\n').count() as u32),
            ),
        );
        cursor = skip_sql_trivia(query, close + 1);
        if upper.as_bytes().get(cursor) != Some(&b',') {
            break;
        }
        cursor = skip_sql_trivia(query, cursor + 1);
    }
    Context {
        schema: &working,
        uri: context.uri,
        line: context
            .line
            .saturating_add(query[..cursor].matches('\n').count() as u32),
        dialect: context.dialect,
        environment: context.environment,
    }
    .query(&query[cursor..], &upper[cursor..], depth + 1, budget)
}

#[cfg(test)]
mod tests {
    use crate::schema::{Column, Schema, SchemaId, Table};
    fn schema() -> Schema {
        Schema {
            id: SchemaId::new(),
            database: "app".into(),
            server: None,
            namespace: None,
            catalog: None,
            server_version: None,
            source_uri: None,
            functions: vec![],
            tables: vec![Table {
                name: "users".into(),
                columns: vec![
                    Column {
                        name: "id".into(),
                        data_type: "integer".into(),
                        ..Default::default()
                    },
                    Column {
                        name: "name".into(),
                        data_type: "text".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
        }
    }
    fn output(sql: &str, names: &[&str]) -> Vec<(String, String)> {
        super::super::query_output::infer_query_output_columns_with_names(
            sql,
            &schema(),
            "oxide://with.sql",
            1,
            Some("postgres"),
            &names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>(),
        )
        .into_iter()
        .map(|column| (column.name, column.data_type))
        .collect()
    }
    #[test]
    fn nested_with_outputs_follow_ordered_local_bindings() {
        let sql="WITH c AS (SELECT id AS value FROM users), d AS (WITH c AS (SELECT name AS label FROM users) SELECT c.* FROM c), e(x) AS (SELECT label FROM d) SELECT e.*,c.value FROM e CROSS JOIN c";
        assert_eq!(
            output(sql, &[]),
            [
                ("x".into(), "text".into()),
                ("value".into(), "integer".into())
            ]
        );
    }
    #[test]
    fn nested_recursive_definition_keeps_anchor_types() {
        let sql="WITH x AS (WITH RECURSIVE r(n) AS (SELECT id FROM users UNION ALL SELECT r.n+1 FROM r WHERE r.n<10) SELECT r.n FROM r) SELECT * FROM x";
        assert_eq!(output(sql, &[]), [("n".into(), "integer".into())]);
    }
    #[test]
    fn with_bindings_cover_all_set_branches_and_materialized_names() {
        let sql="WITH /* 🧭 */ \"rows\"(v) AS NOT MATERIALIZED (SELECT id FROM users) SELECT v FROM \"rows\" UNION ALL SELECT v::bigint FROM \"rows\"";
        assert_eq!(output(sql, &[]), [("v".into(), "bigint".into())]);
    }
    #[test]
    fn with_bodies_and_final_query_share_branch_budget() {
        for (bodies, ty) in [(63, "integer"), (64, "unknown")] {
            let defs = (0..bodies)
                .map(|i| format!("c{i}(v) AS (SELECT 1)"))
                .collect::<Vec<_>>()
                .join(",");
            assert_eq!(
                output(&format!("WITH {defs} SELECT v FROM c0"), &["result"]),
                [("result".into(), ty.into())]
            );
        }
    }
    #[test]
    fn recursive_ctes_do_not_restart_enclosing_with_budget() {
        for (bodies, ty) in [(61, "integer"), (62, "unknown")] {
            let defs = (0..bodies)
                .map(|i| format!("c{i}(v) AS (SELECT 1)"))
                .collect::<Vec<_>>()
                .join(",");
            let sql=format!("WITH RECURSIVE {defs},r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<10) SELECT n FROM r");
            assert_eq!(output(&sql, &["result"]), [("result".into(), ty.into())]);
        }
    }
    #[test]
    fn nested_with_depth_and_malformed_bodies_degrade_without_catalog_guessing() {
        for (layers, ty) in [(5, "integer"), (40, "unknown")] {
            let mut sql = "SELECT id FROM users".to_string();
            for _ in 0..layers {
                sql = format!("WITH c AS ({sql}) SELECT * FROM c");
            }
            assert_eq!(output(&sql, &["result"]), [("result".into(), ty.into())]);
        }
        for sql in [
            "WITH c AS (SELECT id FROM users)",
            "WITH c AS (SELECT id FROM users",
            "WITH c(a b) AS (SELECT id FROM users) SELECT * FROM c",
        ] {
            assert_eq!(
                output(sql, &["result"]),
                [("result".into(), "unknown".into())]
            );
        }
    }
}
