use crate::schema::{Column, Constraint, Function, Index, Schema, Table};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Documentation};

pub(crate) fn completion_sort_text(sort_prefix: &str, label: &str) -> String {
    format!("{}:{}", sort_prefix, label.to_ascii_lowercase())
}

pub(crate) fn set_completion_sort_text(item: &mut CompletionItem, sort_prefix: &str, label: &str) {
    item.sort_text = Some(completion_sort_text(sort_prefix, label));
}

pub(crate) fn create_keyword_item(dialect_label: &str, keyword: &str) -> CompletionItem {
    CompletionItem {
        label: keyword.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        detail: Some(format!("{dialect_label} keyword: {keyword}")),
        documentation: None,
        deprecated: None,
        preselect: None,
        sort_text: Some(completion_sort_text("0", keyword)),
        filter_text: None,
        insert_text: Some(keyword.to_string()),
        insert_text_format: None,
        insert_text_mode: None,
        text_edit: None,
        additional_text_edits: None,
        commit_characters: None,
        command: None,
        data: None,
        tags: None,
        label_details: None,
    }
}

pub(crate) fn create_operator_item(operator: &str, sort_prefix: &str) -> CompletionItem {
    CompletionItem {
        label: operator.to_string(),
        kind: Some(CompletionItemKind::OPERATOR),
        detail: Some(format!("Operator: {}", operator)),
        documentation: None,
        deprecated: None,
        preselect: None,
        sort_text: Some(completion_sort_text(sort_prefix, operator)),
        filter_text: None,
        insert_text: Some(operator.to_string()),
        insert_text_format: None,
        insert_text_mode: None,
        text_edit: None,
        additional_text_edits: None,
        commit_characters: None,
        command: None,
        data: None,
        tags: None,
        label_details: None,
    }
}

pub(crate) fn add_operator_items(
    items: &mut Vec<CompletionItem>,
    operators: &[&str],
    prefix: &str,
    sort_prefix: &str,
) {
    for operator in operators {
        if !prefix.is_empty() && !operator.to_lowercase().starts_with(prefix) {
            continue;
        }
        items.push(create_operator_item(operator, sort_prefix));
    }
}

pub(crate) fn create_table_item(
    table: &Table,
    schema: &Schema,
    qualify_with_database: bool,
) -> CompletionItem {
    let label = if qualify_with_database && !schema.database.is_empty() {
        format!("{}.{}", schema.database, table.name)
    } else {
        table.name.clone()
    };
    let detail = if qualify_with_database && !schema.database.is_empty() {
        format!(
            "{}: {}.{}",
            table.object_kind(),
            schema.database,
            table.name
        )
    } else {
        format!("{}: {}", table.object_kind(), table.name)
    };

    CompletionItem {
        label: label.clone(),
        kind: Some(CompletionItemKind::CLASS),
        detail: Some(detail),
        documentation: table.documentation().map(Documentation::String),
        deprecated: None,
        preselect: None,
        sort_text: Some(completion_sort_text("1", &table.name)),
        filter_text: Some(crate::identifier_search::filter_text(&table.name, None)),
        insert_text: Some(label),
        insert_text_format: None,
        insert_text_mode: None,
        text_edit: None,
        additional_text_edits: None,
        commit_characters: None,
        command: None,
        data: crate::identifier_search::table_data(&table.name),
        tags: None,
        label_details: None,
    }
}

