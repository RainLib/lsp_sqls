use super::cursor::text_before_position;
use super::items::completion_sort_text;
use super::references::find_table_by_reference;
use crate::parser::SqlParser;
use crate::placeholder::SqlPlaceholderDialect;
use crate::schema::Schema;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, InsertTextFormat, Position, Range,
    TextEdit,
};

pub(crate) fn add_column_value_items(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    referenced_tables: &[String],
    sql: &str,
    position: Position,
    prefix: &str,
    profile: &str,
) -> Option<SqlValueCompletionPolicy> {
    let reference = predicate_column_before_value(sql, position, prefix, profile)?;
    let column_name = reference.column;
    let mut domain_value_sets = Vec::new();
    let mut value_kinds = Vec::new();
    let tables = if let Some(qualifier) = reference.qualifier.as_deref() {
        let before = &sql[..reference.end];
        let at = Position::new(
            before.bytes().filter(|byte| *byte == b'\n').count() as u32,
            before
                .rfind('\n')
                .map_or(before.len(), |index| before.len() - index - 1) as u32,
        );
        let aliases = SqlParser::relation_aliases_at_position(sql, at);
        let resolved = SqlParser::resolve_relation_alias(&aliases, qualifier)
            .map(|alias| alias.relation.as_str());
        match resolved {
            Some(name) => find_table_by_reference(schema, name).into_iter().collect(),
            None => referenced_tables
                .iter()
                .filter_map(|name| find_table_by_reference(schema, name))
                .filter(|table| {
                    SqlParser::table_name_matches(qualifier, &schema.database, &table.name)
                })
                .collect(),
        }
    } else if referenced_tables.is_empty() {
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
        value_kinds.push(sql_type_value_template_kind(&column.data_type, profile));
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
        for value in values
            .iter()
            .filter(|value| {
                normalized_prefix.is_empty()
                    || value.to_ascii_lowercase().starts_with(&normalized_prefix)
            })
            .take(64)
        {
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

    let value_kind = value_kinds.first().copied().flatten().filter(|kind| {
        value_kinds
            .iter()
            .all(|candidate| candidate == &Some(*kind))
    })?;
    add_typed_value_template_items(items, &column_name, value_kind, &normalized_prefix);
    Some(SqlValueCompletionPolicy { kind: value_kind })
}

#[derive(Clone, Copy)]
pub(crate) struct SqlValueCompletionPolicy {
    kind: SqlValueTemplateKind,
}

pub(crate) fn add_expression_value_items(
    items: &mut Vec<CompletionItem>,
    data_type: &str,
    profile: &str,
    prefix: &str,
    range: Range,
) -> Option<()> {
    let kind = sql_type_value_template_kind(data_type, profile)?;
    apply_value_completion_policy(items, Some(SqlValueCompletionPolicy { kind }), profile);
    let prefix = prefix.to_ascii_lowercase();
    let mut values = Vec::new();
    add_null_value_item(&mut values, "comparison expression", &prefix);
    add_typed_value_template_items(&mut values, "comparison expression", kind, &prefix);
    // A generic duplicate must not win deduplication with a cursor-only edit.
    // Fields, functions and quoted domain values keep their independent identity.
    items.retain(|item| {
        !matches!(
            item.kind,
            Some(
                CompletionItemKind::KEYWORD
                    | CompletionItemKind::VALUE
                    | CompletionItemKind::SNIPPET
            )
        ) || !values
            .iter()
            .any(|value| value.label.eq_ignore_ascii_case(&item.label))
    });
    for item in &mut values {
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: item
                .insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone()),
        }));
    }
    items.splice(0..0, values);
    Some(())
}

