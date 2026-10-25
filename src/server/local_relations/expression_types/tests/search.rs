use super::{check, output};

#[test]
fn postgres_search_operators_compose_without_whitespace() {
    for (expr, ty) in [
        ("to_tsvector(i.label)@@to_tsquery('oxide:*')", "boolean"),
        ("to_tsvector(i.label)||to_tsvector('studio')", "tsvector"),
        ("to_tsquery('oxide')&&to_tsquery('studio')", "tsquery"),
        ("to_tsquery('oxide')<->to_tsquery('studio')", "tsquery"),
        (
            "to_tsquery('oxide')@>to_tsquery('oxide & studio')",
            "boolean",
        ),
        ("i.label~*'^oxide'", "boolean"),
        ("CAST(i.label AS jsonb)@?'$.tags[*]'", "boolean"),
        ("CAST(i.label AS jsonb)@@'$.count > 0'", "boolean"),
        ("CAST(i.label AS jsonb)->>'title'", "text"),
        ("CAST(i.label AS jsonb)#>>'{tags,0}'", "text"),
        ("CAST(i.label AS jsonb)?'title'", "boolean"),
        ("CAST(i.label AS jsonb)?&ARRAY['title','tags']", "boolean"),
        (
            "CAST(i.label AS jsonb)?|CAST('{title,tags}' AS text[])",
            "boolean",
        ),
        (
            "ts_rank(to_tsvector(i.label),plainto_tsquery('oxide')) > 0.1 AND i.label ~* 'sql'",
            "boolean",
        ),
        (
            "to_tsvector(i.label)@@to_tsquery('oxide') OR CAST(i.label AS jsonb)@?'$.tags'",
            "boolean",
        ),
    ] {
        check("postgres", expr, ty, true);
    }
}

#[test]
fn search_types_do_not_guess_unknown_operators_or_functions() {
    for (dialect, expr) in [
        ("postgres", "i.amount @@ i.large"),
        ("postgres", "i.amount ~ i.large"),
        ("postgres", "i.label @? '$.tags'"),
        (
            "postgres",
            "CAST(i.label AS jsonb) ?| CAST('{1}' AS integer[])",
        ),
        ("postgres", "public.to_tsvector(i.label)"),
        ("postgres", "to_tsvector()"),
        ("mysql", "MATCH(i.label)"),
        ("postgres", "MATCH(i.label) AGAINST ('oxide')"),
        ("sqlite", "to_tsvector(i.label)"),
        ("mysql", "websearch_to_tsquery(i.label)"),
    ] {
        assert!(
            output(dialect, expr).data_type == "unknown",
            "{dialect}: {expr}"
        );
    }
}

#[test]
fn mysql_fulltext_modes_and_bound_queries_keep_double_type() {
    for query in [
        "'oxide'",
        "'oxide' IN BOOLEAN MODE",
        "'oxide' IN NATURAL LANGUAGE MODE",
        "'oxide' WITH QUERY EXPANSION",
        "'oxide' IN NATURAL LANGUAGE MODE WITH QUERY EXPANSION",
        ":query IN BOOLEAN MODE",
        "CONCAT('oxide', ' studio') IN BOOLEAN MODE",
    ] {
        check(
            "mysql",
            &format!("MATCH(i.label) AGAINST ({query})"),
            "double",
            true,
        );
    }
}

#[test]
fn weighted_search_vectors_and_negated_predicates_keep_types() {
    check(
        "postgres",
        "setweight(to_tsvector(i.label),'A')||setweight(to_tsvector('sql'),'B')",
        "tsvector",
        true,
    );
    check(
        "postgres",
        "NOT (to_tsvector(i.label)@@to_tsquery('oxide'))",
        "boolean",
        true,
    );
    check("postgres", "!!to_tsquery('oxide')", "tsquery", true);
}
