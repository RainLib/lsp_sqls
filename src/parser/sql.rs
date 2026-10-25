//! SQL parser facade. One SqlParser owns incremental state; feature modules extend it.

mod aliases;
mod apply;
mod array_casts;
mod bracket_identifiers;
mod case_context;
mod completion_context;
mod context_probes;
mod core;
mod correlation;
mod cte_scope;
mod ddl_context;
mod fulltext;
mod identifier_paths;
mod identifiers;
mod insert_context;
mod join_model;
mod join_scope;
mod lateral_views;
mod masking;
mod match_against;
mod normalization;
mod order_group_context;
mod predicate_context;
mod qualify;
mod query_scopes;
mod reference_context;
mod relation_continuation;
mod relation_scanner;
mod relation_targets;
mod row_sources;
mod soft_statements;
mod statement_boundaries;
mod syntax_tree;
#[cfg(test)]
mod tests;
mod tsql_modules;
mod types;

pub(crate) use lateral_views::LateralView;
pub use types::{
    AstNode, CompletionContext, JoinExpr, JoinKind, ParseResult, RelationAlias, RelationScope,
    SqlParser,
};