pub(crate) fn create_column_item(column: &Column, table_name: Option<&str>) -> CompletionItem {
    let label = if let Some(table) = table_name {
        format!("{}.{}", table, column.name)
    } else {
        column.name.clone()
    };

    let mut detail = if let Some(table) = table_name {
        format!("Column: {}.{} ({})", table, column.name, column.data_type)
    } else {
        format!("Column: {} ({})", column.name, column.data_type)
    };
    detail.push_str(column_visibility_suffix(column));

    CompletionItem {
        label: label.clone(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some(detail),
        documentation: column.documentation().map(Documentation::String),
        deprecated: None,
        preselect: None,
        sort_text: Some(completion_sort_text(
            if column.hidden { "3" } else { "2" },
            &column.name,
        )),
        filter_text: Some(crate::identifier_search::filter_text(
            &column.name,
            column.comment.as_deref(),
        )),
        insert_text: Some(label),
        insert_text_format: None,
        insert_text_mode: None,
        text_edit: None,
        additional_text_edits: None,
        commit_characters: None,
        command: None,
        data: Some(
            serde_json::json!({"oxide":{"column":column.name,"searchComment":column.comment.as_deref().map(crate::identifier_search::comment_prefix)}}),
        ),
        tags: None,
        label_details: None,
    }
}

pub(super) fn column_visibility_suffix(column: &Column) -> &'static str {
    if column.hidden {
        " · hidden"
    } else if column.generated {
        " · generated"
    } else {
        ""
    }
}

pub(crate) fn create_function_item(
    function: &Function,
    schema: &Schema,
    qualify_with_database: bool,
) -> CompletionItem {
    let label = if qualify_with_database && !schema.database.is_empty() {
        format!("{}.{}", schema.database, function.name)
    } else {
        function.name.clone()
    };
    let signature = if qualify_with_database && !schema.database.is_empty() {
        format!("{}.{}", schema.database, function.signature())
    } else {
        function.signature()
    };
    let detail = if function.routine_kind() == "Procedure" {
        format!("Procedure: {signature}")
    } else {
        format!("Function: {signature} -> {}", function.return_type)
    };

    CompletionItem {
        label: label.clone(),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some(detail),
        documentation: Some(Documentation::String(function.documentation())),
        deprecated: None,
        preselect: None,
        sort_text: Some(completion_sort_text("1", &function.name)),
        filter_text: Some(function.name.clone()),
        insert_text: Some(format!("{label}()")),
        insert_text_format: None,
        insert_text_mode: None,
        text_edit: None,
        additional_text_edits: None,
        commit_characters: None,
        command: None,
        data: None,
        tags: None,
        label_details: None,
    }
}

pub(crate) fn create_constraint_item(constraint: &Constraint) -> CompletionItem {
    CompletionItem {
        label: constraint.name.clone(),
        kind: Some(CompletionItemKind::REFERENCE),
        detail: Some(format!("Constraint: {}", constraint.constraint_type)),
        documentation: constraint
            .definition
            .as_ref()
            .map(|definition| Documentation::String(definition.clone())),
        deprecated: None,
        preselect: None,
        sort_text: Some(completion_sort_text("1", &constraint.name)),
        filter_text: Some(constraint.name.clone()),
        insert_text: Some(constraint.name.clone()),
        insert_text_format: None,
        insert_text_mode: None,
        text_edit: None,
        additional_text_edits: None,
        commit_characters: None,
        command: None,
        data: None,
        tags: None,
        label_details: None,
    }
}

pub(crate) fn create_index_item(
    index: &Index,
    schema: &Schema,
    qualify_with_database: bool,
) -> CompletionItem {
    let label = if qualify_with_database && !schema.database.is_empty() {
        format!("{}.{}", schema.database, index.name)
    } else {
        index.name.clone()
    };
    let index_kind = if index.is_primary {
        "Primary index"
    } else if index.is_unique {
        "Unique index"
    } else {
        "Index"
    };

    CompletionItem {
        label: label.clone(),
        kind: Some(CompletionItemKind::REFERENCE),
        detail: Some(format!("{}: {}", index_kind, index.name)),
        documentation: index
            .definition
            .as_ref()
            .map(|definition| Documentation::String(definition.clone())),
        deprecated: None,
        preselect: None,
        sort_text: Some(completion_sort_text("1", &index.name)),
        filter_text: Some(index.name.clone()),
        insert_text: Some(label),
        insert_text_format: None,
        insert_text_mode: None,
        text_edit: None,
        additional_text_edits: None,
        commit_characters: None,
        command: None,
        data: None,
        tags: None,
        label_details: None,
    }
}
