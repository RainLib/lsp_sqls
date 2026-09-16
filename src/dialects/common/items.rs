use super::cursor::text_before_position;
use super::references::find_table_by_reference;
use crate::schema::{Column, Constraint, Function, Index, Schema, Table};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, InsertTextFormat, Position,
};

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

pub(crate) fn add_column_value_items(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    referenced_tables: &[String],
    sql: &str,
    position: Position,
    prefix: &str,
) {
    let Some(column_name) = predicate_column_before_value(sql, position, prefix) else {
        return;
    };
    let mut domain_value_sets = Vec::new();
    let mut value_kinds = Vec::new();
    let tables = if referenced_tables.is_empty() {
        schema.tables.iter().collect::<Vec<_>>()
    } else {
        referenced_tables
            .iter()
            .filter_map(|reference| find_table_by_reference(schema, reference))
            .collect::<Vec<_>>()
    };

    for table in tables {
        let Some(column) = table
            .columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(&column_name))
        else {
            continue;
        };
        let mut domain_values = sql_type_domain_values(&column.data_type);
        domain_values.sort();
        domain_values.dedup();
        domain_value_sets.push(domain_values);
        value_kinds.push(sql_type_value_template_kind(&column.data_type));
    }

    let normalized_prefix = prefix.trim_matches(['\'', '"']).to_ascii_lowercase();
    add_null_value_item(items, &column_name, &normalized_prefix);
    if let Some(values) = domain_value_sets
        .first()
        .filter(|values| !values.is_empty())
        .filter(|values| {
            domain_value_sets
                .iter()
                .all(|candidate| candidate == *values)
        })
    {
        for value in values.iter().take(64) {
            if !normalized_prefix.is_empty()
                && !value.to_ascii_lowercase().starts_with(&normalized_prefix)
            {
                continue;
            }
            let quoted = format!("'{}'", value.replace('\'', "''"));
            items.push(CompletionItem {
                label: value.clone(),
                kind: Some(CompletionItemKind::ENUM_MEMBER),
                detail: Some(format!("Domain value for {column_name}")),
                sort_text: Some(completion_sort_text("0", value)),
                filter_text: Some(value.clone()),
                insert_text: Some(quoted),
                ..CompletionItem::default()
            });
        }
    }

    let Some(value_kind) = value_kinds.first().copied().flatten().filter(|kind| {
        value_kinds
            .iter()
            .all(|candidate| candidate == &Some(*kind))
    }) else {
        return;
    };
    add_typed_value_template_items(items, &column_name, value_kind, &normalized_prefix);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SqlValueTemplateKind {
    Boolean,
    Date,
    Json,
    Numeric,
    Text,
    Time,
    Timestamp,
    Uuid,
}

fn sql_type_value_template_kind(data_type: &str) -> Option<SqlValueTemplateKind> {
    let normalized = data_type.trim().to_ascii_lowercase();
    let base = normalized.split(['(', ' ', '[']).next().unwrap_or("");
    if normalized.starts_with("enum(") || normalized.starts_with("set(") {
        return None;
    }
    if matches!(base, "boolean" | "bool" | "bit") {
        return Some(SqlValueTemplateKind::Boolean);
    }
    if matches!(
        base,
        "timestamp" | "timestamptz" | "datetime" | "datetime2" | "smalldatetime"
    ) || normalized.starts_with("timestamp with")
        || normalized.starts_with("timestamp without")
    {
        return Some(SqlValueTemplateKind::Timestamp);
    }
    if matches!(base, "date") {
        return Some(SqlValueTemplateKind::Date);
    }
    if matches!(base, "time" | "timetz") || normalized.starts_with("time with") {
        return Some(SqlValueTemplateKind::Time);
    }
    if matches!(base, "uuid" | "uniqueidentifier") {
        return Some(SqlValueTemplateKind::Uuid);
    }
    if matches!(base, "json" | "jsonb") {
        return Some(SqlValueTemplateKind::Json);
    }
    if matches!(
        base,
        "tinyint"
            | "smallint"
            | "mediumint"
            | "int"
            | "integer"
            | "bigint"
            | "decimal"
            | "numeric"
            | "number"
            | "float"
            | "float4"
            | "float8"
            | "real"
            | "double"
            | "money"
            | "smallmoney"
    ) {
        return Some(SqlValueTemplateKind::Numeric);
    }
    if matches!(
        base,
        "char"
            | "nchar"
            | "varchar"
            | "varchar2"
            | "nvarchar"
            | "nvarchar2"
            | "text"
            | "tinytext"
            | "mediumtext"
            | "longtext"
            | "ntext"
            | "citext"
            | "string"
    ) {
        return Some(SqlValueTemplateKind::Text);
    }
    None
}

fn value_item_matches_prefix(label: &str, prefix: &str) -> bool {
    prefix.is_empty() || label.to_ascii_lowercase().starts_with(prefix)
}