pub(crate) fn add_pattern_escape_value_items(
    items: &mut Vec<CompletionItem>,
    prefix: &str,
    range: Range,
) {
    items.retain(|item| {
        !matches!(
            item.kind,
            Some(
                CompletionItemKind::KEYWORD
                    | CompletionItemKind::VALUE
                    | CompletionItemKind::SNIPPET
                    | CompletionItemKind::ENUM_MEMBER
                    | CompletionItemKind::CONSTANT
            )
        )
    });
    let mut values = Vec::new();
    add_null_value_item(
        &mut values,
        "pattern escape character",
        &prefix.to_ascii_lowercase(),
    );
    if prefix.is_empty() {
        for literal in ["'#'", "'!'"] {
            values.push(CompletionItem {
                label: literal.into(),
                kind: Some(CompletionItemKind::VALUE),
                insert_text: Some(literal.into()),
                detail: Some("Single pattern escape character".into()),
                sort_text: Some(format!("0:escape:{literal}")),
                ..CompletionItem::default()
            });
        }
    }
    for item in &mut values {
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: item
                .insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone()),
        }));
    }
    items.splice(0..0, values);
}

pub(crate) fn type_supports_truth_predicates(data_type: &str, profile: &str) -> Option<bool> {
    sql_type_value_template_kind(data_type, profile)
        .map(|kind| kind == SqlValueTemplateKind::Boolean)
}
pub(crate) fn type_supports_logical_continuation(data_type: &str, profile: &str) -> Option<bool> {
    let kind = sql_type_value_template_kind(data_type, profile)?;
    use SqlValueTemplateKind::*;
    Some(match profile {
        "postgres" | "hive" => kind == Boolean,
        "sqlserver" => false,
        "sqlite" => true,
        "mysql" | "mariadb" => match kind {
            Json => return None,
            NullOnly => {
                let name = data_type.trim().to_ascii_lowercase();
                name.starts_with("enum(") || name.starts_with("set(")
            }
            _ => true,
        },
        "duckdb" => match kind {
            Boolean | NumericBoolean | Numeric | Text | DuckBits => true,
            Json => return None,
            NullOnly if data_type.trim().to_ascii_lowercase().starts_with("enum(") => return None,
            _ => false,
        },
        "clickhouse" => {
            let mut name = data_type.trim().to_ascii_lowercase();
            for _ in 0..4 {
                if (name.starts_with("nullable(") || name.starts_with("lowcardinality("))
                    && name.ends_with(')')
                {
                    name = name[name.find('(')? + 1..name.len() - 1].trim().to_owned();
                } else {
                    break;
                }
            }
            kind == Boolean
                || name
                    .strip_prefix('u')
                    .unwrap_or(&name)
                    .strip_prefix("int")
                    .is_some_and(|suffix| suffix.bytes().all(|byte| byte.is_ascii_digit()))
                || name
                    .strip_prefix("float")
                    .is_some_and(|suffix| suffix.bytes().all(|byte| byte.is_ascii_digit()))
        }
        _ => return None,
    })
}

