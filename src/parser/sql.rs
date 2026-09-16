//! SQL parser facade. One SqlParser owns incremental state; feature modules extend it.

mod aliases;
mod case_context;
mod completion_context;
mod context_probes;
mod core;
mod cte_scope;
mod ddl_context;
mod identifiers;
mod insert_context;
mod masking;
mod order_group_context;
mod predicate_context;
mod query_scopes;
mod reference_context;
mod relation_continuation;
mod relation_scanner;
mod relation_targets;
mod row_sources;
mod soft_statements;
mod statement_boundaries;
#[cfg(test)]
mod tests;
mod types;

pub use types::{AstNode, CompletionContext, ParseResult, RelationAlias, SqlParser};