fn add_null_value_item(items: &mut Vec<CompletionItem>, column_name: &str, prefix: &str) {
    if !value_item_matches_prefix("NULL", prefix) {
        return;
    }
    items.push(CompletionItem {
        label: "NULL".to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        detail: Some(format!("NULL value for {column_name}")),
        sort_text: Some(completion_sort_text("0", "NULL")),
        insert_text: Some("NULL".to_string()),
        ..CompletionItem::default()
    });
}

fn add_typed_value_template_items(
    items: &mut Vec<CompletionItem>,
    column_name: &str,
    kind: SqlValueTemplateKind,
    prefix: &str,
) {
    let detail = |label: &str| format!("{label} for {column_name}");
    let mut add_snippet = |label: &str, insert_text: &str, description: &str| {
        if !value_item_matches_prefix(label, prefix) {
            return;
        }
        items.push(CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(detail(description)),
            sort_text: Some(completion_sort_text("0", label)),
            filter_text: Some(label.to_string()),
            insert_text: Some(insert_text.to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..CompletionItem::default()
        });
    };

    match kind {
        SqlValueTemplateKind::Boolean => {
            for value in ["TRUE", "FALSE"] {
                if value_item_matches_prefix(value, prefix) {
                    items.push(CompletionItem {
                        label: value.to_string(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        detail: Some(detail("Boolean value")),
                        sort_text: Some(completion_sort_text("0", value)),
                        insert_text: Some(value.to_string()),
                        ..CompletionItem::default()
                    });
                }
            }
        }
        SqlValueTemplateKind::Date => {
            add_snippet("'YYYY-MM-DD'", "'${1:YYYY-MM-DD}'", "Date literal")
        }
        SqlValueTemplateKind::Json => {
            add_snippet("'{}'", r#"'{"${1:key}":"${2:value}"}'"#, "JSON literal")
        }
        SqlValueTemplateKind::Numeric => add_snippet("0", "${1:0}", "Numeric literal"),
        SqlValueTemplateKind::Text => add_snippet("''", "'${1:value}'", "String literal"),
        SqlValueTemplateKind::Time => add_snippet("'HH:MM:SS'", "'${1:HH:MM:SS}'", "Time literal"),
        SqlValueTemplateKind::Timestamp => add_snippet(
            "'YYYY-MM-DD HH:MM:SS'",
            "'${1:YYYY-MM-DD HH:MM:SS}'",
            "Timestamp literal",
        ),
        SqlValueTemplateKind::Uuid => add_snippet(
            "'UUID'",
            "'${1:00000000-0000-0000-0000-000000000000}'",
            "UUID literal",
        ),
    }
}

fn predicate_column_before_value(sql: &str, position: Position, prefix: &str) -> Option<String> {
    let mut before = text_before_position(sql, position).trim_end();
    if !prefix.is_empty() && before.len() >= prefix.len() {
        let prefix_start = before.len() - prefix.len();
        if before[prefix_start..].eq_ignore_ascii_case(prefix) {
            before = before[..prefix_start].trim_end();
        }
    }
    before = before.trim_end_matches(['\'', '"']).trim_end();
    if before.ends_with('(') {
        before = before[..before.len() - 1].trim_end();
        if before.to_ascii_uppercase().ends_with(" IN") {
            before = before[..before.len() - 3].trim_end();
        }
    } else {
        let operator = before
            .char_indices()
            .rev()
            .find(|(_, character)| matches!(character, '=' | '<' | '>'))
            .map(|(index, _)| index)?;
        before = before[..operator].trim_end();
    }

    let identifier = before
        .rsplit(|character: char| {
            !(character.is_ascii_alphanumeric()
                || matches!(character, '_' | '$' | '.' | '`' | '"' | '[' | ']'))
        })
        .next()?
        .trim_matches(['`', '"', '[', ']']);
    let column = identifier
        .rsplit('.')
        .next()?
        .trim_matches(['`', '"', '[', ']']);
    (!column.is_empty()).then(|| column.to_string())
}

fn sql_type_domain_values(data_type: &str) -> Vec<String> {
    let normalized = data_type.trim();
    let lower = normalized.to_ascii_lowercase();
    if !(lower.starts_with("enum(") || lower.starts_with("set(")) || !normalized.ends_with(')') {
        return Vec::new();
    }
    let Some(open) = normalized.find('(') else {
        return Vec::new();
    };
    let content = &normalized[open + 1..normalized.len() - 1];
    let mut values = Vec::new();
    let mut current = String::new();
    let mut chars = content.chars().peekable();
    let mut quoted = false;
    while let Some(character) = chars.next() {
        match character {
            '\'' if quoted && chars.peek() == Some(&'\'') => {
                current.push('\'');
                chars.next();
            }
            '\'' => {
                if quoted {
                    values.push(std::mem::take(&mut current));
                }
                quoted = !quoted;
            }
            _ if quoted => current.push(character),
            _ => {}
        }
    }
    values
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
        filter_text: Some(table.name.clone()),
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
        filter_text: Some(column.name.clone()),
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
