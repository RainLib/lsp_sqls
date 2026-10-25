use super::types::{CompletionContext, SqlParser};
use tower_lsp::lsp_types::Position;
use tree_sitter::Node;

impl SqlParser {
    /// 分析补全上下文 (Text-based heuristics for reliability)
    /// 分析补全上下文 (AST-based)
    /// Uses Tree-sitter AST node traversal for robust context detection
    pub fn analyze_completion_context(
        &self,
        node: Node,
        source: &str,
        position: Position,
    ) -> CompletionContext {
        if let Some(context) =
            Self::qualified_member_context(source, position, self.placeholder_dialect)
        {
            return context;
        }
        let scoped = Self::completion_context_source(source, position);
        let source = scoped.as_ref();
        if Self::expression_value_context_at_position(source, position) {
            return CompletionContext::ExpressionValueClause;
        }

        if Self::predicate_continuation_context_at_position(source, position) {
            return CompletionContext::PredicateContinuationClause;
        }

        if Self::simple_case_when_value_context_at_position(source, position) {
            return CompletionContext::CaseResultClause;
        }

        if Self::simple_case_when_value_continuation_context_at_position(source, position) {
            return CompletionContext::CaseWhenValueContinuationClause;
        }

        if Self::case_when_condition_context_at_position(source, position) {
            return CompletionContext::WhereClause;
        }

        if Self::case_result_context_at_position(source, position) {
            return CompletionContext::CaseResultClause;
        }

        if Self::case_continuation_context_at_position(source, position) {
            return CompletionContext::CaseContinuationClause;
        }

        if Self::select_continuation_context_at_position(source, position) {
            return CompletionContext::SelectContinuationClause;
        }

        if let Some(context) = Self::analyze_completed_keyword_context(source, position) {
            return context;
        }

        if Self::qualify_query_range(source, Self::byte_offset_for_position(source, position))
            .is_some()
        {
            return CompletionContext::WhereClause;
        }

        if Self::join_using_column_context_at_position(source, position) {
            return CompletionContext::UsingClause;
        }

        if Self::order_direction_context_at_position(source, position) {
            return CompletionContext::OrderDirectionClause;
        }

        if Self::group_by_continuation_context_at_position(source, position) {
            return CompletionContext::GroupByContinuationClause;
        }

        if Self::reference_column_context_at_position(source, position) {
            return CompletionContext::ReferenceColumnClause;
        }

        if Self::reference_rule_context_at_position(source, position) {
            return CompletionContext::ReferenceRuleClause;
        }

        if Self::reference_action_context_at_position(source, position) {
            return CompletionContext::ReferenceActionClause;
        }

        if Self::reference_relation_target_context_at_position(source, position) {
            return CompletionContext::FromClause;
        }

        if Self::data_type_context_at_position(source, position) {
            return CompletionContext::DataTypeClause;
        }

        if let Some(context) = Self::analyze_ddl_target_context_at_position(source, position) {
            return context;
        }

        if Self::alter_table_action_context_at_position(source, position) {
            return CompletionContext::AlterTableActionClause;
        }

        if let Some(context) = Self::analyze_dml_action_context_at_position(source, position) {
            return context;
        }

        if Self::insert_value_context_at_position(source, position) {
            return CompletionContext::InsertValueClause;
        }

        if Self::insert_conflict_target_context_at_position(source, position) {
            return CompletionContext::InsertConflictTargetClause;
        }

        if Self::insert_conflict_constraint_context_at_position(source, position) {
            return CompletionContext::InsertConflictConstraintClause;
        }

        if Self::insert_conflict_action_context_at_position(source, position) {
            return CompletionContext::InsertConflictActionClause;
        }

        if Self::insert_continuation_context_at_position(source, position) {
            return CompletionContext::InsertContinuationClause;
        }

        if let Some(context) =
            Self::analyze_relation_continuation_context_at_position(source, position)
        {
            return context;
        }

        let mut current_node = Some(node);

        // First, check if we are inside a specific node type that dictates context directly
        // Usually we want to find the clause we are in (SELECT, FROM, WHERE, etc.)
        while let Some(n) = current_node {
            match n.kind() {
                "join"
                    if n.child_by_field_name("predicate").is_some_and(|predicate| {
                        let cursor = Self::byte_offset_for_position(source, position);
                        n.named_children(&mut n.walk())
                            .find(|n| n.kind() == "keyword_on")
                            .is_some_and(|on| on.end_byte() <= cursor)
                            && cursor <= predicate.end_byte()
                    }) =>
                {
                    return CompletionContext::WhereClause
                }
                "subquery" => break,
                // SELECT clause
                "select_clause" | "select_list" => {
                    // Check if we are in a column position or after a dot
                    // (This might need refinement, but select_clause generally means column completion)
                    return CompletionContext::SelectClause;
                }
                // FROM clause
                "from_clause" | "table_references" => {
                    return CompletionContext::FromClause;
                }
                // JOIN clause
                // Tree-sitter sql often structures joins inside table_references or as specific join nodes
                // Depending on the exact grammar structure.
                // Assuming "join_clause" or similar if available, or fallback to heuristics if tree-sitter is murky here.
                // Note: tree-sitter-sql often puts joins in `table_expression` or `joined_table`
                "joined_table" => {
                    // Verify if we are at the ON part or table part
                    // For now, treat as JoinClause
                    return CompletionContext::JoinClause;
                }
                // WHERE clause
                "where_clause" => {
                    return CompletionContext::WhereClause;
                }
                // ORDER BY clause
                "order_by_clause" => {
                    return CompletionContext::OrderByClause;
                }
                // GROUP BY clause
                "group_by_clause" => {
                    return CompletionContext::GroupByClause;
                }
                // HAVING clause
                "having_clause" => {
                    return CompletionContext::HavingClause;
                }
                "using_clause" => {
                    return CompletionContext::UsingClause;
                }
                // If we hit the statement level, we might be in a specific position
                "select_statement" => {
                    // If we traversed up to statement without hitting a clause,
                    // we might be in an empty space between clauses or at the end.
                    // Fallback or check children?
                    // For robustness, let's keep searching up or break if root.
                }
                _ => {}
            }
            current_node = n.parent();
        }

        // Fallback: Use simple heuristics if AST traversal didn't find a specific clause
        // This handles cases where Syntax is broken (common during typing) and AST is incomplete
        self.analyze_completion_context_fallback(source, position)
    }

