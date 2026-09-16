use crate::placeholder::SqlPlaceholderDialect;
use tower_lsp::lsp_types::Diagnostic;
use tower_lsp::lsp_types::Range;
use tree_sitter::Parser;
use tree_sitter::Tree;

/// 补全上下文类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    /// 在 FROM 子句中，应该补全表名
    FromClause,
    /// 在 FROM 关系目标之后，应该补全 JOIN/WHERE/GROUP 等后续结构
    FromContinuationClause,
    /// 在 SELECT 子句中，应该补全列名和关键字
    SelectClause,
    /// 在 SELECT 表达式之后，应该补全 FROM/AS/逗号
    SelectContinuationClause,
    /// 在 WHERE 子句中，应该补全列名、操作符、关键字
    WhereClause,
    /// 在谓词或赋值操作符之后，应该补全值表达式
    ExpressionValueClause,
    /// 在完整谓词或赋值之后，应该补全连接词或后续子句
    PredicateContinuationClause,
    /// 在 CASE THEN/ELSE 之后，应该补全结果表达式
    CaseResultClause,
    /// 在简单 CASE WHEN 值之后，应该补全 THEN
    CaseWhenValueContinuationClause,
    /// 在 CASE 结果表达式之后，应该补全 WHEN/ELSE/END
    CaseContinuationClause,
    /// 在表名后（如 table.），应该补全列名
    TableColumn,
    /// 在 JOIN 子句中，应该补全表名
    JoinClause,
    /// 在 JOIN 关系目标之后，应该补全 ON/USING 条件
    JoinConditionClause,
    /// 在 ORDER BY 子句中，应该补全列名
    OrderByClause,
    /// 在 ORDER BY 表达式之后，应该补全排序方向和 NULLS 规则
    OrderDirectionClause,
    /// 在 GROUP BY 子句中，应该补全列名
    GroupByClause,
    /// 在 GROUP BY 表达式之后，应该补全后续分组子句
    GroupByContinuationClause,
    /// 在 HAVING 子句中，应该补全列名和关键字
    HavingClause,
    /// 在 JOIN ... USING (...) 子句中，应该补全可共享的列名
    UsingClause,
    /// 在 REFERENCES table (...) 子句中，应该补全被引用表的列名
    ReferenceColumnClause,
    /// 在 REFERENCES table 之后，应该补全外键引用动作
    ReferenceActionClause,
    /// 在 REFERENCES table ON DELETE/UPDATE 之后，应该补全外键规则
    ReferenceRuleClause,
    /// 在 ALTER TABLE 的列目标位置，应该补全当前表列名
    ColumnTargetClause,
    /// 在 ALTER TABLE 的约束目标位置，应该补全当前表约束名
    ConstraintTargetClause,
    /// 在 ALTER TABLE 表名之后，应该补全结构操作
    AlterTableActionClause,
    /// 在 INSERT INTO 表名之后，应该补全插入动作
    InsertActionClause,
    /// 在 INSERT ... VALUES (...) 值位置，应该补全值相关关键字
    InsertValueClause,
    /// 在 INSERT 值列表或 DEFAULT VALUES 之后，应该补全后续动作
    InsertContinuationClause,
    /// 在 PostgreSQL ON CONFLICT (...) 中，应该补全冲突目标列
    InsertConflictTargetClause,
    /// 在 PostgreSQL ON CONFLICT ON CONSTRAINT 后，应该补全冲突约束
    InsertConflictConstraintClause,
    /// 在 PostgreSQL ON CONFLICT 之后，应该补全冲突处理动作
    InsertConflictActionClause,
    /// 在 UPDATE 表名之后，应该补全更新动作
    UpdateActionClause,
    /// 在 DELETE FROM 表名之后，应该补全删除动作
    DeleteActionClause,
    /// 在索引名目标位置，应该补全 schema 或当前表的索引名
    IndexTargetClause,
    /// 在列定义的数据类型位置，应该补全当前方言的数据类型
    DataTypeClause,
    /// 默认上下文，返回所有关键字
    Default,
}

/// SQL 解析结果
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// 解析后的 AST Tree
    pub tree: Option<Tree>,
    /// 诊断信息
    pub diagnostics: Vec<Diagnostic>,
    /// 解析是否成功（Tree-sitter 总是能生成树，即使有错误）
    pub success: bool,
    /// 原始 SQL 文本
    pub source: String,
}

/// A relation alias visible in the current SQL query scope.
///
/// `name` is normalized for semantic lookup while `sql` preserves the exact
/// identifier spelling and quoting used by the document.  Keeping both avoids
/// changing the meaning of aliases such as PostgreSQL `u` versus `"U"` when a
/// completion item is inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationAlias {
    pub name: String,
    pub sql: String,
    pub relation: String,
}

/// SQL 解析器（基于 Tree-sitter）
pub struct SqlParser {
    pub(super) parser: Parser,
    pub(super) source: String, // 存储当前解析的 SQL 文本
    pub(super) placeholder_dialect: SqlPlaceholderDialect,
}

impl Default for SqlParser {
    fn default() -> Self {
        Self::new()
    }
}

/// AST 节点信息
#[derive(Debug, Clone)]
pub struct AstNode {
    pub node_type: String,
    pub position: Range,
    pub text: String,
}
