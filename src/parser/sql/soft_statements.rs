pub(super) const SOFT_STATEMENT_KEYWORDS: &[&str] = &[
    "SELECT", "WITH", "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "CREATE", "ALTER", "DROP",
    "TRUNCATE", "EXPLAIN", "SHOW", "DESCRIBE", "DESC", "USE", "CALL", "EXEC", "EXECUTE", "BEGIN",
    "COMMIT", "ROLLBACK", "ANALYZE", "VACUUM",
];

pub(super) fn soft_statement_keyword(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    let end = trimmed
        .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .unwrap_or(trimmed.len());
    let keyword = trimmed.get(..end)?;
    SOFT_STATEMENT_KEYWORDS
        .iter()
        .copied()
        .find(|candidate| keyword.eq_ignore_ascii_case(candidate))
}

pub(super) fn soft_statement_is_wrapped_continuation(initial: &str, candidate: &str) -> bool {
    match initial {
        "WITH" => matches!(
            candidate,
            "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "MERGE"
        ),
        "INSERT" => matches!(candidate, "SELECT" | "WITH"),
        "EXPLAIN" | "DESCRIBE" | "DESC" => matches!(
            candidate,
            "SELECT" | "WITH" | "INSERT" | "UPDATE" | "DELETE" | "MERGE"
        ),
        "CREATE" => matches!(candidate, "SELECT" | "WITH" | "BEGIN"),
        "ALTER" => candidate == "UPDATE",
        _ => false,
    }
}

pub(super) fn soft_statement_is_set_continuation(
    previous_non_empty_line: &str,
    candidate: &str,
) -> bool {
    candidate == "SELECT"
        && matches!(
            previous_non_empty_line.trim().to_ascii_uppercase().as_str(),
            "UNION" | "UNION ALL" | "INTERSECT" | "EXCEPT" | "MINUS"
        )
}
