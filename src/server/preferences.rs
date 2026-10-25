//! Preferences for the SQL language server.
use crate::dialects::common::LogicalOperatorLayout;
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CompletionPreferences {
    #[serde(default = "default_keyword_case")]
    pub(super) keyword_case: KeywordCase,
    #[serde(default = "default_function_case")]
    pub(super) function_case: KeywordCase,
    #[serde(default)]
    pub(super) table_alias: TableAliasStyle,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FormattingPreferences {
    #[serde(default = "default_keyword_case")]
    pub(super) keyword_case: KeywordCase,
    #[serde(default = "default_function_case")]
    pub(super) function_case: KeywordCase,
    #[serde(default)]
    pub(super) logical_operator_newline: LogicalOperatorNewline,
    #[serde(default)]
    pub(super) from_clause_layout: FromClauseLayout,
}

impl Default for FormattingPreferences {
    fn default() -> Self {
        Self {
            keyword_case: KeywordCase::Upper,
            function_case: KeywordCase::Preserve,
            logical_operator_newline: LogicalOperatorNewline::Before,
            from_clause_layout: FromClauseLayout::NewLine,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) enum LogicalOperatorNewline {
    #[default]
    Before,
    After,
    None,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) enum FromClauseLayout {
    #[default]
    NewLine,
    SameLine,
}

impl FormattingPreferences {
    pub(super) fn logical_operator_layout(&self) -> LogicalOperatorLayout {
        match self.logical_operator_newline {
            LogicalOperatorNewline::Before => LogicalOperatorLayout::Before,
            LogicalOperatorNewline::After => LogicalOperatorLayout::After,
            LogicalOperatorNewline::None => LogicalOperatorLayout::SameLine,
        }
    }

    pub(super) fn keyword_uppercase(&self) -> Option<bool> {
        self.keyword_case.uppercase_option()
    }

    pub(super) fn function_uppercase(&self) -> Option<bool> {
        self.function_case.uppercase_option()
    }
}

impl Default for CompletionPreferences {
    fn default() -> Self {
        Self {
            keyword_case: KeywordCase::Upper,
            function_case: KeywordCase::Preserve,
            table_alias: TableAliasStyle::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum KeywordCase {
    Upper,
    Lower,
    Preserve,
}

impl KeywordCase {
    pub(super) fn uppercase_option(self) -> Option<bool> {
        match self {
            Self::Upper => Some(true),
            Self::Lower => Some(false),
            Self::Preserve => None,
        }
    }
}

pub(super) fn default_keyword_case() -> KeywordCase {
    KeywordCase::Upper
}

pub(super) fn default_function_case() -> KeywordCase {
    KeywordCase::Preserve
}

pub(super) fn dialect_uses_sql_preferences(dialect: &str) -> bool {
    matches!(
        dialect,
        "postgres" | "mysql" | "sqlite" | "duckdb" | "hive" | "clickhouse" | "sqlserver"
    )
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum TableAliasStyle {
    #[default]
    None,
    Initials,
}
