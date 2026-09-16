//! Schema resolution for the SQL language server.
use crate::parser::SqlParser;
use crate::schema::Schema;
use crate::schema::SchemaId;
use crate::schema::SchemaManager;
use crate::schema::Table;
use dashmap::DashMap;
use tower_lsp::lsp_types::*;
pub(super) fn calculate_schema_match_score(tables: &[String], schema: &Schema) -> i32 {
    let mut score = 0;

    for table_name in tables {
        if schema
            .tables
            .iter()
            .any(|table| schema_table_matches(schema, table_name, table))
        {
            score += 10;
        }
    }

    let matched_count = tables
        .iter()
        .filter(|table_name| {
            schema
                .tables
                .iter()
                .any(|table| schema_table_matches(schema, table_name, table))
        })
        .count();

    if matched_count > 1 {
        score += matched_count as i32 * 2;
    }

    score
}

pub(super) fn infer_schema_id_from_tables(
    tables: &[String],
    schema_manager: &SchemaManager,
) -> Option<SchemaId> {
    let mut best_match: Option<(SchemaId, i32)> = None;
    let mut has_tie = false;

    for schema_id in schema_manager.list_ids() {
        let Some(schema) = schema_manager.get(schema_id) else {
            continue;
        };
        let score = calculate_schema_match_score(tables, &schema);
        if score <= 0 {
            continue;
        }

        match best_match {
            None => {
                best_match = Some((schema_id, score));
                has_tie = false;
            }
            Some((_, best_score)) if score > best_score => {
                best_match = Some((schema_id, score));
                has_tie = false;
            }
            Some((_, best_score)) if score == best_score => {
                has_tie = true;
            }
            _ => {}
        }
    }

    if has_tie {
        None
    } else {
        best_match.map(|(schema_id, _)| schema_id)
    }
}

pub(super) fn infer_schema_id_from_text(
    text: &str,
    schema_manager: &SchemaManager,
) -> Option<SchemaId> {
    let mut parser = SqlParser::new();
    let parse_result = parser.parse(text);
    let tree = parse_result.tree?;
    let tables = parser.extract_referenced_tables(&tree, text);
    if tables.is_empty() {
        return None;
    }

    // 只有唯一最高分才推断，避免多个 schema 拥有同名表时随机绑定错误上下文。
    infer_schema_id_from_tables(&tables, schema_manager)
}

pub(super) fn schema_id_for_file(
    uri: &str,
    text: Option<&str>,
    configured_file_schemas: &DashMap<String, SchemaId>,
    inferred_file_schemas: &DashMap<String, SchemaId>,
    schema_manager: &SchemaManager,
) -> Option<SchemaId> {
    if let Some(schema_id) = configured_file_schemas
        .get(uri)
        .map(|schema_id| *schema_id.value())
    {
        if schema_manager.get(schema_id).is_some() {
            return Some(schema_id);
        }
    }

    if let Some(schema_id) = inferred_file_schemas
        .get(uri)
        .map(|schema_id| *schema_id.value())
    {
        if schema_manager.get(schema_id).is_some() {
            return Some(schema_id);
        }
        inferred_file_schemas.remove(uri);
    }

    let schema_id = infer_schema_id_from_text(text?, schema_manager)?;
    inferred_file_schemas.insert(uri.to_string(), schema_id);
    Some(schema_id)
}

pub(super) fn schema_qualifier_at_position(text: &str, position: Position) -> Option<String> {
    let byte_position = SqlParser::lsp_position_to_byte_position(text, position);
    SqlParser::column_qualifier_before_position(text, byte_position)
}

pub(super) fn schema_table_matches(schema: &Schema, reference: &str, table: &Table) -> bool {
    SqlParser::table_name_matches_with_catalog(
        reference,
        schema.catalog.as_deref(),
        &schema.database,
        &table.name,
    )
}

