use super::normalization::normalize_for_parser;
use super::types::{ParseResult, SqlParser};
use crate::placeholder::SqlPlaceholderDialect;
use crate::token::Delimiters;
use crate::token::Keywords;
use crate::token::Operators;
use crate::token::Token;
use crate::token::TokenType;
use tower_lsp::lsp_types::Diagnostic;
use tower_lsp::lsp_types::DiagnosticSeverity;
use tower_lsp::lsp_types::NumberOrString;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::Range;
use tree_sitter::InputEdit;
use tree_sitter::Node;
use tree_sitter::Parser;
use tree_sitter::Point;
use tree_sitter::Tree;

pub(super) fn point_at_byte(source: &str, byte_offset: usize) -> Point {
    let byte_offset = byte_offset.min(source.len());
    let before = &source.as_bytes()[..byte_offset];
    let row = before.iter().filter(|byte| **byte == b'\n').count();
    let column = before
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(byte_offset, |line_break| byte_offset - line_break - 1);
    Point::new(row, column)
}

pub(super) fn minimal_input_edit(previous: &str, next: &str) -> InputEdit {
    let mut start = previous
        .as_bytes()
        .iter()
        .zip(next.as_bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while start > 0 && (!previous.is_char_boundary(start) || !next.is_char_boundary(start)) {
        start -= 1;
    }

    let max_suffix = previous.len().min(next.len()).saturating_sub(start);
    let mut suffix = previous
        .as_bytes()
        .iter()
        .rev()
        .zip(next.as_bytes().iter().rev())
        .take(max_suffix)
        .take_while(|(left, right)| left == right)
        .count();
    while suffix > 0
        && (!previous.is_char_boundary(previous.len() - suffix)
            || !next.is_char_boundary(next.len() - suffix))
    {
        suffix -= 1;
    }

    let old_end = previous.len() - suffix;
    let new_end = next.len() - suffix;
    InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: point_at_byte(previous, start),
        old_end_position: point_at_byte(previous, old_end),
        new_end_position: point_at_byte(next, new_end),
    }
}

impl SqlParser {
    /// 创建 SQL 解析器
    pub fn new() -> Self {
        Self::new_with_placeholder_dialect(SqlPlaceholderDialect::Generic)
    }

    pub(crate) fn new_with_placeholder_dialect(dialect: SqlPlaceholderDialect) -> Self {
        let language = crate::sql_grammar::language();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("Failed to set SQL language");

        Self {
            parser,
            source: String::new(),
            source_positions: None,
            syntax_tree: None,
            #[cfg(test)]
            syntax_parse_count: 0,
            placeholder_dialect: dialect,
            sqlite_join_scopes: false,
        }
    }

