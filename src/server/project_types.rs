//! Project types for the SQL language server.
use tower_lsp::lsp_types::*;
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum ProjectSqlSymbolKind {
    Table,
    View,
    Function,
    Procedure,
}

impl ProjectSqlSymbolKind {
    pub(super) fn symbol_kind(self) -> SymbolKind {
        match self {
            Self::Table | Self::View => SymbolKind::CLASS,
            Self::Function => SymbolKind::FUNCTION,
            Self::Procedure => SymbolKind::METHOD,
        }
    }

    pub(super) fn detail(self) -> &'static str {
        match self {
            Self::Table => "Project table",
            Self::View => "Project view",
            Self::Function => "Project function",
            Self::Procedure => "Project procedure",
        }
    }

    pub(super) fn is_routine(self) -> bool {
        matches!(self, Self::Function | Self::Procedure)
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum ProjectSqlSymbolRole {
    Definition,
    Reference,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectSqlSymbolOccurrence {
    pub(super) name: String,
    pub(super) normalized_name: String,
    pub(super) kind: ProjectSqlSymbolKind,
    pub(super) role: ProjectSqlSymbolRole,
    pub(super) range: Range,
}

#[derive(Clone)]
pub(super) struct CachedProjectSqlIndex {
    pub(super) revision: u64,
    pub(super) occurrences: Vec<ProjectSqlSymbolOccurrence>,
}