pub(super) fn find_schema_by_qualifier(
    schema_manager: &SchemaManager,
    qualifier: &str,
) -> Option<Schema> {
    let normalized_qualifier = SqlParser::normalize_identifier(qualifier);
    if normalized_qualifier.is_empty() {
        return None;
    }

    let qualifier_has_catalog = normalized_qualifier.contains('.');
    let mut matched_schema = None;
    for schema_id in schema_manager.list_ids() {
        let Some(schema) = schema_manager.get(schema_id) else {
            continue;
        };
        let normalized_database = SqlParser::normalize_identifier(&schema.database);
        let normalized_namespace = schema
            .catalog
            .as_deref()
            .map(SqlParser::normalize_identifier)
            .filter(|catalog| !catalog.is_empty())
            .map(|catalog| format!("{catalog}.{normalized_database}"))
            .unwrap_or_else(|| normalized_database.clone());
        let is_match = normalized_namespace.eq_ignore_ascii_case(&normalized_qualifier)
            || (!qualifier_has_catalog
                && normalized_database.eq_ignore_ascii_case(&normalized_qualifier));
        if !is_match {
            continue;
        }
        if matched_schema.is_some() {
            return None;
        }
        matched_schema = Some(schema);
    }

    matched_schema
}

pub(super) fn find_schema_by_table_reference(
    schema_manager: &SchemaManager,
    table_reference: &str,
) -> Option<Schema> {
    let mut matched_schema: Option<Schema> = None;

    for schema_id in schema_manager.list_ids() {
        let Some(schema) = schema_manager.get(schema_id) else {
            continue;
        };

        let has_table = schema
            .tables
            .iter()
            .any(|table| schema_table_matches(&schema, table_reference, table));
        if !has_table {
            continue;
        }

        if matched_schema.is_some() {
            return None;
        }
        matched_schema = Some(schema);
    }

    matched_schema
}

pub(super) fn schema_for_table_column_at_position(
    schema_manager: &SchemaManager,
    text: &str,
    position: Position,
) -> Option<Schema> {
    let mut parser = SqlParser::new();
    let byte_position = SqlParser::lsp_position_to_byte_position(text, position);
    let parse_result = parser.parse(text);
    let tree = parse_result.tree.as_ref()?;
    let table_name =
        SqlParser::column_qualifier_before_position(text, byte_position).or_else(|| {
            let node = parser.get_node_at_position(tree, byte_position)?;
            parser.get_table_name_for_column(node, text)
        })?;
    let aliases = SqlParser::relation_aliases_at_position(text, byte_position);
    let table_reference = SqlParser::resolve_relation_alias(&aliases, &table_name)
        .map(|alias| alias.relation.as_str())
        .unwrap_or(table_name.as_str());

    find_schema_by_table_reference(schema_manager, table_reference)
}

/// Product identities registered as compatibility aliases but not represented
/// by a standalone parser. Their suffix is enough to retain product-specific
/// completions while a client configuration update is still in flight.
pub(super) const PRODUCT_SQL_FILE_SUFFIX_DIALECTS: &[(&str, &str)] = &[
    ("access", "access"),
    ("argo", "argo"),
    ("cloudflare-d1", "cloudflare-d1"),
    ("cockroachdb", "cockroachdb"),
    ("dameng", "dameng"),
    ("databend", "databend"),
    ("db2", "db2"),
    ("doris", "doris"),
    ("exasol", "exasol"),
    ("firebird", "firebird"),
    ("gaussdb", "gaussdb"),
    ("gbase", "gbase"),
    ("goldendb", "goldendb"),
    ("greenplum", "greenplum"),
    ("h2", "h2"),
    ("highgo", "highgo"),
    ("informix", "informix"),
    ("iris", "iris"),
    ("kingbase", "kingbase"),
    ("kwdb", "kwdb"),
    ("manticoresearch", "manticoresearch"),
    ("oceanbase-oracle", "oceanbase-oracle"),
    ("opengauss", "opengauss"),
    ("oracle", "oracle"),
    ("questdb", "questdb"),
    ("redshift", "redshift"),
    ("sqlserver", "sqlserver"),
    ("starrocks", "starrocks"),
    ("sundb", "sundb"),
    ("tidb", "tidb"),
    ("timescaledb", "timescaledb"),
    ("turso", "turso"),
    ("vastbase", "vastbase"),
    ("vertica", "vertica"),
    ("xugu", "xugu"),
    ("yashandb", "yashandb"),
    ("yugabytedb", "yugabytedb"),
];
