//! 解析器模块
//! 参考 sqls-server/sqls 的实现
//! https://github.com/sqls-server/sqls/tree/master/parser

pub mod dsl;
pub mod sql;

pub use dsl::{DslCompletionContext, DslParser};
pub(crate) use sql::LateralView;
pub use sql::{
    AstNode, CompletionContext, JoinExpr, JoinKind, ParseResult, RelationAlias, RelationScope,
    SqlParser,
};
