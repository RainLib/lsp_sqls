//! Shared search signatures and fixed result families. SQL syntax clauses are
//! kept out of ordinary function completion; compatible engines are not aliases.
use crate::builtin_signatures::BuiltinSignature;

pub(crate) struct SearchFunction {
    pub name: &'static str,
    dialect: &'static str,
    minimum: (u32, u32),
    signatures: &'static [&'static [&'static str]],
    result: SearchResult,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchResult {
    Scalar(&'static str),
    Predicate,
    Table,
}
use SearchResult::{Predicate, Scalar, Table};

const FUNCTIONS: &[SearchFunction] = &[
    SearchFunction {
        name: "CONTAINSTABLE",
        dialect: "sqlserver",
        minimum: (0, 0),
        signatures: &[
            &["table", "column_or_columns", "query"],
            &["table", "column_or_columns", "query", "top_n_by_rank"],
            &[
                "table",
                "column_or_columns",
                "query",
                "LANGUAGE language_term",
                "top_n_by_rank?",
            ],
        ],
        result: Table,
    },
    SearchFunction {
        name: "FREETEXTTABLE",
        dialect: "sqlserver",
        minimum: (0, 0),
        signatures: &[
            &["table", "column_or_columns", "query"],
            &["table", "column_or_columns", "query", "top_n_by_rank"],
            &[
                "table",
                "column_or_columns",
                "query",
                "LANGUAGE language_term",
                "top_n_by_rank?",
            ],
        ],
        result: Table,
    },
    SearchFunction {
        name: "CONTAINS",
        dialect: "sqlserver",
        minimum: (0, 0),
        signatures: &[
            &["column_or_columns", "query"],
            &["column_or_columns", "query", "LANGUAGE language_term"],
        ],
        result: Predicate,
    },
    SearchFunction {
        name: "FREETEXT",
        dialect: "sqlserver",
        minimum: (0, 0),
        signatures: &[
            &["column_or_columns", "query"],
            &["column_or_columns", "query", "LANGUAGE language_term"],
        ],
        result: Predicate,
    },
    SearchFunction {
        name: "CONTAINS",
        dialect: "oracle",
        minimum: (0, 0),
        signatures: &[&["column", "text_query", "label?"]],
        result: Scalar("number"),
    },
    SearchFunction {
        name: "SCORE",
        dialect: "oracle",
        minimum: (0, 0),
        signatures: &[&["label"]],
        result: Scalar("number"),
    },
    SearchFunction {
        name: "SETWEIGHT",
        dialect: "postgres",
        minimum: (8, 3),
        signatures: &[&["vector", "weight"]],
        result: Scalar("tsvector"),
    },
    SearchFunction {
        name: "TO_TSVECTOR",
        dialect: "postgres",
        minimum: (8, 3),
        signatures: &[&["document"], &["config", "document"]],
        result: Scalar("tsvector"),
    },
    SearchFunction {
        name: "TO_TSQUERY",
        dialect: "postgres",
        minimum: (8, 3),
        signatures: &[&["query"], &["config", "query"]],
        result: Scalar("tsquery"),
    },
    SearchFunction {
        name: "PLAINTO_TSQUERY",
        dialect: "postgres",
        minimum: (8, 3),
        signatures: &[&["query"], &["config", "query"]],
        result: Scalar("tsquery"),
    },
    SearchFunction {
        name: "PHRASETO_TSQUERY",
        dialect: "postgres",
        minimum: (9, 6),
        signatures: &[&["query"], &["config", "query"]],
        result: Scalar("tsquery"),
    },
    SearchFunction {
        name: "WEBSEARCH_TO_TSQUERY",
        dialect: "postgres",
        minimum: (11, 0),
        signatures: &[&["query"], &["config", "query"]],
        result: Scalar("tsquery"),
    },
    SearchFunction {
        name: "TS_RANK",
        dialect: "postgres",
        minimum: (8, 3),
        signatures: &[
            &["vector", "query", "normalization?"],
            &["weights", "vector", "query", "normalization?"],
        ],
        result: Scalar("real"),
    },
    SearchFunction {
        name: "TS_RANK_CD",
        dialect: "postgres",
        minimum: (8, 3),
        signatures: &[
            &["vector", "query", "normalization?"],
            &["weights", "vector", "query", "normalization?"],
        ],
        result: Scalar("real"),
    },
    SearchFunction {
        name: "TS_HEADLINE",
        dialect: "postgres",
        minimum: (8, 3),
        signatures: &[
            &["document", "query", "options?"],
            &["config", "document", "query", "options?"],
        ],
        result: Scalar("text"),
    },
    SearchFunction {
        name: "JSONB_PATH_EXISTS",
        dialect: "postgres",
        minimum: (12, 0),
        signatures: &[&["target", "path", "vars?", "silent?"]],
        result: Scalar("boolean"),
    },
    SearchFunction {
        name: "JSONB_PATH_MATCH",
        dialect: "postgres",
        minimum: (12, 0),
        signatures: &[&["target", "path", "vars?", "silent?"]],
        result: Scalar("boolean"),
    },
    SearchFunction {
        name: "JSONB_PATH_QUERY_FIRST",
        dialect: "postgres",
        minimum: (12, 0),
        signatures: &[&["target", "path", "vars?", "silent?"]],
        result: Scalar("jsonb"),
    },
    SearchFunction {
        name: "JSONB_PATH_QUERY_ARRAY",
        dialect: "postgres",
        minimum: (12, 0),
        signatures: &[&["target", "path", "vars?", "silent?"]],
        result: Scalar("jsonb"),
    },
    SearchFunction {
        name: "MATCH",
        dialect: "mysql",
        minimum: (0, 0),
        signatures: &[&["column", "...columns"]],
        result: Scalar("double"),
    },
    SearchFunction {
        name: "REGEXP_LIKE",
        dialect: "mysql",
        minimum: (8, 0),
        signatures: &[&["expression", "pattern", "match_type?"]],
        result: Scalar("integer"),
    },
    SearchFunction {
        name: "REGEXP_INSTR",
        dialect: "mysql",
        minimum: (8, 0),
        signatures: &[&[
            "expression",
            "pattern",
            "position?",
            "occurrence?",
            "return_option?",
            "match_type?",
        ]],
        result: Scalar("integer"),
    },
    SearchFunction {
        name: "JSON_CONTAINS",
        dialect: "mysql",
        minimum: (5, 7),
        signatures: &[&["target", "candidate", "path?"]],
        result: Scalar("integer"),
    },
    SearchFunction {
        name: "JSON_CONTAINS_PATH",
        dialect: "mysql",
        minimum: (5, 7),
        signatures: &[&["document", "one_or_all", "path", "...paths"]],
        result: Scalar("integer"),
    },
    SearchFunction {
        name: "JSON_EXTRACT",
        dialect: "mysql",
        minimum: (5, 7),
        signatures: &[&["document", "path", "...paths"]],
        result: Scalar("json"),
    },
    SearchFunction {
        name: "JSON_SEARCH",
        dialect: "mysql",
        minimum: (5, 7),
        signatures: &[&["document", "one_or_all", "search", "escape?", "...paths"]],
        result: Scalar("json"),
    },
];

fn family(dialect: &str) -> &str {
    match dialect {
        "postgresql" | "pgsql" | "psql" => "postgres",
        "tsql" | "mssql" => "sqlserver",
        other => other,
    }
}

pub(crate) fn catalog_for(
    dialect: &str,
    version: Option<(u32, u32)>,
) -> impl Iterator<Item = &'static SearchFunction> + '_ {
    FUNCTIONS.iter().filter(move |rule| {
        rule.dialect == family(dialect) && version.is_none_or(|version| version >= rule.minimum)
    })
}

fn signature(rule: &SearchFunction, parameters: &[&'static str]) -> BuiltinSignature {
    BuiltinSignature {
        name: rule.name.into(),
        parameter_groups: vec![parameters.to_vec()],
    }
}

pub(crate) fn completion_catalog(
    dialect: &str,
    version: Option<(u32, u32)>,
) -> Vec<BuiltinSignature> {
    catalog_for(dialect, version)
        .filter(|rule| rule.result != Table)
        .map(|rule| signature(rule, rule.signatures[0]))
        .collect()
}

pub(crate) fn table_catalog(dialect: &str) -> Vec<BuiltinSignature> {
    catalog_for(dialect, None)
        .filter(|rule| rule.result == Table)
        .map(|rule| signature(rule, rule.signatures[0]))
        .collect()
}

pub(crate) fn is_table_function(dialect: &str, name: &str) -> bool {
    catalog_for(dialect, None)
        .any(|rule| rule.result == Table && rule.name.eq_ignore_ascii_case(name))
}

pub(crate) fn is_predicate(dialect: &str, name: &str) -> bool {
    catalog_for(dialect, None)
        .any(|rule| rule.result == Predicate && rule.name.eq_ignore_ascii_case(name))
}

pub(crate) fn signatures_for(
    dialect: &str,
    name: &str,
    version: Option<(u32, u32)>,
) -> Vec<BuiltinSignature> {
    if dialect == "mysql" && name.eq_ignore_ascii_case("AGAINST") {
        return vec![BuiltinSignature {
            name: "AGAINST".into(),
            parameter_groups: vec![vec!["query [IN search_mode]"]],
        }];
    }
    catalog_for(dialect, version)
        .find(|rule| rule.name.eq_ignore_ascii_case(name))
        .map(|rule| {
            rule.signatures
                .iter()
                .map(|parameters| signature(rule, parameters))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn return_type(
    dialect: &str,
    name: &str,
    arity: usize,
    version: Option<(u32, u32)>,
) -> Option<&'static str> {
    let rule = catalog_for(dialect, version).find(|rule| rule.name.eq_ignore_ascii_case(name))?;
    let Scalar(result) = rule.result else {
        return None;
    };
    rule.signatures
        .iter()
        .any(|parameters| {
            let required = parameters
                .iter()
                .filter(|param| !param.ends_with('?') && !param.starts_with("..."))
                .count();
            arity >= required
                && (parameters
                    .last()
                    .is_some_and(|param| param.starts_with("..."))
                    || arity <= parameters.len())
        })
        .then_some(result)
}

pub(crate) fn completion_snippet(dialect: &str, name: &str) -> Option<&'static str> {
    (dialect == "mysql" && name.eq_ignore_ascii_case("MATCH"))
        .then_some("MATCH(${1:column}) AGAINST (${2:query} IN BOOLEAN MODE)")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_catalog_applies_family_version_and_arity_limits() {
        for (name, before, since) in [
            ("PHRASETO_TSQUERY", (9, 5), (9, 6)),
            ("WEBSEARCH_TO_TSQUERY", (10, 0), (11, 0)),
            ("JSONB_PATH_EXISTS", (11, 0), (12, 0)),
        ] {
            assert!(signatures_for("postgres", name, Some(before)).is_empty());
            assert!(!signatures_for("postgresql", name, Some(since)).is_empty());
            assert!(return_type("postgres", name, 1, Some(before)).is_none());
        }
        assert!(return_type("postgres", "to_tsvector", 0, None).is_none());
        assert!(return_type("postgres", "to_tsvector", 3, None).is_none());
        assert_eq!(
            return_type("postgres", "to_tsvector", 2, None),
            Some("tsvector")
        );
        assert!(return_type("mysql", "regexp_like", 2, Some((5, 7))).is_none());
        assert_eq!(
            return_type("mysql", "regexp_like", 2, Some((8, 0))),
            Some("integer")
        );
        for dialect in ["mariadb", "sqlite", "clickhouse"] {
            assert!(catalog_for(dialect, None).next().is_none());
            assert!(completion_snippet(dialect, "MATCH").is_none());
        }
        assert!(completion_catalog("mysql", None)
            .iter()
            .all(|s| s.name != "AGAINST"));
    }
    #[test]
    fn rowsets_predicates_and_oracle_numeric_scores_are_distinct() {
        for dialect in ["sqlserver", "mssql", "tsql"] {
            assert_eq!(table_catalog(dialect).len(), 2);
            assert!(is_table_function(dialect, "CONTAINSTABLE"));
            assert!(!completion_catalog(dialect, None)
                .iter()
                .any(|s| s.name == "CONTAINSTABLE"));
            assert!(return_type(dialect, "CONTAINSTABLE", 3, None).is_none());
            assert!(return_type(dialect, "CONTAINS", 2, None).is_none());
            assert!(is_predicate(dialect, "FREETEXT"));
        }
        assert_eq!(return_type("oracle", "CONTAINS", 2, None), Some("number"));
        assert_eq!(return_type("oracle", "CONTAINS", 3, None), Some("number"));
        assert_eq!(return_type("oracle", "SCORE", 1, None), Some("number"));
        for (name, arity) in [("CONTAINS", 1), ("CONTAINS", 4), ("SCORE", 0), ("SCORE", 2)] {
            assert!(return_type("oracle", name, arity, None).is_none());
        }
        for dialect in ["postgres", "mysql", "oracle", "oceanbase-oracle", "sqlite"] {
            assert!(table_catalog(dialect).is_empty());
            assert!(!is_table_function(dialect, "FREETEXTTABLE"));
        }
    }
}