pub(crate) fn apply_value_completion_policy(
    items: &mut Vec<CompletionItem>,
    policy: Option<SqlValueCompletionPolicy>,
    profile: &str,
) {
    let tsql = matches!(profile, "sqlserver" | "mssql" | "tsql");
    items.retain(|item| {
        if item.kind != Some(CompletionItemKind::KEYWORD) {
            return true;
        }
        let label = item.label.to_ascii_uppercase();
        if tsql
            && matches!(
                label.as_str(),
                "TRUE"
                    | "FALSE"
                    | "UNKNOWN"
                    | "IS TRUE"
                    | "IS FALSE"
                    | "IS UNKNOWN"
                    | "IS NOT TRUE"
                    | "IS NOT FALSE"
                    | "IS NOT UNKNOWN"
            )
        {
            return false;
        }
        if !matches!(
            label.as_str(),
            "TRUE" | "FALSE" | "CURRENT_DATE" | "CURRENT_TIMESTAMP"
        ) {
            return true;
        }
        match policy.map(|value| value.kind) {
            None => true,
            Some(SqlValueTemplateKind::Boolean) => matches!(label.as_str(), "TRUE" | "FALSE"),
            Some(
                SqlValueTemplateKind::Date
                | SqlValueTemplateKind::Time
                | SqlValueTemplateKind::Timestamp,
            ) => matches!(label.as_str(), "CURRENT_DATE" | "CURRENT_TIMESTAMP"),
            Some(_) => false,
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SqlValueTemplateKind {
    Boolean,
    NumericBoolean,
    BinaryBits(Option<u32>),
    DuckBits,
    NullOnly,
    Date,
    Json,
    Numeric,
    Text,
    Time,
    Timestamp,
    Uuid,
}

fn sql_type_value_template_kind(data_type: &str, profile: &str) -> Option<SqlValueTemplateKind> {
    let postgres = matches!(profile, "postgres" | "postgresql" | "pgsql" | "psql");
    let builtin = postgres
        .then(|| crate::postgres_types::builtin_type(data_type))
        .flatten();
    if builtin.as_ref().is_some_and(|ty| ty.array) {
        return Some(SqlValueTemplateKind::NullOnly);
    }
    let canonical;
    let data_type = if let Some(ty) = builtin.as_ref() {
        canonical = format!("{}{}", ty.name, ty.modifiers.unwrap_or(""));
        canonical.as_str()
    } else {
        data_type
    };
    let mut normalized = data_type.trim().to_ascii_lowercase();
    for _ in 0..4 {
        if (normalized.starts_with("nullable(") || normalized.starts_with("lowcardinality("))
            && normalized.ends_with(')')
        {
            normalized = normalized[normalized.find('(')? + 1..normalized.len() - 1]
                .trim()
                .to_string();
        } else {
            break;
        }
    }
    if normalized.contains('[')
        || ["array(", "map(", "tuple(", "struct("]
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
    {
        return Some(SqlValueTemplateKind::NullOnly);
    }
    let base = normalized.split(['(', ' ', '[']).next().unwrap_or("");
    if normalized.starts_with("enum(") || normalized.starts_with("set(") {
        return Some(SqlValueTemplateKind::NullOnly);
    }
    if matches!(base, "bit" | "varbit" | "bitstring") {
        return match profile {
            "sqlserver" | "mssql" | "tsql" | "sqlite" => Some(SqlValueTemplateKind::NumericBoolean),
            "duckdb" => Some(SqlValueTemplateKind::DuckBits),
            "postgres" | "postgresql" | "pgsql" | "psql" | "mysql" | "mariadb" => {
                if base == "bitstring" {
                    return None;
                }
                let variable = normalized.starts_with("bit varying") || base == "varbit";
                let width = bit_value_width(&normalized)?;
                if matches!(profile, "mysql" | "mariadb")
                    && (base != "bit" || variable || width > 64)
                {
                    return None;
                }
                Some(SqlValueTemplateKind::BinaryBits(
                    (!variable).then_some(width),
                ))
            }
            _ => None,
        };
    }
    if matches!(base, "boolean" | "bool") {
        if matches!(profile, "sqlserver" | "mssql" | "tsql") {
            return Some(SqlValueTemplateKind::NumericBoolean);
        }
        return Some(SqlValueTemplateKind::Boolean);
    }
    if matches!(
        base,
        "timestamp"
            | "timestamptz"
            | "datetime"
            | "datetime2"
            | "datetime64"
            | "datetimeoffset"
            | "smalldatetime"
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
    ) || profile == "clickhouse"
        && matches!(
            base,
            "decimal32" | "decimal64" | "decimal128" | "decimal256"
        )
        || base
            .strip_prefix('u')
            .unwrap_or(base)
            .strip_prefix("int")
            .is_some_and(|suffix| matches!(suffix, "8" | "16" | "32" | "64" | "128" | "256"))
    {
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
            | "character"
            | "fixedstring"
    ) {
        return Some(SqlValueTemplateKind::Text);
    }
    builtin.map(|_| SqlValueTemplateKind::NullOnly)
}

fn bit_value_width(data_type: &str) -> Option<u32> {
    let Some(open) = data_type.find('(') else {
        return Some(1);
    };
    let value = data_type.get(open + 1..data_type.len().checked_sub(1)?)?;
    data_type.ends_with(')').then_some(())?;
    let width = value.trim().parse::<u32>().ok()?;
    (width > 0).then_some(width)
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
        SqlValueTemplateKind::Boolean | SqlValueTemplateKind::NumericBoolean => {
            let numeric = kind == SqlValueTemplateKind::NumericBoolean;
            for value in if numeric {
                ["0", "1"]
            } else {
                ["TRUE", "FALSE"]
            } {
                if value_item_matches_prefix(value, prefix) {
                    items.push(CompletionItem {
                        label: value.to_string(),
                        kind: Some(if numeric {
                            CompletionItemKind::VALUE
                        } else {
                            CompletionItemKind::KEYWORD
                        }),
                        detail: Some(detail("Boolean value")),
                        sort_text: Some(completion_sort_text("0", value)),
                        insert_text: Some(value.to_string()),
                        ..CompletionItem::default()
                    });
                }
            }
        }
        SqlValueTemplateKind::BinaryBits(width) => {
            for last in ['0', '1'] {
                let value = match width {
                    Some(width) if width > 64 => format!("B'{last}'::bit({width})"),
                    _ => format!(
                        "B'{}{last}'",
                        "0".repeat(width.unwrap_or(1).saturating_sub(1) as usize)
                    ),
                };
                if value_item_matches_prefix(&value, prefix) {
                    items.push(CompletionItem {
                        label: value.clone(),
                        kind: Some(CompletionItemKind::VALUE),
                        detail: Some(detail("Bit-string literal")),
                        insert_text: Some(value.clone()),
                        filter_text: Some(value.clone()),
                        sort_text: Some(completion_sort_text("0", &value)),
                        ..Default::default()
                    });
                }
            }
        }
        SqlValueTemplateKind::DuckBits => {
            for value in ["'0'::BIT", "'1'::BIT"] {
                if value_item_matches_prefix(value, prefix) {
                    items.push(CompletionItem {
                        label: value.into(),
                        kind: Some(CompletionItemKind::VALUE),
                        detail: Some(detail("Bit-string literal")),
                        insert_text: Some(value.into()),
                        filter_text: Some(value.into()),
                        sort_text: Some(completion_sort_text("0", value)),
                        ..Default::default()
                    });
                }
            }
        }
        SqlValueTemplateKind::NullOnly => {}
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

#[derive(Debug, PartialEq, Eq)]
struct ValueReference {
    column: String,
    qualifier: Option<String>,
    end: usize,
}

#[derive(Default)]
struct ValueFrame {
    case_expression: bool,
    list: Option<usize>,
    between: Option<(usize, bool)>,
    last_word: Option<(String, usize)>,
}

fn predicate_column_before_value(
    sql: &str,
    position: Position,
    prefix: &str,
    profile: &str,
) -> Option<ValueReference> {
    let mut before = text_before_position(sql, position).trim_end();
    if before.len() > 524288 {
        return None;
    }
    if !prefix.is_empty() && before.len() >= prefix.len() {
        let prefix_start = before.len() - prefix.len();
        if before
            .get(prefix_start..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(prefix))
        {
            before = before[..prefix_start].trim_end();
        }
    }
    before = before.trim_end_matches(['\'', '"']).trim_end();
    let mut comments = Vec::new();
    let mask = SqlParser::mask_sql_value_context_and_comments(before, profile, &mut |range| {
        comments.push(range)
    });
    let mut frames = vec![ValueFrame::default()];
    let mut index = 0;
    let mut last_symbol = None;
    while index < mask.len() {
        let character = mask[index..].chars().next()?;
        if character.is_whitespace() {
            index += character.len_utf8();
            continue;
        }
        if character.is_alphanumeric() || character == '_' {
            let start = index;
            index += character.len_utf8();
            while let Some(next) = mask[index..].chars().next() {
                if next.is_alphanumeric() || next == '_' {
                    index += next.len_utf8();
                } else {
                    break;
                }
            }
            let word = mask[start..index].to_ascii_uppercase();
            if word == "CASE" {
                if frames.len() >= 128 {
                    return None;
                }
                frames.push(ValueFrame {
                    case_expression: true,
                    ..Default::default()
                });
                last_symbol = None;
                continue;
            }
            if word == "END" && frames.last()?.case_expression {
                frames.pop();
                frames.last_mut()?.last_word = None;
                last_symbol = None;
                continue;
            }
            let frame = frames.last_mut()?;
            if word == "BETWEEN" {
                frame.between = Some((start, false));
            } else if word == "AND" {
                if let Some((_, separator @ false)) = frame.between.as_mut() {
                    *separator = true;
                } else {
                    frame.between = None;
                }
            } else if [
                "OR", "WHERE", "HAVING", "ON", "WHEN", "THEN", "ELSE", "FROM", "SELECT", "QUALIFY",
            ]
            .contains(&word.as_str())
            {
                frame.between = None;
            }
            frame.last_word = Some((word, start));
            last_symbol = None;
            continue;
        }
        match character {
            '(' | '[' => {
                if frames.len() >= 128 {
                    return None;
                }
                let list = frames
                    .last()
                    .and_then(|frame| frame.last_word.as_ref())
                    .filter(|(word, _)| word == "IN")
                    .map(|(_, start)| *start);
                frames.push(ValueFrame {
                    list,
                    ..Default::default()
                });
            }
            ')' | ']' => {
                if frames.len() > 1 {
                    frames.pop();
                }
                frames.last_mut()?.last_word = None;
            }
            ';' => {
                frames.clear();
                frames.push(ValueFrame::default());
            }
            _ => {
                frames.last_mut()?.last_word = None;
            }
        }
        last_symbol = Some((character, index));
        index += character.len_utf8();
    }
    let frame = frames.last()?;
    let mut end = match last_symbol {
        Some(('(' | ',', _)) => frame.list?,
        Some(('=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%', start)) => {
            let mut start = start;
            while start > 0 && matches!(mask.as_bytes()[start - 1], b'=' | b'<' | b'>' | b'!') {
                start -= 1;
            }
            start
        }
        _ => {
            let (word, start) = frame.last_word.as_ref()?;
            if ["LIKE", "ILIKE", "RLIKE", "REGEXP", "BETWEEN"].contains(&word.as_str()) {
                *start
            } else if word == "AND" {
                frame.between.filter(|(_, separator)| *separator)?.0
            } else {
                return None;
            }
        }
    };
    let mut comments = comments.into_iter().rev().peekable();
    loop {
        let left = before.get(..end)?.trim_end();
        end = left.len();
        while comments.peek().is_some_and(|range| range.end > end) {
            comments.next();
        }
        if let Some(range) = comments.next_if(|range| range.end == end) {
            end = range.start;
        } else {
            break;
        }
    }
    let mut left = before.get(..end)?.trim_end();
    if left
        .rsplit(|character: char| character.is_whitespace())
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("NOT"))
    {
        left = left.get(..left.len() - 3)?.trim_end();
    }
    let identifier = SqlParser::identifier_path_token_for_dialect(
        left,
        SqlPlaceholderDialect::for_name(Some(profile)),
    )?;
    if crate::token::Keywords::is_keyword(identifier)
        || identifier
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    {
        return None;
    }
    Some(ValueReference {
        column: SqlParser::identifier_last_part(identifier),
        qualifier: SqlParser::identifier_qualifier(identifier),
        end: left.len(),
    })
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

#[cfg(test)]
#[path = "value_items_tests.rs"]
mod tests;
