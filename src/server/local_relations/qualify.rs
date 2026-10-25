//! SELECT output names are expression bindings in QUALIFY, never source columns.
use super::{provenance::ParsedOrigin, query_output, LOCAL_RELATION_SCAN_MAX_BYTES};
use crate::server::completion_context::position_to_byte_offset;
use crate::{
    dialects::common,
    parser::SqlParser,
    position::byte_position_at_end,
    schema::{Schema, SchemaId, SchemaManager},
};
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

pub(in crate::server) fn add_qualify_completions(
    text: &str,
    position: Position,
    profile: &str,
    schema: Option<&Schema>,
    uri: &str,
    routines: Option<&SchemaManager>,
    items: &mut Vec<CompletionItem>,
) -> bool {
    if profile != "clickhouse" || text.len() > LOCAL_RELATION_SCAN_MAX_BYTES {
        return false;
    }
    let cursor = position_to_byte_offset(text, position);
    let byte_position = byte_position_at_end(&text[..cursor]);
    let Some(query) = SqlParser::qualify_projection_range(text, byte_position) else {
        return false;
    };
    // Parser helpers use byte columns. A quoted dot inside an alias is atomic.
    if SqlParser::column_qualifier_before_position(text, byte_position).is_some() {
        return false;
    }
    let prefix = common::cursor_prefix(text, byte_position);
    let prefix = if prefix.eq_ignore_ascii_case("QUALIFY")
        || prefix.eq_ignore_ascii_case("AND")
        || prefix.eq_ignore_ascii_case("OR")
    {
        ""
    } else {
        &prefix
    };
    let empty = Schema {
        id: SchemaId(uuid::Uuid::nil()),
        server: None,
        namespace: None,
        catalog: None,
        database: String::new(),
        server_version: None,
        tables: Vec::new(),
        functions: Vec::new(),
        source_uri: None,
    };
    let schema = schema.unwrap_or(&empty);
    let source_line = text[..query.start].bytes().filter(|b| *b == b'\n').count() as u32 + 1;
    let origin = ParsedOrigin::new(text, Some(profile));
    if !origin.qualify_keyword_at(query.end) {
        return false;
    }
    let context = query_output::Context {
        schema,
        uri,
        line: source_line,
        dialect: Some(profile),
        environment: query_output::Environment {
            origin: origin.view(),
            routines,
            variables: None,
        },
    };
    let body = &text[query];
    let upper = SqlParser::mask_sql_noise(body).to_ascii_uppercase();
    let mut budget = query_output::MAX_SET_BRANCHES;
    let output = context.query(body, &upper, 0, &mut budget);
    let columns = context.finish(output, &[]);
    // Duplicate output names do not establish an unambiguous expression type.
    let mut names = HashSet::new();
    let duplicates: HashSet<_> = columns
        .iter()
        .filter_map(|c| (!names.insert(c.name.as_str())).then_some(c.name.as_str()))
        .collect();
    let mut eligible: Vec<_> = columns
        .iter()
        .filter(|column| {
            !column.name.is_empty()
                && !duplicates.contains(column.name.as_str())
                && crate::identifier_search::column_match_rank(column, prefix).is_some()
        })
        .collect();
    // An exact name must survive even when it is declared after many matches.
    eligible.sort_by_key(|column| !column.name.eq_ignore_ascii_case(prefix));
    const MAX_OUTPUT_COMPLETIONS: usize = 256;
    let incomplete = eligible.len() > MAX_OUTPUT_COMPLETIONS;
    let mut input_types: HashMap<String, Vec<&str>> = HashMap::new();
    for item in items
        .iter()
        .filter(|item| item.kind == Some(CompletionItemKind::FIELD))
    {
        let name = item
            .data
            .as_ref()
            .and_then(|data| data.pointer("/oxide/column"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| SqlParser::identifier_last_part(&item.label));
        input_types
            .entry(name)
            .or_default()
            .push(item.detail.as_deref().unwrap_or(""));
    }
    let mut additions = Vec::new();
    let mut replaced = HashSet::new();
    for column in eligible.into_iter().take(MAX_OUTPUT_COMPLETIONS) {
        // Same-named input fields can depend on prefer_column_name_to_alias.
        // Preserve a neutral type when these possible bindings disagree.
        let mut column = column.clone();
        let expected_type = format!("({})", column.data_type);
        if input_types.get(&column.name).is_some_and(|details| {
            details
                .iter()
                .any(|detail| !detail.contains(&expected_type))
        }) {
            column.data_type = "unknown".into();
            column.nullable = true;
        }
        let mut item = common::create_column_item(&column, None);
        item.detail
            .as_mut()
            .unwrap()
            .push_str("; SELECT output in QUALIFY");
        item.data.as_mut().unwrap()["oxide"]["projection"] = serde_json::json!(true);
        replaced.insert(column.name);
        additions.push(item);
    }
    drop(input_types);
    items.retain(|item| {
        item.kind != Some(CompletionItemKind::FIELD) || !replaced.contains(&item.label)
    });
    items.extend(additions);
    incomplete
}
