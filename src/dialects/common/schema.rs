use super::items::{
    column_visibility_suffix, completion_sort_text, create_column_item, create_constraint_item,
    create_function_item, create_index_item, create_table_item, set_completion_sort_text,
};
use super::references::{find_table_by_reference, table_is_referenced, table_matches};
use crate::parser::RelationAlias;
use crate::schema::{Constraint, Schema, Table};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Documentation, InsertTextFormat};

pub(crate) fn add_schema_tables(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    prefix: &str,
    qualify_with_database: bool,
    qualified_only: bool,
) {
    let local_names: HashSet<_> = schema
        .tables
        .iter()
        .filter(|table| crate::relation_bindings::query_local(table))
        .map(|table| table.name.to_ascii_lowercase())
        .collect();
    for table in &schema.tables {
        let local = crate::relation_bindings::query_local(table);
        let shadowed = !crate::relation_bindings::local(table)
            && local_names.contains(&table.name.to_ascii_lowercase());
        if (local && qualified_only) || (shadowed && schema.database.is_empty()) {
            continue;
        }
        if identifier_match_rank(&table.name, prefix).is_none() {
            continue;
        }
        items.push(create_table_item(
            table,
            schema,
            !local && (qualify_with_database || shadowed),
        ));
    }
}

pub(crate) fn add_foreign_key_join_snippets(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    referenced_tables: &[String],
    aliases: &HashMap<String, String>,
    prefix: &str,
    qualify_with_database: bool,
) {
    let referenced = referenced_tables
        .iter()
        .filter_map(|reference| find_table_by_reference(schema, reference))
        .filter(|table| !crate::relation_bindings::local(table))
        .collect::<Vec<_>>();
    if referenced.is_empty() {
        return;
    }
    let used_aliases = aliases
        .keys()
        .map(|alias| alias.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();

    let local_names: HashSet<_> = schema
        .tables
        .iter()
        .filter(|table| crate::relation_bindings::query_local(table))
        .map(|table| table.name.to_ascii_lowercase())
        .collect();
    for candidate in &schema.tables {
        if crate::relation_bindings::local(candidate) {
            continue;
        }
        if table_is_referenced(schema, candidate, referenced_tables)
            || identifier_match_rank(&candidate.name, prefix).is_none()
        {
            continue;
        }
        let candidate_alias = available_table_alias(&candidate.name, &used_aliases);
        let relation = if (qualify_with_database
            || local_names.contains(&candidate.name.to_ascii_lowercase()))
            && !schema.database.is_empty()
        {
            format!("{}.{}", schema.database, candidate.name)
        } else {
            candidate.name.clone()
        };

        for existing in &referenced {
            let existing_reference = aliases
                .iter()
                .find_map(|(alias, table)| {
                    table_matches(schema, existing, table).then_some(alias.as_str())
                })
                .unwrap_or(existing.name.as_str());

            for constraint in &candidate.constraints {
                if !is_foreign_key(constraint)
                    || !constraint_targets_table(schema, existing, constraint)
                {
                    continue;
                }
                push_foreign_key_join_item(
                    items,
                    &mut seen,
                    &relation,
                    &candidate_alias,
                    existing_reference,
                    &constraint.columns,
                    &constraint.referenced_columns,
                    constraint,
                );
            }

            for constraint in &existing.constraints {
                if !is_foreign_key(constraint)
                    || !constraint_targets_table(schema, candidate, constraint)
                {
                    continue;
                }
                push_foreign_key_join_item(
                    items,
                    &mut seen,
                    &relation,
                    &candidate_alias,
                    existing_reference,
                    &constraint.referenced_columns,
                    &constraint.columns,
                    constraint,
                );
            }
        }
    }
}

fn constraint_targets_table(schema: &Schema, table: &Table, constraint: &Constraint) -> bool {
    !crate::relation_bindings::local(table)
        && constraint
            .referenced_schema
            .as_ref()
            .is_none_or(|namespace| namespace.eq_ignore_ascii_case(&schema.database))
        && constraint
            .referenced_table
            .as_deref()
            .is_some_and(|reference| {
                crate::relation_bindings::catalog_table_matches(schema, table, reference)
            })
}

fn is_foreign_key(constraint: &Constraint) -> bool {
    constraint
        .constraint_type
        .to_ascii_uppercase()
        .contains("FOREIGN")
}

fn available_table_alias(table: &str, used: &HashSet<String>) -> String {
    let mut alias = table
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter_map(|part| part.chars().next())
        .collect::<String>()
        .to_ascii_lowercase();
    if alias.is_empty() {
        alias = table
            .chars()
            .find(|ch| ch.is_ascii_alphanumeric())
            .unwrap_or('t')
            .to_ascii_lowercase()
            .to_string();
    }
    if !used.contains(&alias) {
        return alias;
    }
    for suffix in 2..100 {
        let candidate = format!("{alias}{suffix}");
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    format!("{alias}_join")
}

#[allow(clippy::too_many_arguments)]
fn push_foreign_key_join_item(
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    relation: &str,
    candidate_alias: &str,
    existing_reference: &str,
    candidate_columns: &[String],
    existing_columns: &[String],
    constraint: &Constraint,
) {
    if candidate_columns.is_empty() || candidate_columns.len() != existing_columns.len() {
        return;
    }
    let predicate = candidate_columns
        .iter()
        .zip(existing_columns)
        .map(|(candidate, existing)| {
            format!("{candidate_alias}.{candidate} = {existing_reference}.{existing}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let insert_text = format!("{relation} {candidate_alias} ON {predicate}");
    if !seen.insert(insert_text.to_ascii_lowercase()) {
        return;
    }
    items.push(CompletionItem {
        label: insert_text.clone(),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some(format!("Foreign-key JOIN via {}", constraint.name)),
        documentation: constraint
            .definition
            .as_ref()
            .map(|definition| Documentation::String(definition.clone())),
        sort_text: Some(completion_sort_text("0", relation)),
        filter_text: Some(
            relation
                .split('.')
                .next_back()
                .unwrap_or(relation)
                .to_string(),
        ),
        insert_text: Some(insert_text),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    });
}

pub(crate) fn add_schema_functions(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    prefix: &str,
    sort_prefix: &str,
    qualify_with_database: bool,
) {
    for function in &schema.functions {
        if !prefix.is_empty() && !function.name.to_lowercase().starts_with(prefix) {
            continue;
        }
        let mut item = create_function_item(function, schema, qualify_with_database);
        set_completion_sort_text(&mut item, sort_prefix, &function.name);
        items.push(item);
    }
}

pub(crate) fn add_schema_columns(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    referenced_tables: &[String],
    use_table_prefix: bool,
    prefix: &str,
    sort_prefix: &str,
) {
    let candidate_tables = schema
        .tables
        .iter()
        .filter(|table| table_is_referenced(schema, table, referenced_tables))
        .collect::<Vec<_>>();
    let mut column_sources = HashMap::<String, usize>::new();
    for table in &candidate_tables {
        let mut table_columns = HashSet::new();
        for column in &table.columns {
            let normalized = column.name.to_ascii_lowercase();
            if table_columns.insert(normalized.clone()) {
                *column_sources.entry(normalized).or_default() += 1;
            }
        }
    }
    let mut seen_sources = HashSet::new();

    for (table_order, table) in candidate_tables.into_iter().enumerate() {
        let relation_order = referenced_tables
            .iter()
            .position(|reference| table_matches(schema, table, reference))
            .unwrap_or(table_order);

        for column in &table.columns {
            let Some(match_rank) = crate::identifier_search::column_match_rank(column, prefix)
            else {
                continue;
            };
            let source_key = format!(
                "{}\u{0}{}\u{0}{}\u{0}{}",
                crate::relation_bindings::query_local(table),
                schema.database.to_ascii_lowercase(),
                table.name.to_ascii_lowercase(),
                column.name.to_ascii_lowercase()
            );
            if !seen_sources.insert(source_key) {
                continue;
            }

            let is_ambiguous = column_sources
                .get(&column.name.to_ascii_lowercase())
                .copied()
                .unwrap_or_default()
                > 1;
            let table_name = if use_table_prefix || (referenced_tables.is_empty() && is_ambiguous) {
                Some(table.name.as_str())
            } else {
                None
            };
            let mut item = create_column_item(column, table_name);
            let namespace = schema
                .catalog
                .as_deref()
                .map(|catalog| format!("{catalog}.{}", schema.database))
                .unwrap_or_else(|| schema.database.clone());
            let namespace = schema
                .server
                .as_deref()
                .map(|server| format!("{server}.{namespace}"))
                .unwrap_or(namespace);
            item.detail = Some(format!(
                "Column: {}.{}.{} ({}){}",
                namespace,
                table.name,
                column.name,
                column.data_type,
                column_visibility_suffix(column)
            ));
            item.data = Some(json!({
                "oxide": {
                    "kind": "column",
                    "queryLocal": crate::relation_bindings::query_local(table),
                    "catalog": schema.catalog,
                    "schema": schema.database,
                    "table": table.name,
                    "column": column.name,
                    "searchComment": column.comment.as_deref().map(crate::identifier_search::comment_prefix),
                }
            }));
            if let Some(server) = &schema.server {
                item.data.as_mut().unwrap()["oxide"]["server"] = json!(server);
            }
            item.sort_text = Some(format!(
                "{}:{:01}:{:02}:{:04}:{}:{}",
                sort_prefix,
                u8::from(column.hidden),
                match_rank,
                relation_order,
                column.name.to_ascii_lowercase(),
                table.name.to_ascii_lowercase()
            ));
            items.push(item);
        }
    }
}

/// Returns a bounded semantic match tier shared by schema fields and local
/// relation aliases. Keeping aliases on this same ladder makes the popup
/// consistent with DBX's identifier completion: exact, prefix, abbreviation,
/// then ordered-subsequence matches.
pub(crate) fn identifier_match_rank(candidate: &str, prefix: &str) -> Option<u8> {
    if prefix.is_empty() {
        return Some(0);
    }

    let candidate_lower = candidate.to_lowercase();
    let prefix_lower = prefix.to_lowercase();
    if candidate_lower == prefix_lower {
        return Some(0);
    }
    if candidate_lower.starts_with(&prefix_lower) {
        return Some(1);
    }

    let abbreviation = identifier_abbreviation(candidate);
    if abbreviation.starts_with(&prefix_lower) {
        return Some(2);
    }
    if let Some(rank) = crate::identifier_search::pinyin_rank(candidate, &prefix_lower) {
        return Some(rank);
    }
    is_subsequence(&prefix_lower, &candidate_lower).then_some(3)
}

fn identifier_abbreviation(candidate: &str) -> String {
    let mut abbreviation = String::new();
    let mut previous_is_separator = true;
    let mut previous_is_lowercase = false;
    for ch in candidate.chars() {
        if !ch.is_alphanumeric() {
            previous_is_separator = true;
            previous_is_lowercase = false;
            continue;
        }
        if previous_is_separator || (previous_is_lowercase && ch.is_uppercase()) {
            abbreviation.extend(ch.to_lowercase());
        }
        previous_is_separator = false;
        previous_is_lowercase = ch.is_lowercase();
    }
    abbreviation
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut needle_chars = needle.chars();
    let mut next = needle_chars.next();
    for ch in haystack.chars() {
        if next == Some(ch) {
            next = needle_chars.next();
            if next.is_none() {
                return true;
            }
        }
    }
    next.is_none()
}

pub(crate) fn apply_column_aliases(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    aliases: &[RelationAlias],
) {
    if aliases.is_empty() {
        return;
    }

    let tables: HashMap<_, _> = schema
        .tables
        .iter()
        .map(|table| {
            (
                (
                    table.name.as_str(),
                    crate::relation_bindings::query_local(table),
                ),
                table,
            )
        })
        .collect();
    let mut expanded = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let Some(source) = completion_column_source(&item) else {
            expanded.push(item);
            continue;
        };
        let matching_aliases = aliases
            .iter()
            .filter(|alias| {
                tables
                    .get(&(source.table.as_str(), source.query_local))
                    .is_some_and(|table| {
                        table_matches(
                            schema,
                            table,
                            super::references::row_reference(schema, aliases, &alias.sql),
                        )
                    })
                    && source
                        .server
                        .as_deref()
                        .unwrap_or_default()
                        .eq_ignore_ascii_case(schema.server.as_deref().unwrap_or_default())
                    && source
                        .catalog
                        .as_deref()
                        .unwrap_or_default()
                        .eq_ignore_ascii_case(schema.catalog.as_deref().unwrap_or_default())
                    && source.schema.eq_ignore_ascii_case(&schema.database)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut matching_aliases = matching_aliases;
        matching_aliases.sort_by_key(|alias| alias.name.to_ascii_lowercase());

        let is_qualified = item.label != source.column;
        if matching_aliases.is_empty() || (!is_qualified && matching_aliases.len() == 1) {
            expanded.push(item);
            continue;
        }

        for alias in matching_aliases {
            let mut aliased = item.clone();
            let label = format!("{}.{}", alias.name, source.column);
            let insert_text = format!("{}.{}", alias.sql, source.column);
            aliased.label = label.clone();
            aliased.insert_text = Some(insert_text);
            // A qualified display label must remain searchable by the bare
            // column name, matching DBX/Monaco filtering behavior.
            aliased.filter_text = Some(source.column.clone());
            aliased.sort_text = aliased
                .sort_text
                .as_ref()
                .map(|sort_text| format!("{sort_text}:{}", alias.name.to_ascii_lowercase()));
            expanded.push(aliased);
        }
    }
    *items = expanded;
}

struct CompletionColumnSource {
    server: Option<String>,
    query_local: bool,
    catalog: Option<String>,
    schema: String,
    table: String,
    column: String,
}

fn completion_column_source(item: &CompletionItem) -> Option<CompletionColumnSource> {
    let source = item.data.as_ref()?.get("oxide")?;
    if source.get("kind")?.as_str()? != "column" {
        return None;
    }
    Some(CompletionColumnSource {
        server: source
            .get("server")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        query_local: source
            .get("queryLocal")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        catalog: source
            .get("catalog")
            .and_then(|catalog| catalog.as_str())
            .map(str::to_string),
        schema: source.get("schema")?.as_str()?.to_string(),
        table: source.get("table")?.as_str()?.to_string(),
        column: source.get("column")?.as_str()?.to_string(),
    })
}

pub(crate) fn add_schema_using_columns(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    referenced_tables: &[String],
    prefix: &str,
    sort_prefix: &str,
) {
    let referenced_schema_tables = referenced_tables
        .iter()
        .filter_map(|reference| find_table_by_reference(schema, reference))
        .collect::<Vec<_>>();

    let Some((right_table, left_tables)) = referenced_schema_tables.split_last() else {
        return;
    };

    if left_tables.is_empty() {
        add_schema_columns(items, schema, referenced_tables, false, prefix, sort_prefix);
        return;
    }

    let left_column_names = left_tables
        .iter()
        .flat_map(|table| table.columns.iter())
        .map(|column| column.name.to_ascii_lowercase())
        .collect::<HashSet<_>>();

    let mut seen = HashSet::new();
    for column in &right_table.columns {
        let column_key = column.name.to_ascii_lowercase();
        if !left_column_names.contains(&column_key) || !seen.insert(column_key) {
            continue;
        }
        if !prefix.is_empty() && !column.name.to_lowercase().starts_with(prefix) {
            continue;
        }

        let mut item = create_column_item(column, None);
        set_completion_sort_text(&mut item, sort_prefix, &column.name);
        items.push(item);
    }
}

pub(crate) fn add_reference_table_columns(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    table_reference: &str,
    prefix: &str,
    sort_prefix: &str,
) {
    let Some(table) = find_table_by_reference(schema, table_reference) else {
        return;
    };

    for column in &table.columns {
        if !prefix.is_empty() && !column.name.to_lowercase().starts_with(prefix) {
            continue;
        }

        let mut item = create_column_item(column, None);
        set_completion_sort_text(&mut item, sort_prefix, &column.name);
        items.push(item);
    }
}

pub(crate) fn add_schema_constraints(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    referenced_tables: &[String],
    prefix: &str,
    sort_prefix: &str,
) {
    for table in &schema.tables {
        if !table_is_referenced(schema, table, referenced_tables) {
            continue;
        }

        for constraint in &table.constraints {
            if !prefix.is_empty() && !constraint.name.to_lowercase().starts_with(prefix) {
                continue;
            }

            let mut item = create_constraint_item(constraint);
            set_completion_sort_text(&mut item, sort_prefix, &constraint.name);
            items.push(item);
        }
    }
}

pub(crate) fn add_schema_conflict_constraints(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    referenced_tables: &[String],
    prefix: &str,
    sort_prefix: &str,
) {
    for table in &schema.tables {
        if !table_is_referenced(schema, table, referenced_tables) {
            continue;
        }

        for constraint in &table.constraints {
            let constraint_type = constraint.constraint_type.to_ascii_uppercase();
            if !constraint_type.contains("PRIMARY")
                && !constraint_type.contains("UNIQUE")
                && !constraint_type.contains("EXCLUSION")
            {
                continue;
            }
            if !prefix.is_empty() && !constraint.name.to_lowercase().starts_with(prefix) {
                continue;
            }

            let mut item = create_constraint_item(constraint);
            set_completion_sort_text(&mut item, sort_prefix, &constraint.name);
            items.push(item);
        }
    }
}

pub(crate) fn add_schema_indexes(
    items: &mut Vec<CompletionItem>,
    schema: &Schema,
    referenced_tables: &[String],
    prefix: &str,
    sort_prefix: &str,
    qualify_with_database: bool,
) {
    let mut seen = HashSet::new();
    for table in &schema.tables {
        if !table_is_referenced(schema, table, referenced_tables) {
            continue;
        }

        for index in &table.indexes {
            if !prefix.is_empty() && !index.name.to_lowercase().starts_with(prefix) {
                continue;
            }
            if !seen.insert(index.name.to_ascii_lowercase()) {
                continue;
            }

            let mut item = create_index_item(index, schema, qualify_with_database);
            set_completion_sort_text(&mut item, sort_prefix, &index.name);
            items.push(item);
        }
    }
}