    pub(crate) fn byte_offset_for_position(source: &str, position: Position) -> usize {
        // Dialect entry points convert LSP UTF-16 positions to tree-sitter byte
        // columns exactly once. Parser context helpers must not reinterpret the
        // byte column as UTF-16, especially after emoji or non-ASCII literals.
        let mut line_start = 0usize;
        for (line_index, line) in source.split_inclusive('\n').enumerate() {
            if line_index == position.line as usize {
                let mut offset = (line_start + position.character as usize).min(source.len());
                while offset > line_start && !source.is_char_boundary(offset) {
                    offset -= 1;
                }
                return offset;
            }
            line_start += line.len();
        }
        source.len()
    }

    pub(super) fn analyze_completed_keyword_context(
        source: &str,
        position: Position,
    ) -> Option<CompletionContext> {
        let cursor_offset = Self::byte_offset_for_position(source, position);
        let text_before = source.get(..cursor_offset).unwrap_or(source);

        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement = text_upper[statement_start..].trim_end();
        if statement.is_empty() {
            return None;
        }

        if text_before.trim_end().ends_with('.') {
            if Self::is_dotted_relation_target_context(statement) {
                return Some(CompletionContext::FromClause);
            }
            return Some(CompletionContext::TableColumn);
        }

        let words = Self::statement_words(statement);
        if Self::words_end_with(&words, &["SELECT"])
            || Self::words_end_with(&words, &["SELECT", "DISTINCT"])
        {
            return Some(CompletionContext::SelectClause);
        }
        if Self::words_end_with(&words, &["FROM"]) {
            return Some(CompletionContext::FromClause);
        }
        if Self::words_end_with(&words, &["ON", "DUPLICATE", "KEY", "UPDATE"])
            && Self::is_insert_duplicate_update_assignment_context(statement)
        {
            return Some(CompletionContext::WhereClause);
        }
        if Self::words_end_with(&words, &["INSERT", "INTO"])
            || Self::words_end_with(&words, &["UPDATE"])
            || Self::words_end_with(&words, &["DELETE", "FROM"])
            || Self::words_end_with(&words, &["TRUNCATE", "TABLE"])
            || Self::words_end_with(&words, &["ALTER", "TABLE"])
            || Self::words_end_with(&words, &["DROP", "TABLE"])
            || Self::words_end_with(&words, &["DROP", "VIEW"])
        {
            return Some(CompletionContext::FromClause);
        }
        if Self::words_end_with(&words, &["JOIN"])
            || Self::words_end_with(&words, &["INNER", "JOIN"])
            || Self::words_end_with(&words, &["LEFT", "JOIN"])
            || Self::words_end_with(&words, &["RIGHT", "JOIN"])
            || Self::words_end_with(&words, &["FULL", "JOIN"])
            || Self::words_end_with(&words, &["CROSS", "JOIN"])
        {
            return Some(CompletionContext::JoinClause);
        }
        if Self::words_end_with(&words, &["ON"]) {
            if Self::ddl_on_relation_target_at_position(source, position) {
                return Some(CompletionContext::FromClause);
            }
            return Some(CompletionContext::WhereClause);
        }
        if Self::words_end_with(&words, &["USING"]) && Self::is_join_using_column_context(statement)
        {
            return Some(CompletionContext::UsingClause);
        }
        if Self::words_end_with(&words, &["WHEN"])
            && Self::is_simple_case_when_value_context(statement)
        {
            return Some(CompletionContext::CaseResultClause);
        }
        if Self::words_end_with(&words, &["WHERE"])
            || Self::words_end_with(&words, &["AND"])
            || Self::words_end_with(&words, &["OR"])
            || Self::words_end_with(&words, &["NOT"])
            || Self::words_end_with(&words, &["WHEN"])
            || Self::words_end_with(&words, &["SET"])
        {
            return Some(CompletionContext::WhereClause);
        }
        if Self::words_end_with(&words, &["RETURNING"]) {
            return Some(CompletionContext::SelectClause);
        }
        if Self::words_end_with(&words, &["ORDER", "BY"]) {
            return Some(CompletionContext::OrderByClause);
        }
        if Self::words_end_with(&words, &["GROUP", "BY"]) {
            return Some(CompletionContext::GroupByClause);
        }
        if Self::words_end_with(&words, &["HAVING"]) {
            return Some(CompletionContext::HavingClause);
        }
        if Self::words_end_with(&words, &["THEN"]) || Self::words_end_with(&words, &["ELSE"]) {
            return Some(CompletionContext::CaseResultClause);
        }

        None
    }