    /// 解析 SQL 语句
    pub fn parse(&mut self, sql: &str) -> ParseResult {
        let tree = self.parse_syntax_tree(sql);

        let mut diagnostics = Vec::new();

        if let Some(tree) = &tree {
            // Tree-sitter 即使有错误也能生成部分树
            // 检查是否有错误节点
            self.collect_errors(tree.root_node(), sql, &mut diagnostics);
            Self::filter_trailing_incomplete_diagnostics(sql, &mut diagnostics);
        } else {
            // 完全无法解析
            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: crate::position::lsp_position_at_end(sql),
                },
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("PARSE_ERROR".to_string())),
                code_description: None,
                source: Some("tree-sitter-sql".to_string()),
                message: "Failed to parse SQL".to_string(),
                related_information: None,
                tags: None,
                data: None,
            });
        }

        ParseResult {
            tree,
            diagnostics,
            success: true, // Tree-sitter 总是能生成树
            source: sql.to_string(),
        }
    }

    /// 收集错误节点
    /// 参考 sqls 的错误处理逻辑：过滤误报，只报告真正的语法错误
    pub fn parse_incremental(
        &mut self,
        sql: &str,
        previous_source: &str,
        previous_tree: &Tree,
    ) -> ParseResult {
        let normalized_sql = normalize_for_parser(sql, self.placeholder_dialect);
        let previous_normalized = normalize_for_parser(previous_source, self.placeholder_dialect);
        let mut edited_tree = previous_tree.clone();
        edited_tree.edit(&minimal_input_edit(&previous_normalized, &normalized_sql));
        #[cfg(test)]
        {
            self.syntax_parse_count += 1;
        }
        let tree = self.parser.parse(&normalized_sql, Some(&edited_tree));
        self.remember_syntax_tree(sql, tree.as_ref());
        let mut diagnostics = Vec::new();
        if let Some(tree) = &tree {
            self.collect_errors(tree.root_node(), sql, &mut diagnostics);
            Self::filter_trailing_incomplete_diagnostics(sql, &mut diagnostics);
        } else {
            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position::new(0, 0),
                    end: crate::position::lsp_position_at_end(sql),
                },
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("PARSE_ERROR".to_string())),
                code_description: None,
                source: Some("tree-sitter-sql".to_string()),
                message: "Failed to parse SQL".to_string(),
                related_information: None,
                tags: None,
                data: None,
            });
        }
        ParseResult {
            tree,
            diagnostics,
            success: true,
            source: sql.to_string(),
        }
    }

    pub(super) fn collect_errors(
        &self,
        node: Node,
        source: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        super::tsql_modules::validate(self, node, source, diagnostics);
        if node.kind() == "join"
            && node
                .named_children(&mut node.walk())
                .any(|n| n.kind() == "keyword_natural")
        {
            // Recovery can interpret USING(id) as the right input's alias and
            // alias-column list. It is still an illegal NATURAL condition.
            if let Some(alias) = node
                .named_children(&mut node.walk())
                .find(|n| n.kind() == "relation")
                .and_then(|n| n.child_by_field_name("alias"))
                .filter(|a| source[a.byte_range()].eq_ignore_ascii_case("USING"))
            {
                diagnostics.push(Diagnostic {
                    range: self.node_range(alias),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String(
                        "INVALID_NATURAL_JOIN_CONDITION".into(),
                    )),
                    source: Some("sql-dialect".into()),
                    message: "NATURAL JOIN cannot have an explicit ON or USING condition".into(),
                    ..Default::default()
                });
            }
        }
        if self.placeholder_dialect == SqlPlaceholderDialect::SqlServer
            && node.kind() == "identifier"
        {
            let raw = &source[node.byte_range()];
            if raw.starts_with('@')
                && !raw.starts_with("@@")
                && !crate::tsql_identifier::is_complete_name(raw)
            {
                diagnostics.push(Diagnostic {range:self.node_range(node),severity:Some(DiagnosticSeverity::ERROR),
                    code:Some(NumberOrString::String("INVALID_TSQL_VARIABLE".into())),source:Some("sql-dialect".into()),
                    message:"T-SQL local names require @ plus BMP letters/digits or _@$#, within 128 UTF-16 units".into(),..Default::default()});
            }
        }
        if node.kind() == "tsql_declaration" {
            let prohibited = node.child_by_field_name("type").is_some_and(|ty| {
                ["text", "ntext", "image"]
                    .iter()
                    .any(|name| source[ty.byte_range()].eq_ignore_ascii_case(name))
            });
            let array = node
                .named_children(&mut node.walk())
                .any(|child| child.kind() == "array_size_definition");
            if prohibited || array {
                diagnostics.push(Diagnostic {
                range:self.node_range(node),severity:Some(DiagnosticSeverity::ERROR),
                code:Some(NumberOrString::String("INVALID_TSQL_VARIABLE_TYPE".into())),source:Some("sql-dialect".into()),
                message:"SQL Server local variables do not support text/ntext/image or array declarations".into(),..Default::default()
            });
            }
        }
        if node.kind() == "tsql_declaration"
            && node.child_by_field_name("name").is_some_and(|name| {
                let raw = &source[name.byte_range()];
                !raw.starts_with('@') || raw.starts_with("@@")
            })
        {
            diagnostics.push(Diagnostic {
                range: self.node_range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("INVALID_TSQL_VARIABLE".into())),
                source: Some("sql-dialect".into()),
                message: "T-SQL local variable declarations require an @name".into(),
                ..Default::default()
            });
        }
        if node.kind() == "execute_argument" || node.kind() == "execute_statement" {
            let bad_name = node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("return_status"))
                .is_some_and(|name| {
                    let raw = &source[name.byte_range()];
                    !raw.starts_with('@') || raw.starts_with("@@")
                });
            let output = node
                .named_children(&mut node.walk())
                .any(|child| matches!(child.kind(), "keyword_out" | "keyword_output"));
            let bad_output = output
                && node.child_by_field_name("value").is_none_or(|value| {
                    let raw = &source[value.byte_range()];
                    value.kind() != "identifier" || !raw.starts_with('@') || raw.starts_with("@@")
                });
            if bad_name || bad_output {
                diagnostics.push(Diagnostic {
                    range: self.node_range(node),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String("INVALID_EXECUTE_ARGUMENT".into())),
                    source: Some("sql-dialect".into()),
                    message: "EXEC parameter/return names and OUTPUT values must be @variables"
                        .into(),
                    ..Default::default()
                });
            }
        }
        if matches!(node.kind(), "execute_statement" | "tsql_declare_statement")
            && self.placeholder_dialect != SqlPlaceholderDialect::SqlServer
        {
            diagnostics.push(Diagnostic {
                range: self.node_range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into())),
                source: Some("sql-dialect".into()),
                message: "Static EXEC/EXECUTE and local DECLARE require the SQL Server profile"
                    .into(),
                ..Default::default()
            });
        }
        if node.kind() == "named_argument"
            && matches!(
                self.placeholder_dialect,
                SqlPlaceholderDialect::Mysql | SqlPlaceholderDialect::SqlServer
            )
        {
            diagnostics.push(Diagnostic {
                range: self.node_range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into())),
                source: Some("sql-dialect".into()),
                message: "Named =>/:= call arguments are not supported by this SQL profile".into(),
                ..Default::default()
            });
        }
        if node.kind() == "invocation" {
            let mut variadic = None;
            let mut count = 0;
            for child in node.named_children(&mut node.walk()) {
                if child.kind() == "keyword_variadic" {
                    count += 1;
                    variadic = Some(child);
                }
            }
            if let Some(marker) = variadic {
                let following = node
                    .children_by_field_name("parameter", &mut node.walk())
                    .filter(|parameter| parameter.start_byte() > marker.start_byte())
                    .count();
                if self.placeholder_dialect != SqlPlaceholderDialect::Postgres
                    || count != 1
                    || following != 1
                {
                    diagnostics.push(Diagnostic {
                        range: self.node_range(node),
                        severity: Some(DiagnosticSeverity::ERROR),
                        code: Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into())),
                        source: Some("sql-dialect".into()),
                        message: "VARIADIC is supported only on the last PostgreSQL call argument"
                            .into(),
                        ..Default::default()
                    });
                }
            }
        }
        if self.placeholder_dialect == SqlPlaceholderDialect::Postgres
            && Self::unsupported_postgres_array_constructor(node)
        {
            diagnostics.push(Diagnostic {
                range: self.node_range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into())),
                source: Some("sql-dialect".into()),
                message: "PostgreSQL requires ARRAY[...] or ARRAY(query)".into(),
                ..Default::default()
            });
        }
        if self.placeholder_dialect != SqlPlaceholderDialect::Postgres
            && Self::is_postfix_array_cast(node)
        {
            diagnostics.push(Diagnostic {
                range: self.node_range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("UNSUPPORTED_DIALECT_SYNTAX".into())),
                source: Some("sql-dialect".into()),
                message: "Postfix array casts require the PostgreSQL dialect".into(),
                ..Default::default()
            });
        }
        // The grammar can consume a bare FROM as the entire SELECT field,
        // hiding the missing projection instead of producing an ERROR node.
        if node.kind() == "field"
            && node
                .utf8_text(source.as_bytes())
                .is_ok_and(|text| text.eq_ignore_ascii_case("FROM"))
            && node
                .parent()
                .and_then(|parent| parent.parent())
                .is_some_and(|parent| parent.kind() == "select_expression")
        {
            diagnostics.push(Diagnostic {
                range: self.node_range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("PARSE_ERROR".into())),
                source: Some("tree-sitter-sql".into()),
                message: "Expected a SELECT expression before FROM".into(),
                ..Default::default()
            });
        }
        // Tree-sitter can hide a missing _identifier under a zero-width
        // identifier leaf: has_error is true but is_missing is false there.
        let missing = node.is_missing()
            || (node.has_error()
                && node.child_count() == 0
                && node.start_byte() == node.end_byte());
        // 检查是否是错误节点
        if node.is_error() || missing {
            let start_byte = node.start_byte();
            let end_byte = node.end_byte();
            let start_point = node.start_position();
            let end_point = node.end_position();

            // 获取节点文本
            let node_text = if start_byte < source.len() && end_byte <= source.len() {
                &source[start_byte..end_byte]
            } else {
                ""
            };

            // 参考 sqls：过滤常见的误报情况

            // 1. SELECT * 中的 * 是有效的
            if node_text.trim() == "*" && self.is_in_select_context(node, source) {
                // 跳过这个错误，* 在 SELECT 中是有效的
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.collect_errors(child, source, diagnostics);
                }
                return;
            }

            // 2. 过滤空白字符错误（格式问题，不是语法错误）
            if node_text.trim().is_empty() && !missing {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.collect_errors(child, source, diagnostics);
                }
                return;
            }

            // 3. 过滤已知的有效语法模式
            // 例如：某些方言的特殊语法可能被 tree-sitter-sql 误判
            if self.is_valid_syntax_pattern(node, source) {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.collect_errors(child, source, diagnostics);
                }
                return;
            }

            diagnostics.push(Diagnostic {
                range: Range {
                    start: Self::tree_sitter_point_to_lsp_position(source, start_point),
                    end: Self::tree_sitter_point_to_lsp_position(source, end_point),
                },
                severity: Some(if node.is_error() {
                    DiagnosticSeverity::ERROR
                } else {
                    DiagnosticSeverity::WARNING
                }),
                code: Some(NumberOrString::String("SYNTAX_ERROR".to_string())),
                code_description: None,
                source: Some("tree-sitter-sql".to_string()),
                message: if node.is_error() {
                    format!("Syntax error: {}", node_text)
                } else {
                    "Missing syntax element".to_string()
                },
                related_information: None,
                tags: None,
                data: None,
            });
        }

        // 递归检查子节点
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_errors(child, source, diagnostics);
        }
    }

    /// Convert an LSP UTF-16 position to the byte-based column expected by tree-sitter.
    pub fn lsp_position_to_byte_position(source: &str, position: Position) -> Position {
        crate::position::lsp_position_to_byte_position(source, position)
    }

    pub(super) fn tree_sitter_point_to_lsp_position(
        source: &str,
        point: tree_sitter::Point,
    ) -> Position {
        crate::position::byte_position_to_lsp_position(
            source,
            Position {
                line: point.row as u32,
                character: point.column as u32,
            },
        )
    }

    pub(super) fn filter_trailing_incomplete_diagnostics(
        source: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if !Self::is_trailing_incomplete_statement(source) {
            return;
        }

        let eof = Self::position_at_end(source);
        diagnostics.retain(|diagnostic| !Self::diagnostic_reaches_position(diagnostic, eof));
    }

    pub(super) fn position_at_end(source: &str) -> Position {
        crate::position::lsp_position_at_end(source)
    }

    pub(super) fn diagnostic_reaches_position(diagnostic: &Diagnostic, position: Position) -> bool {
        diagnostic.range.end.line > position.line
            || (diagnostic.range.end.line == position.line
                && diagnostic.range.end.character >= position.character.saturating_sub(1))
            || (diagnostic.range.start.line == position.line
                && diagnostic.range.start.character >= position.character.saturating_sub(1))
    }

    pub(super) fn is_trailing_incomplete_statement(source: &str) -> bool {
        let searchable_source = Self::mask_sql_noise(source);
        let trimmed = searchable_source.trim_end();
        if trimmed.is_empty() || trimmed.ends_with(';') {
            return false;
        }

        let source_upper = trimmed.to_ascii_uppercase();
        let statement_start = source_upper
            .rfind(';')
            .map(|position| position + 1)
            .unwrap_or(0);
        let statement = source_upper[statement_start..].trim();
        if statement.is_empty() {
            return false;
        }

        let words = Self::statement_words(statement);
        let incomplete_phrases: &[&[&str]] = &[
            &["SELECT"],
            &["SELECT", "DISTINCT"],
            &["WITH"],
            &["FROM"],
            &["JOIN"],
            &["INNER", "JOIN"],
            &["LEFT", "JOIN"],
            &["RIGHT", "JOIN"],
            &["FULL", "JOIN"],
            &["CROSS", "JOIN"],
            &["ON"],
            &["USING"],
            &["WHERE"],
            &["AND"],
            &["OR"],
            &["NOT"],
            &["GROUP", "BY"],
            &["ORDER", "BY"],
            &["HAVING"],
            &["LIMIT"],
            &["OFFSET"],
            &["INSERT"],
            &["INSERT", "INTO"],
            &["UPDATE"],
            &["DELETE", "FROM"],
            &["CREATE", "TABLE"],
            &["ALTER", "TABLE"],
            &["DROP", "TABLE"],
            &["SET"],
            &["VALUES"],
        ];

        if incomplete_phrases
            .iter()
            .any(|phrase| Self::words_end_with(&words, phrase))
        {
            return true;
        }

        let last_char = statement.chars().rev().find(|ch| !ch.is_whitespace());
        matches!(
            last_char,
            Some(',' | '.' | '(' | '=' | '<' | '>' | '!' | '+' | '-' | '/' | '%')
        )
    }

    pub(super) fn statement_words(statement_upper: &str) -> Vec<&str> {
        statement_upper
            .split(|ch: char| !Self::is_identifier_char(ch))
            .filter(|word| !word.is_empty())
            .collect()
    }

    pub(super) fn words_end_with(words: &[&str], phrase: &[&str]) -> bool {
        words.len() >= phrase.len()
            && words[words.len() - phrase.len()..]
                .iter()
                .zip(phrase.iter())
                .all(|(word, expected)| word == expected)
    }

    /// 检查节点是否在 SELECT 上下文中
    pub(super) fn is_in_select_context(&self, node: Node, source: &str) -> bool {
        let mut current = Some(node);
        while let Some(n) = current {
            let kind = n.kind();
            if kind == "select_list"
                || kind == "select_expression_list"
                || kind == "select_statement"
                || kind == "select"
                || kind == "query"
            {
                return true;
            }
            if let Ok(text) = n.utf8_text(source.as_bytes()) {
                if text.to_uppercase().contains("SELECT") {
                    return true;
                }
            }
            current = n.parent();
        }
        false
    }

    /// 检查是否是有效的语法模式（参考 sqls 的容错处理）
    pub(super) fn is_valid_syntax_pattern(&self, node: Node, source: &str) -> bool {
        // 检查是否是已知的有效语法模式
        // 例如：某些方言的特殊语法

        // 检查节点类型和上下文
        let node_kind = node.kind();

        // 某些节点类型即使被标记为错误，也可能是有效的
        // 这取决于具体的 SQL 方言
        match node_kind {
            // 这些节点类型在某些情况下可能是有效的
            "identifier" | "expression" | "literal" => {
                // 检查上下文，如果是合理的语法位置，可能是误报
                self.has_reasonable_context(node, source)
            }
            _ => false,
        }
    }

    /// 检查节点是否有合理的上下文（不是真正的语法错误）
    pub(super) fn has_reasonable_context(&self, node: Node, _source: &str) -> bool {
        // 检查父节点和兄弟节点，判断是否是合理的语法位置
        if let Some(parent) = node.parent() {
            let parent_kind = parent.kind();
            // 如果父节点是合理的容器节点，可能是误报
            matches!(
                parent_kind,
                "select_list"
                    | "expression"
                    | "where_clause"
                    | "order_by_clause"
                    | "group_by_clause"
                    | "having_clause"
                    | "table_reference"
                    | "column_reference"
            )
        } else {
            false
        }
    }

    /// 提取所有 Token（参考 sqls 的 tokenizer）
    pub fn tokenize(&self, tree: &Tree, source: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        self.tokenize_recursive(tree.root_node(), source, &mut tokens);
        tokens
    }

    /// 递归提取 Token
    pub(super) fn tokenize_recursive(&self, node: Node, source: &str, tokens: &mut Vec<Token>) {
        let node_kind = node.kind();
        let start_point = node.start_position();

        {
            let text = Self::source_node_text(node, source).trim();
            if !text.is_empty() {
                let token_type = self.classify_token(node_kind, text);
                let position = Self::tree_sitter_point_to_lsp_position(source, start_point);
                tokens.push(Token::new(token_type, text.to_string(), position));
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.tokenize_recursive(child, source, tokens);
        }
    }

    /// 分类 Token 类型（参考 sqls 的 token 分类逻辑）
    pub(super) fn classify_token(&self, node_kind: &str, text: &str) -> TokenType {
        // 检查是否是关键字
        if Keywords::is_keyword(text) {
            return TokenType::Keyword;
        }

        // 检查是否是操作符
        if Operators::is_operator(text) {
            return TokenType::Operator;
        }

        // 检查是否是分隔符
        if Delimiters::is_delimiter(text) {
            return TokenType::Delimiter;
        }

        // 根据节点类型分类
        match node_kind {
            "string" | "string_literal" => TokenType::String,
            "number" | "numeric_literal" => TokenType::Number,
            "identifier" | "table_name" | "column_name" => TokenType::Identifier,
            "comment" => TokenType::Comment,
            _ => TokenType::Unknown,
        }
    }

    /// 获取指定位置的节点
    pub fn get_node_at_position<'a>(&self, tree: &'a Tree, position: Position) -> Option<Node<'a>> {
        let root = tree.root_node();
        let row = position.line as usize;
        let col = position.character as usize;

        // Try exact position
        let point = tree_sitter::Point { row, column: col };
        let node = root.descendant_for_point_range(point, point);

        // If we got the root node (and we are not at 0,0), it usually means we are at the end of a token or file
        // and missed the specific node. Try moving back 1 char.
        if let Some(n) = node {
            if n.kind() == "program" && col > 0 {
                let point_prev = tree_sitter::Point {
                    row,
                    column: col - 1,
                };
                return root.descendant_for_point_range(point_prev, point_prev);
            }
            return Some(n);
        }

        node
    }

    /// 提取查询中的表名
    pub fn extract_tables(&self, tree: &Tree, source: &str) -> Vec<String> {
        let mut tables = Vec::new();
        self.extract_tables_recursive(tree.root_node(), source, &mut tables);
        tables
    }

    /// 递归提取表名
    /// 参考 sqls 的实现：查找 FROM/JOIN 子句中的表名
    pub(super) fn extract_tables_recursive(
        &self,
        node: Node,
        source: &str,
        tables: &mut Vec<String>,
    ) {
        let node_kind = node.kind();

        // 参考 sqls：查找 table_name, table_reference, table_identifier 等节点
        if node_kind == "table_name"
            || node_kind == "table_reference"
            || node_kind == "table_identifier"
            || node_kind == "table"
            || (node_kind == "identifier" && self.is_in_from_context(node, source))
        {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                let text = text.trim();
                let table_name = Self::normalize_identifier(text);
                // 过滤关键字和操作符
                if !text.is_empty()
                    && !Keywords::is_keyword(text)
                    && !Operators::is_operator(text)
                    && !Delimiters::is_delimiter(text)
                    && !table_name.is_empty()
                    && !tables.contains(&table_name)
                {
                    tables.push(table_name);
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.extract_tables_recursive(child, source, tables);
        }
    }

    /// 检查节点是否在 FROM/JOIN 上下文中
    pub fn is_in_from_context(&self, node: Node, source: &str) -> bool {
        let mut current = Some(node);
        while let Some(n) = current {
            let kind = n.kind();
            // 检查是否是 FROM/JOIN 相关的节点
            if kind == "from_clause"
                || kind == "join_clause"
                || kind == "table_reference"
                || kind == "table_expression"
            {
                return true;
            }
            // 检查父节点文本是否包含 FROM/JOIN
            if let Ok(text) = n.utf8_text(source.as_bytes()) {
                let upper = text.to_uppercase();
                if upper.contains("FROM") || upper.contains("JOIN") {
                    return true;
                }
            }
            current = n.parent();
        }
        false
    }

    /// 提取查询中的列名
    pub fn extract_columns(&self, tree: &Tree, source: &str) -> Vec<String> {
        let mut columns = Vec::new();
        self.extract_columns_recursive(tree.root_node(), source, &mut columns);
        columns
    }

    /// 递归提取列名
    /// 参考 sqls 的实现：查找 SELECT/WHERE/ORDER BY 等子句中的列名
    pub(super) fn extract_columns_recursive(
        &self,
        node: Node,
        source: &str,
        columns: &mut Vec<String>,
    ) {
        let node_kind = node.kind();

        // 参考 sqls：查找 column_name, column_reference, column_identifier 等节点
        if node_kind == "column_name"
            || node_kind == "column_reference"
            || node_kind == "column_identifier"
            || node_kind == "column"
            || (node_kind == "identifier" && self.is_in_column_context(node, source))
        {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                let text = text.trim();
                let column_name = Self::normalize_identifier(text);
                // 过滤关键字和操作符
                if !text.is_empty()
                    && !Keywords::is_keyword(text)
                    && !Operators::is_operator(text)
                    && !Delimiters::is_delimiter(text)
                    && text != "*"  // 排除通配符
                    && !column_name.is_empty()
                    && !columns.contains(&column_name)
                {
                    columns.push(column_name);
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.extract_columns_recursive(child, source, columns);
        }
    }

    /// 检查节点是否在列上下文中（SELECT/WHERE/ORDER BY 等）
    pub fn is_in_column_context(&self, node: Node, source: &str) -> bool {
        let mut current = Some(node);
        while let Some(n) = current {
            let kind = n.kind();
            // 检查是否是列相关的节点
            if kind == "select_list"
                || kind == "select_expression"
                || kind == "where_clause"
                || kind == "order_by_clause"
                || kind == "group_by_clause"
                || kind == "having_clause"
                || kind == "column_reference"
            {
                return true;
            }
            // 检查父节点文本是否包含 SELECT/WHERE/ORDER 等
            if let Ok(text) = n.utf8_text(source.as_bytes()) {
                let upper = text.to_uppercase();
                if upper.contains("SELECT")
                    || upper.contains("WHERE")
                    || upper.contains("ORDER")
                    || upper.contains("GROUP")
                    || upper.contains("HAVING")
                {
                    return true;
                }
            }
            current = n.parent();
        }
        false
    }

    /// 获取节点的文本内容
    pub fn node_text(&self, node: Node, source: &str) -> String {
        Self::source_node_text(node, source).to_string()
    }

    /// 获取节点的范围
    pub fn node_range(&self, node: Node) -> Range {
        let start = node.start_position();
        let end = node.end_position();
        let position = |point: Point| {
            self.source_positions.as_ref().map_or_else(
                || Self::tree_sitter_point_to_lsp_position(&self.source, point),
                |index| {
                    index.position(
                        &self.source,
                        Position {
                            line: point.row as u32,
                            character: point.column as u32,
                        },
                    )
                },
            )
        };
        let offset = |at| {
            self.source_positions.as_ref().map_or_else(
                || crate::position::lsp_position_at_end(&self.source[..at]),
                |index| index.offset(&self.source, at),
            )
        };
        let end = if node.kind() == "keyword_cast" {
            Self::try_cast_name_range(node, &self.source).map(|(_, end)| offset(end))
        } else {
            Self::apply_keyword_range(node, &self.source)
                .or_else(|| Self::search_operator_range(node, &self.source))
                .map(|(_, end)| offset(end))
        }
        .unwrap_or_else(|| position(end));
        Range {
            start: position(start),
            end,
        }
    }
}
