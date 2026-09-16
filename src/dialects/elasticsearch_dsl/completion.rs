use super::{context, ElasticsearchDslDialect};
use crate::schema::Schema;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

impl ElasticsearchDslDialect {
    /// 创建字段补全项
    fn create_field_item(&self, field: &str, detail_prefix: &str) -> CompletionItem {
        self.create_field_item_with_insert(field, detail_prefix, true)
    }

    fn create_field_item_with_insert(
        &self,
        field: &str,
        detail_prefix: &str,
        quoted_insert: bool,
    ) -> CompletionItem {
        CompletionItem {
            label: field.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(format!("{}: {}", detail_prefix, field)),
            documentation: None,
            deprecated: None,
            preselect: None,
            sort_text: Some(format!("1{}", field)),
            filter_text: Some(field.to_string()),
            insert_text: Some(if quoted_insert {
                format!("\"{}\"", field)
            } else {
                field.to_string()
            }),
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

    fn create_query_type_item_with_insert(
        &self,
        query_type: &str,
        quoted_insert: bool,
    ) -> CompletionItem {
        CompletionItem {
            label: query_type.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(format!("Elasticsearch DSL query type: {}", query_type)),
            documentation: None,
            deprecated: None,
            preselect: None,
            sort_text: Some(format!("0{}", query_type)),
            filter_text: Some(query_type.to_string()),
            insert_text: Some(if quoted_insert {
                format!("\"{}\"", query_type)
            } else {
                query_type.to_string()
            }),
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

    fn create_agg_type_item_with_insert(
        &self,
        agg_type: &str,
        quoted_insert: bool,
    ) -> CompletionItem {
        CompletionItem {
            label: agg_type.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("Elasticsearch aggregation: {}", agg_type)),
            documentation: None,
            deprecated: None,
            preselect: None,
            sort_text: Some(format!("2{}", agg_type)),
            filter_text: Some(agg_type.to_string()),
            insert_text: Some(if quoted_insert {
                format!("\"{}\"", agg_type)
            } else {
                agg_type.to_string()
            }),
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

    fn create_index_item(
        &self,
        table: &crate::schema::Table,
        quoted_insert: bool,
    ) -> CompletionItem {
        CompletionItem {
            label: table.name.clone(),
            kind: Some(CompletionItemKind::CLASS),
            detail: Some(format!("Elasticsearch Index: {}", table.name)),
            documentation: table
                .comment
                .clone()
                .map(tower_lsp::lsp_types::Documentation::String),
            deprecated: None,
            preselect: None,
            sort_text: Some(format!("3{}", table.name)),
            filter_text: Some(table.name.clone()),
            insert_text: Some(if quoted_insert {
                format!("\"{}\"", table.name)
            } else {
                table.name.clone()
            }),
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

    fn add_schema_field_items(
        &self,
        items: &mut Vec<CompletionItem>,
        schema: Option<&Schema>,
        prefix: &str,
    ) {
        self.add_schema_field_items_with_insert(items, schema, prefix, true);
    }

    fn add_schema_field_items_with_insert(
        &self,
        items: &mut Vec<CompletionItem>,
        schema: Option<&Schema>,
        prefix: &str,
        quoted_insert: bool,
    ) {
        let Some(schema) = schema else {
            return;
        };

        for table in &schema.tables {
            for column in &table.columns {
                if !prefix.is_empty() && !column.name.to_ascii_lowercase().starts_with(prefix) {
                    continue;
                }

                items.push(self.create_field_item_with_insert(
                    &column.name,
                    &format!("Elasticsearch field in {}", table.name),
                    quoted_insert,
                ));
            }
        }
    }

    fn add_schema_index_items(
        &self,
        items: &mut Vec<CompletionItem>,
        schema: Option<&Schema>,
        prefix: &str,
        quoted_insert: bool,
    ) {
        let Some(schema) = schema else {
            return;
        };

        for table in &schema.tables {
            if !prefix.is_empty() && !table.name.to_ascii_lowercase().starts_with(prefix) {
                continue;
            }

            items.push(self.create_index_item(table, quoted_insert));
        }
    }

    fn add_top_level_items(
        &self,
        items: &mut Vec<CompletionItem>,
        schema: Option<&Schema>,
        prefix: &str,
        quoted_insert: bool,
    ) {
        for field in ES_TOP_LEVEL_FIELDS {
            if !elasticsearch_top_level_field_supported(field, schema) {
                continue;
            }
            if !prefix.is_empty() && !field.to_ascii_lowercase().starts_with(prefix) {
                continue;
            }

            items.push(self.create_field_item_with_insert(
                field,
                "Elasticsearch DSL field",
                quoted_insert,
            ));
        }
    }

    fn add_query_type_items(
        &self,
        items: &mut Vec<CompletionItem>,
        schema: Option<&Schema>,
        prefix: &str,
        quoted_insert: bool,
    ) {
        for query_type in ES_QUERY_TYPES {
            if !elasticsearch_query_type_supported(query_type, schema) {
                continue;
            }
            if !prefix.is_empty() && !query_type.to_ascii_lowercase().starts_with(prefix) {
                continue;
            }

            items.push(self.create_query_type_item_with_insert(query_type, quoted_insert));
        }
    }

    fn add_agg_type_items(
        &self,
        items: &mut Vec<CompletionItem>,
        prefix: &str,
        quoted_insert: bool,
    ) {
        for agg_type in ES_AGG_TYPES {
            if !prefix.is_empty() && !agg_type.to_ascii_lowercase().starts_with(prefix) {
                continue;
            }

            items.push(self.create_agg_type_item_with_insert(agg_type, quoted_insert));
        }
    }

    fn add_bool_field_items(
        &self,
        items: &mut Vec<CompletionItem>,
        prefix: &str,
        quoted_insert: bool,
    ) {
        for field in ES_BOOL_FIELDS {
            if !prefix.is_empty() && !field.to_ascii_lowercase().starts_with(prefix) {
                continue;
            }

            items.push(self.create_field_item_with_insert(
                field,
                "Bool query field",
                quoted_insert,
            ));
        }
    }
}

const ES_TOP_LEVEL_FIELDS: &[&str] = &[
    "query",
    "aggs",
    "aggregations",
    "sort",
    "from",
    "size",
    "source",
    "_source",
    "fields",
    "highlight",
    "suggest",
    "script_fields",
    "docvalue_fields",
    "stored_fields",
    "post_filter",
    "min_score",
    "timeout",
    "terminate_after",
    "collapse",
    "search_after",
    "track_total_hits",
    "runtime_mappings",
    "knn",
];

const ES_QUERY_TYPES: &[&str] = &[
    "match",
    "match_all",
    "match_none",
    "match_phrase",
    "match_phrase_prefix",
    "multi_match",
    "common",
    "query_string",
    "simple_query_string",
    "term",
    "terms",
    "range",
    "exists",
    "prefix",
    "wildcard",
    "regexp",
    "fuzzy",
    "type",
    "ids",
    "constant_score",
    "bool",
    "boosting",
    "dis_max",
    "function_score",
    "script_score",
    "percolate",
    "intervals",
    "combined_fields",
    "distance_feature",
    "rank_feature",
    "pinned",
];

fn elasticsearch_top_level_field_supported(field: &str, schema: Option<&Schema>) -> bool {
    let Some(schema) = schema else {
        return true;
    };
    match field {
        "runtime_mappings" => schema.supports_server_version(7, 11),
        "knn" => schema.supports_server_version(8, 0),
        _ => true,
    }
}

fn elasticsearch_query_type_supported(query_type: &str, schema: Option<&Schema>) -> bool {
    let Some(schema) = schema else {
        return true;
    };
    match query_type {
        "type" => schema.server_major_version().is_none_or(|major| major <= 6),
        "intervals" | "distance_feature" | "rank_feature" | "pinned" => {
            schema.supports_server_version(7, 0)
        }
        "combined_fields" => schema.supports_server_version(7, 13),
        _ => true,
    }
}

const ES_AGG_TYPES: &[&str] = &[
    "terms",
    "range",
    "date_range",
    "ip_range",
    "histogram",
    "date_histogram",
    "geo_distance",
    "geohash_grid",
    "geotile_grid",
    "filters",
    "adjacency_matrix",
    "sampler",
    "diversified_sampler",
    "global",
    "filter",
    "missing",
    "nested",
    "reverse_nested",
    "children",
    "parent",
    "cardinality",
    "avg",
    "sum",
    "min",
    "max",
    "stats",
    "extended_stats",
    "percentiles",
    "percentile_ranks",
    "top_hits",
    "scripted_metric",
    "matrix_stats",
    "bucket_script",
    "bucket_selector",
    "bucket_sort",
    "serial_diff",
    "moving_avg",
];

const ES_BOOL_FIELDS: &[&str] = &["must", "must_not", "should", "filter"];

pub(super) fn complete(
    dialect: &ElasticsearchDslDialect,
    dsl: &str,
    position: Position,
    schema: Option<&Schema>,
) -> Vec<CompletionItem> {
    // Console documents may expose mappings for many indices. Once the
    // request path names an index, completion must not mix identically
    // named fields from unrelated mappings.
    let scoped_schema = context::elasticsearch_index_at_position(dsl, position).and_then(|index| {
        let schema = schema?;
        let table = schema
            .tables
            .iter()
            .find(|table| table.name.eq_ignore_ascii_case(&index))?
            .clone();
        let mut scoped = schema.clone();
        scoped.tables = vec![table];
        Some(scoped)
    });
    let schema = scoped_schema.as_ref().or(schema);
    let prefix = crate::position::cursor_token_prefix(dsl, position, context::is_token_char);
    let hint = context::elasticsearch_completion_context(dsl, position);
    let quoted_insert = !hint.inside_string;
    let mut items = Vec::new();

    match hint.kind {
        context::EsCompletionKind::TopLevel => {
            dialect.add_top_level_items(&mut items, schema, &prefix, quoted_insert);
            return items;
        }
        context::EsCompletionKind::QueryType => {
            dialect.add_query_type_items(&mut items, schema, &prefix, quoted_insert);
            return items;
        }
        context::EsCompletionKind::AggType => {
            dialect.add_agg_type_items(&mut items, &prefix, quoted_insert);
            return items;
        }
        context::EsCompletionKind::BoolField => {
            dialect.add_bool_field_items(&mut items, &prefix, quoted_insert);
            return items;
        }
        context::EsCompletionKind::FieldName => {
            dialect.add_schema_field_items_with_insert(&mut items, schema, &prefix, quoted_insert);
            return items;
        }
        context::EsCompletionKind::IndexValue => {
            dialect.add_schema_index_items(&mut items, schema, &prefix, quoted_insert);
            return items;
        }
        context::EsCompletionKind::Broad => {}
    }

    let byte_position = crate::position::lsp_position_to_byte_position(dsl, position);
    let mut parser = dialect.dsl_parser.lock().unwrap();
    let (tree, _) = parser.parse_with_tree(dsl);

    // 分析补全上下文
    let context = if let Some(ref tree) = tree {
        if let Some(node) = parser.get_node_at_position(tree, byte_position) {
            parser.analyze_completion_context(node, dsl)
        } else {
            crate::parser::DslCompletionContext::Default
        }
    } else {
        crate::parser::DslCompletionContext::Default
    };

    // 根据上下文提供不同的补全
    match context {
        crate::parser::DslCompletionContext::TopLevel => {
            dialect.add_top_level_items(&mut items, schema, &prefix, true);
        }

        crate::parser::DslCompletionContext::QueryObject => {
            dialect.add_query_type_items(&mut items, schema, &prefix, true);
            dialect.add_schema_field_items(&mut items, schema, &prefix);
        }

        crate::parser::DslCompletionContext::AggsObject => {
            dialect.add_agg_type_items(&mut items, &prefix, true);
            dialect.add_schema_field_items(&mut items, schema, &prefix);
        }

        crate::parser::DslCompletionContext::BoolQuery => {
            dialect.add_bool_field_items(&mut items, &prefix, true);
            dialect.add_schema_field_items(&mut items, schema, &prefix);
        }

        crate::parser::DslCompletionContext::SortObject => {
            // sort 字段（可以是字段名或特殊值）
            dialect.add_schema_field_items(&mut items, schema, &prefix);

            // 排序方向
            if prefix.is_empty() || "_score".starts_with(&prefix) {
                items.push(dialect.create_field_item("_score", "Sort by score"));
            }
            if prefix.is_empty() || "_doc".starts_with(&prefix) {
                items.push(dialect.create_field_item("_doc", "Sort by document order"));
            }
        }

        crate::parser::DslCompletionContext::Default => {
            dialect.add_query_type_items(&mut items, schema, &prefix, true);
            dialect.add_top_level_items(&mut items, schema, &prefix, true);
            dialect.add_schema_field_items(&mut items, schema, &prefix);
        }
    }

    // 如果提供了 schema，添加索引名补全
    dialect.add_schema_index_items(&mut items, schema, &prefix, true);

    items
}
