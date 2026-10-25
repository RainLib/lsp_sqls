//! Request-scanner rules. Legacy callers retain their old generic policy;
//! dialect-aware editor features distinguish array brackets from identifiers.
use crate::placeholder::SqlPlaceholderDialect;

#[derive(Clone, Copy)]
pub(crate) struct LexicalRules {
    pub placeholder: SqlPlaceholderDialect,
    pub bracket_identifiers: bool,
    pub nested_comments: bool,
    pub hash_comments: bool,
    pub slash_comments: bool,
    pub dollar_quotes: bool,
    pub backslash_strings: bool,
    pub postgres_escape_strings: bool,
    pub double_quoted_strings: bool,
    pub backslash_identifiers: bool,
}

pub(crate) const LEGACY_LEXICAL_RULES: LexicalRules = LexicalRules {
    placeholder: SqlPlaceholderDialect::Generic,
    bracket_identifiers: true,
    nested_comments: false,
    hash_comments: true,
    slash_comments: false,
    dollar_quotes: true,
    backslash_strings: true,
    postgres_escape_strings: false,
    double_quoted_strings: false,
    backslash_identifiers: false,
};

pub(crate) fn lexical_rules_for(dialect: &str) -> LexicalRules {
    let named = |names: &[&str]| names.iter().any(|name| dialect.eq_ignore_ascii_case(name));
    let postgres = named(&["postgres", "postgresql", "pgsql", "psql"]);
    let mysql = named(&["mysql", "mariadb"]);
    let sqlserver = named(&["sqlserver", "mssql", "tsql"]);
    let clickhouse = named(&["clickhouse"]);
    let hive = named(&["hive"]);
    let sqlite = named(&["sqlite"]);
    if !postgres && !mysql && !sqlserver && !clickhouse && !hive && !sqlite {
        return LEGACY_LEXICAL_RULES;
    }
    LexicalRules {
        placeholder: if postgres {
            SqlPlaceholderDialect::Postgres
        } else if mysql {
            SqlPlaceholderDialect::Mysql
        } else if sqlserver {
            SqlPlaceholderDialect::SqlServer
        } else {
            SqlPlaceholderDialect::Generic
        },
        bracket_identifiers: sqlserver || sqlite,
        nested_comments: postgres || sqlserver || clickhouse,
        hash_comments: mysql || clickhouse,
        slash_comments: clickhouse,
        dollar_quotes: postgres || clickhouse,
        backslash_strings: mysql || clickhouse || hive,
        postgres_escape_strings: postgres,
        double_quoted_strings: mysql || hive,
        backslash_identifiers: clickhouse,
    }
}

impl LexicalRules {
    pub(crate) fn hash_comment_at(self, source: &str, index: usize) -> bool {
        if !self.hash_comments || source.as_bytes().get(index) != Some(&b'#') {
            return false;
        }
        let tail = &source[index..];
        if self.slash_comments {
            return tail.starts_with("#!")
                || tail[1..].chars().next().is_some_and(char::is_whitespace);
        }
        if self.placeholder == SqlPlaceholderDialect::Mysql {
            return true;
        }
        !tail.starts_with("#>")
            && !tail.starts_with("#-")
            && (index == 0
                || source[..index]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace))
    }
    pub(crate) fn escapes_at(self, source: &str, index: usize, quote: char) -> bool {
        if quote != '\''
            && !(quote == '"' && self.double_quoted_strings)
            && !self.backslash_identifiers
        {
            return false;
        }
        self.backslash_strings
            || self.postgres_escape_strings
                && index > 0
                && matches!(source.as_bytes()[index - 1], b'e' | b'E')
                && (index == 1
                    || !source[..index - 1]
                        .chars()
                        .next_back()
                        .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$')))
    }
}