    /// Fallback heuristics for context analysis when AST is incomplete
    pub(super) fn analyze_completion_context_fallback(
        &self,
        source: &str,
        position: Position,
    ) -> CompletionContext {
        if let Some(context) =
            Self::qualified_member_context(source, position, self.placeholder_dialect)
        {
            return context;
        }
        Self::completion_context_from_text(source, position)
    }

    /// Determine completion context through the bounded text heuristics only.
    /// Callers that already paid for dialect parsing can use this without
    /// constructing a second tree-sitter parser on the typing hot path.
    pub fn completion_context_from_text(source: &str, position: Position) -> CompletionContext {
        if let Some(context) = Self::qualified_member_context(
            source,
            position,
            crate::placeholder::SqlPlaceholderDialect::Generic,
        ) {
            return context;
        }
        let scoped = Self::completion_context_source(source, position);
        let source = scoped.as_ref();
        let cursor_offset = Self::byte_offset_for_position(source, position);
        // Extract text before cursor
        let text_before = if cursor_offset <= source.len() {
            &source[..cursor_offset]
        } else {
            source
        };
        let searchable_text_before = Self::mask_sql_noise(text_before);
        let text_upper = searchable_text_before.to_ascii_uppercase();
        let raw_text_upper = text_before.to_ascii_uppercase();
        let statement_start = Self::previous_statement_start(&text_upper, text_upper.len());
        let statement_upper = &text_upper[statement_start..];
        let raw_statement_upper = &raw_text_upper[statement_start..];
        let words = Self::statement_words(statement_upper);

        // Priority 1: Check for table/alias column access (ends with .)
        if text_before.trim_end().ends_with('.') {
            if Self::is_dotted_relation_target_context(statement_upper) {
                return CompletionContext::FromClause;
            }
            return CompletionContext::TableColumn;
        }

        if Self::is_insert_column_context(statement_upper) {
            return CompletionContext::SelectClause;
        }

        if Self::is_ddl_on_column_context(statement_upper) {
            return CompletionContext::SelectClause;
        }

        if Self::is_join_using_column_context(statement_upper) {
            return CompletionContext::UsingClause;
        }

        if Self::is_order_direction_context(statement_upper) {
            return CompletionContext::OrderDirectionClause;
        }

        if Self::is_group_by_continuation_context(statement_upper) {
            return CompletionContext::GroupByContinuationClause;
        }

        if Self::is_reference_column_context(statement_upper) {
            return CompletionContext::ReferenceColumnClause;
        }

        if Self::is_reference_rule_context(statement_upper) {
            return CompletionContext::ReferenceRuleClause;
        }

        if Self::is_reference_action_context(statement_upper) {
            return CompletionContext::ReferenceActionClause;
        }

        if Self::is_reference_relation_target_context(statement_upper) {
            return CompletionContext::FromClause;
        }

        if Self::is_data_type_context(statement_upper) {
            return CompletionContext::DataTypeClause;
        }

        if let Some(context) = Self::analyze_ddl_target_context(statement_upper) {
            return context;
        }

        if Self::is_alter_table_action_context(statement_upper) {
            return CompletionContext::AlterTableActionClause;
        }

        if let Some(context) = Self::analyze_dml_action_context(statement_upper) {
            return context;
        }

        if Self::is_insert_value_context(statement_upper) {
            return CompletionContext::InsertValueClause;
        }

        if Self::is_insert_continuation_context(raw_statement_upper) {
            return CompletionContext::InsertContinuationClause;
        }

        if Self::is_insert_conflict_target_context(raw_statement_upper) {
            return CompletionContext::InsertConflictTargetClause;
        }

        if Self::is_insert_conflict_constraint_context(raw_statement_upper) {
            return CompletionContext::InsertConflictConstraintClause;
        }

        if Self::is_insert_conflict_action_context(raw_statement_upper) {
            return CompletionContext::InsertConflictActionClause;
        }

        if Self::is_insert_set_assignment_context(raw_statement_upper) {
            return CompletionContext::WhereClause;
        }

        if Self::is_insert_duplicate_update_assignment_context(raw_statement_upper) {
            return CompletionContext::WhereClause;
        }

        if Self::is_expression_value_context(statement_upper, raw_statement_upper) {
            return CompletionContext::ExpressionValueClause;
        }

        if Self::is_predicate_continuation_context(statement_upper, raw_statement_upper) {
            return CompletionContext::PredicateContinuationClause;
        }

        if Self::is_simple_case_when_value_context(raw_statement_upper) {
            return CompletionContext::CaseResultClause;
        }

        if Self::is_simple_case_when_value_continuation_context(raw_statement_upper) {
            return CompletionContext::CaseWhenValueContinuationClause;
        }

        if Self::words_end_with(&words, &["WHEN"]) {
            return CompletionContext::WhereClause;
        }
        if Self::is_case_result_context(raw_statement_upper) {
            return CompletionContext::CaseResultClause;
        }

        if Self::is_case_continuation_context(raw_statement_upper) {
            return CompletionContext::CaseContinuationClause;
        }

        if Self::is_update_set_context(statement_upper) {
            return CompletionContext::WhereClause;
        }

        if let Some(context) = Self::analyze_relation_continuation_context(statement_upper) {
            return context;
        }

        if Self::is_select_continuation_context(raw_statement_upper) {
            return CompletionContext::SelectContinuationClause;
        }

        if Self::is_ddl_on_relation_target_context(statement_upper) {
            return CompletionContext::FromClause;
        }

        // The AST path already recognizes these completed condition markers.
        // Keep the text fallback consistent without rescanning its noise mask.
        if Self::words_end_with(&words, &["ON"]) {
            return CompletionContext::WhereClause;
        }
        if Self::is_relation_target_context(statement_upper) {
            return CompletionContext::FromClause;
        }

        // Priority 2: Find the last complete keyword to determine context

        // Check for WHERE clause
        if let Some(where_pos) = Self::previous_keyword_position(&text_upper, "WHERE") {
            let has_later_keyword = Self::statement_has_any_keyword(
                &text_upper,
                where_pos + "WHERE".len(),
                text_upper.len(),
                &["ORDER BY", "GROUP BY", "LIMIT", "HAVING"],
            );

            if !has_later_keyword {
                return CompletionContext::WhereClause;
            }
        }

        // Check for JOIN clause (basic check)
        if let Some(join_pos) = Self::previous_keyword_position(&text_upper, "JOIN") {
            let after_join = &text_upper[join_pos + 4..].trim_start();
            if !after_join.starts_with("ON") && !after_join.contains(" ON ") {
                return CompletionContext::JoinClause;
            }
        }

        let latest_grouping_clause = [
            Self::previous_keyword_position(&text_upper, "GROUP BY")
                .map(|position| (position, CompletionContext::GroupByClause)),
            Self::previous_keyword_position(&text_upper, "HAVING")
                .map(|position| (position, CompletionContext::HavingClause)),
            Self::previous_keyword_position(&text_upper, "ORDER BY")
                .map(|position| (position, CompletionContext::OrderByClause)),
        ]
        .into_iter()
        .flatten()
        .max_by_key(|(position, _)| *position);

        if let Some((_, context)) = latest_grouping_clause {
            return context;
        }

        // Check for FROM clause
        if let Some(from_pos) = Self::previous_keyword_position(&text_upper, "FROM") {
            let has_later_keyword = Self::statement_has_any_keyword(
                &text_upper,
                from_pos + "FROM".len(),
                text_upper.len(),
                &["WHERE", "JOIN", "ORDER BY", "GROUP BY", "LIMIT"],
            );

            if !has_later_keyword {
                return CompletionContext::FromClause;
            }
        }

        // Check for SELECT clause
        if let Some(select_pos) = Self::previous_keyword_position(&text_upper, "SELECT") {
            if !Self::contains_keyword_between(
                &text_upper,
                "FROM",
                select_pos + "SELECT".len(),
                text_upper.len(),
            ) {
                return CompletionContext::SelectClause;
            }
        }

        CompletionContext::Default
    }
}
