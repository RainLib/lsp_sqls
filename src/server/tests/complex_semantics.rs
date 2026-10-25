//! User-facing completion contracts, through the LanguageServer method rather
//! than keyword recognition. Fixtures use synthetic, disjoint table columns.
use super::*;
use crate::server::SqlLspServer;
use tower_lsp::lsp_types::{CompletionParams, CompletionResponse};
use tower_lsp::{LanguageServer, LspService};

const DIALECTS: &[&str] = &["postgres", "mysql", "sqlite", "hive", "clickhouse"];
mod correlated_outputs;
mod correlation;
mod default_schemas;
mod document_bindings;
mod hive_lateral_views;
mod join_operands;
mod joined_tables;
mod lateral_groups;
mod metadata_search;
mod namespaces;
mod predicate_outputs;
mod qualify;
mod quantified;
mod query_grammar;
mod quoted_names;
mod recursive;
mod routine_inputs;
mod scalar_outputs;
mod search;
mod server_namespaces;
mod tsql_identifiers;
mod variable_scopes;
mod variables;
mod vendor_search;
mod visibility;

fn corpus_schema() -> Schema {
    let mut schema = semantic_test_schema();
    schema.tables[0].columns.push(Column {
        name: "payload".into(),
        data_type: "json".into(),
        ..Default::default()
    });
    schema.tables[0].columns.push(Column {
        name: "columns".into(),
        data_type: "json".into(),
        ..Default::default()
    });
    schema.tables.push(Table {
        name: "orders".into(),
        columns: [
            ("id", "bigint"),
            ("customer_id", "bigint"),
            ("total", "decimal"),
            ("status", "text"),
            ("created_at", "timestamp"),
        ]
        .into_iter()
        .map(|(name, data_type)| Column {
            name: name.into(),
            data_type: data_type.into(),
            ..Default::default()
        })
        .collect(),
        ..Default::default()
    });
    schema.tables.push(Table {
        name: "audit_events".into(),
        columns: vec![Column {
            name: "audit_secret".into(),
            data_type: "text".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    schema
}

struct Harness {
    service: LspService<SqlLspServer>,
    _socket: tower_lsp::ClientSocket,
    uri: Url,
    position: Position,
}

fn configured(dialect: &str, marked: &str) -> Harness {
    assert_eq!(marked.matches("<cursor>").count(), 1);
    let offset = marked.find("<cursor>").unwrap();
    let position = lsp_position_at_end(&marked[..offset]);
    let sql = marked.replace("<cursor>", "");
    let uri = Url::parse(&format!("oxide://query/corpus.{dialect}.sql")).unwrap();
    let (service, socket) = LspService::new(SqlLspServer::new);
    let server = service.inner();
    let schema_id = server.schema_manager.register(corpus_schema());
    server
        .configured_file_dialects
        .insert(uri.to_string(), dialect.into());
    server
        .configured_file_schemas
        .insert(uri.to_string(), schema_id);
    server.document_manager.update(uri.to_string(), sql, 1);
    Harness {
        service,
        _socket: socket,
        uri,
        position,
    }
}

async fn candidates(dialect: &str, marked: &str) -> Vec<CompletionItem> {
    let harness = configured(dialect, marked);
    complete_harness(&harness).await
}

async fn complete_harness(harness: &Harness) -> Vec<CompletionItem> {
    let params: CompletionParams = serde_json::from_value(serde_json::json!({
        "textDocument":{"uri":harness.uri},"position":harness.position,
    }))
    .unwrap();
    match harness
        .service
        .inner()
        .completion(params)
        .await
        .unwrap()
        .expect("configured completion")
    {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    }
}

#[tokio::test]
async fn schema_refresh_keeps_same_sql_but_replaces_field_candidates() {
    let harness = configured("mysql", "SELECT u.<cursor> FROM users u");
    assert!(complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "name"));
    let server = harness.service.inner();
    let id = *server
        .configured_file_schemas
        .get(harness.uri.as_str())
        .unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    schema.tables[0]
        .columns
        .iter_mut()
        .find(|column| column.name == "name")
        .unwrap()
        .name = "refreshed_name".into();
    assert!(server.schema_manager.update(id, schema));
    let items = complete_harness(&harness).await;
    assert!(items.iter().any(|item| item.label == "refreshed_name"));
    assert!(!items.iter().any(|item| item.label == "name"));
}

#[tokio::test]
async fn removed_schema_does_not_leave_cached_physical_columns() {
    let harness = configured("postgres", "SELECT u.<cursor> FROM users u");
    assert!(complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "email"));
    let server = harness.service.inner();
    let id = *server
        .configured_file_schemas
        .get(harness.uri.as_str())
        .unwrap();
    assert!(server.schema_manager.remove(id));
    assert!(!complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.kind == Some(CompletionItemKind::FIELD)));
}

#[tokio::test]
async fn dialect_switch_replaces_builtin_candidates_with_same_document_text() {
    let harness = configured("mysql", "SELECT uuid_s<cursor>");
    assert!(complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "UUID_SHORT"));
    harness
        .service
        .inner()
        .configured_file_dialects
        .insert(harness.uri.to_string(), "postgres".into());
    assert!(!complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "UUID_SHORT"));
}

#[tokio::test]
async fn edit_in_same_uri_rebinds_relation_without_stale_fields() {
    let harness = configured("mysql", "SELECT u.<cursor> FROM users u");
    assert!(complete_harness(&harness)
        .await
        .iter()
        .any(|item| item.label == "email"));
    harness.service.inner().document_manager.update(
        harness.uri.to_string(),
        "SELECT u. FROM orders u".into(),
        2,
    );
    let items = complete_harness(&harness).await;
    assert!(items.iter().any(|item| item.label == "total"));
    assert!(!items.iter().any(|item| item.label == "email"));
}

async fn assert_members(dialect: &str, sql: &str, expected: &[&str], forbidden: &[&str]) {
    let items = candidates(dialect, sql).await;
    let fields: Vec<_> = items
        .iter()
        .filter(|item| item.kind == Some(CompletionItemKind::FIELD))
        .map(|item| item.label.as_str())
        .collect();
    for expected in expected {
        assert!(
            fields.contains(expected),
            "{dialect}: missing {expected}; fields={fields:?}; SQL={sql}"
        );
    }
    for forbidden in forbidden.iter().copied().chain(["audit_secret"]) {
        assert!(
            !fields.contains(&forbidden),
            "{dialect}: leaked {forbidden}; fields={fields:?}; SQL={sql}"
        );
    }
}

macro_rules! common_case {
    ($name:ident,$sql:expr,[$($expected:expr),*],[$($forbidden:expr),*])=>{
        #[tokio::test]
        async fn $name(){
            for dialect in DIALECTS {
                assert_members(dialect,$sql,&[$($expected),*],&[$($forbidden),*]).await;
            }
        }
    };
}

common_case!(
    joined_predicate_member,
    "SELECT * FROM users u JOIN orders o ON o.customer_id=u.id WHERE u.<cursor>",
    ["id", "name", "email"],
    ["total", "customer_id"]
);
common_case!(
    correlated_exists_uses_outer_relation,
    "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.customer_id=u.<cursor>)",
    ["id", "name"],
    ["total", "customer_id"]
);
common_case!(
    inner_alias_shadows_outer_relation,
    "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders u WHERE u.<cursor>)",
    ["total", "customer_id"],
    ["name", "email"]
);
common_case!(sibling_derived_projection_isolated,
    "SELECT * FROM (SELECT id AS left_id FROM users) l JOIN (SELECT id AS right_id FROM orders) r ON r.<cursor>=l.left_id",
    ["right_id"],["left_id","name","total"]);
common_case!(chained_cte_aggregate_projection,
    "WITH base AS (SELECT id AS user_id,name FROM users), totals AS (SELECT user_id,count(*) AS n FROM base GROUP BY user_id) SELECT t.<cursor> FROM totals t",
    ["user_id","n"],["name","id","total"]);
common_case!(derived_case_keeps_output_alias,
    "SELECT d.<cursor> FROM (SELECT id,CASE WHEN name IS NULL THEN 'FROM;--' ELSE name END AS display_name FROM users) d",
    ["id","display_name"],["email","total"]);
common_case!(aggregate_having_projection,
    "SELECT t.<cursor> FROM (SELECT customer_id,SUM(total) AS revenue FROM orders GROUP BY customer_id HAVING SUM(total)>0) t",
    ["customer_id","revenue"],["total","name"]);
common_case!(window_cte_projection,
    "WITH ranked AS (SELECT customer_id,ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY created_at DESC) AS rn FROM orders) SELECT r.<cursor> FROM ranked r",
    ["customer_id","rn"],["total","created_at"]);
common_case!(union_output_names_come_from_first_arm,
    "SELECT x.<cursor> FROM (SELECT id AS entity_id,name AS label FROM users UNION ALL SELECT id AS ignored_second_name,status FROM orders) x",
    ["entity_id","label"],["ignored_second_name","status"]);
common_case!(
    prior_statement_does_not_leak_members,
    "SELECT o.total FROM orders o; SELECT u.<cursor> FROM users u",
    ["id", "name"],
    ["total", "customer_id"]
);
common_case!(
    function_argument_resolves_later_from,
    "SELECT COALESCE(u.<cursor>,'unknown') FROM users u",
    ["id", "name", "email"],
    ["total"]
);
common_case!(
    unicode_before_member_prefix,
    "SELECT '雪😀' AS note,u.na<cursor> FROM users u",
    ["name"],
    ["total"]
);
common_case!(nested_alias_does_not_escape_query_block,
    "SELECT d.<cursor> FROM (SELECT id AS projected_id FROM users) d WHERE EXISTS (SELECT 1 FROM orders d WHERE d.total>0)",
    ["projected_id"],["total","customer_id","email"]);
common_case!(explicit_cte_columns_replace_projection_names,
    "WITH renamed(user_key,display_name) AS (SELECT id,name FROM users) SELECT r.<cursor> FROM renamed r",
    ["user_key","display_name"],["id","name","email"]);
common_case!(qualified_star_through_cte_and_derived,
    "WITH base AS (SELECT id AS user_key,name FROM users) SELECT d.<cursor> FROM (SELECT b.* FROM base b) d",
    ["user_key","name"],["id","total"]);
common_case!(window_frame_does_not_change_output_shape,
    "SELECT d.<cursor> FROM (SELECT customer_id,SUM(total) OVER (PARTITION BY customer_id ORDER BY created_at ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running_total FROM orders) d",
    ["customer_id","running_total"],["total","created_at"]);
common_case!(comments_and_string_semicolons_preserve_scope,
    "SELECT 'FROM orders o; SELECT *' AS note; /* JOIN audit_events a; */ SELECT u.<cursor> FROM users u -- ignored ; orders o",
    ["id","name"],["total","audit_secret"]);
common_case!(
    incomplete_predicate_preserves_relation,
    "SELECT * FROM users u JOIN orders o ON (o.customer_id=u.id AND (u.<cursor>",
    ["id", "name"],
    ["total", "customer_id"]
);

#[tokio::test]
async fn recursive_cte_explicit_outputs() {
    for dialect in ["postgres", "mysql", "sqlite"] {
        assert_members(dialect,"WITH RECURSIVE walk(user_key,depth) AS (SELECT id,0 FROM users UNION ALL SELECT w.user_key,w.depth+1 FROM walk w WHERE w.depth<10) SELECT w.<cursor> FROM walk w",&["user_key","depth"],&["id","name"]).await;
    }
}

#[tokio::test]
async fn postgres_materialized_cte_and_lateral_projection() {
    assert_members("postgres","WITH picked AS MATERIALIZED (SELECT id,name FROM users) SELECT s.<cursor> FROM picked p CROSS JOIN LATERAL (SELECT p.id AS owner_id,p.name AS owner_name) s",&["owner_id","owner_name"],&["id","name","email"]).await;
}

#[tokio::test]
async fn sqlserver_apply_projection() {
    assert_members("sqlserver","SELECT s.<cursor> FROM users u CROSS APPLY (SELECT u.id AS owner_id,u.name AS owner_name) s",&["owner_id","owner_name"],&["id","name","total"]).await;
}

#[tokio::test]
async fn member_insert_type_and_optional_edits_keep_qualifier() {
    let marked = "SELECT '雪😀' AS note,u.na<cursor> FROM users u";
    let prefix = marked.split("<cursor>").next().unwrap();
    let cursor = prefix.encode_utf16().count() as u32;
    for dialect in DIALECTS {
        let items = candidates(dialect, marked).await;
        let item = items
            .iter()
            .find(|item| item.label == "name" && item.kind == Some(CompletionItemKind::FIELD))
            .expect("name completion");
        assert!(
            item.detail
                .as_deref()
                .is_some_and(|detail| detail.to_ascii_lowercase().contains("text")),
            "{dialect}: source type missing: {item:?}"
        );
        let edit = match item.text_edit.as_ref() {
            Some(CompletionTextEdit::Edit(edit)) => Some(edit.clone()),
            Some(CompletionTextEdit::InsertAndReplace(edit)) => Some(TextEdit {
                range: edit.replace,
                new_text: edit.new_text.clone(),
            }),
            None => None,
        };
        if let Some(edit) = &edit {
            assert_eq!(
                edit.range,
                Range::new(Position::new(0, cursor - 2), Position::new(0, cursor)),
                "{dialect}: UTF-16 replacement"
            );
        }
        // LSP also permits insertText with the client's word replacement.
        // Absence of an explicit textEdit is not itself a semantic defect.
        let insertion = edit
            .as_ref()
            .map(|edit| edit.new_text.as_str())
            .or(item.insert_text.as_deref())
            .unwrap_or(&item.label);
        assert!(
            ["name", "\"name\"", "`name`", "[name]"].contains(&insertion),
            "{dialect}: qualifier must stay outside replacement: {edit:?}"
        );
    }
}

#[tokio::test]
async fn json_table_nested_typed_columns() {
    for dialect in ["mysql", "oracle", "postgres"] {
        assert_members(dialect,"SELECT jt.<cursor> FROM users u, JSON_TABLE(u.payload,'$[*]' COLUMNS(entry_id INT PATH '$.id',ord FOR ORDINALITY,NESTED PATH '$.items[*]' COLUMNS(sku VARCHAR(30) PATH '$.sku',price DECIMAL(10,2) PATH '$.price'))) jt",&["entry_id","ord","sku","price"],&["id","payload","name"]).await;
    }
}

#[tokio::test]
async fn postgres_record_function_explicit_types() {
    assert_members("postgres","SELECT r.<cursor> FROM users u CROSS JOIN LATERAL jsonb_to_record(u.payload) AS r(city text,postal_code integer)",&["city","postal_code"],&["id","name","payload"]).await;
}

#[tokio::test]
async fn sqlserver_openjson_explicit_schema() {
    assert_members("sqlserver","SELECT j.<cursor> FROM users u CROSS APPLY OPENJSON(u.payload) WITH(city nvarchar(40) '$.city',items nvarchar(max) '$.items' AS JSON) j",&["city","items"],&["id","name","payload"]).await;
}

#[tokio::test]
async fn repeated_json_table_calls_keep_alias_shapes_separate() {
    for dialect in ["mysql", "oracle", "postgres"] {
        for (alias, expected, forbidden) in [
            ("a", "left_key", "right_key"),
            ("b", "right_key", "left_key"),
        ] {
            let sql=format!("SELECT {alias}.<cursor> FROM JSON_TABLE('[]','$[*]' COLUMNS(left_key INT PATH '$.id')) a CROSS JOIN JSON_TABLE('[]','$[*]' COLUMNS(right_key VARCHAR(30) PATH '$.id')) b");
            assert_members(dialect, &sql, &[expected], &[forbidden]).await;
        }
    }
}

#[tokio::test]
async fn escaped_declaration_identifiers_and_types() {
    for (dialect,sql,name) in [
        ("mysql","SELECT j.<cursor> FROM JSON_TABLE('[]','$[*]' COLUMNS(`display``name` VARCHAR(30) PATH '$.x')) j","display`name"),
        ("oracle","SELECT j.<cursor> FROM JSON_TABLE('[]','$[*]' COLUMNS(\"display\"\"name\" VARCHAR2(30) PATH '$.x')) j","display\"name"),
        ("postgres","SELECT r.<cursor> FROM jsonb_to_record('{}') AS r(\"display\"\"name\" text)","display\"name"),
        ("sqlserver","SELECT j.<cursor> FROM OPENJSON('{}') WITH([display]]name] nvarchar(30) '$.x') j","display]name"),
    ] { assert_members(dialect,sql,&[name],&["name"]).await; }
}

#[tokio::test]
async fn declaration_comments_newlines_and_precision_are_preserved() {
    let sql="SELECT j.<cursor> FROM JSON_TABLE(\n'[]', '$[*]'\n COLUMNS(\n /* column */ amount DECIMAL(10,2) PATH '$.x',\n NESTED PATH '$.nested[*]' COLUMNS(label VARCHAR(30) PATH '$.label'))) j";
    let items = candidates("mysql", sql).await;
    for (name, ty) in [("amount", "decimal(10,2)"), ("label", "varchar(30)")] {
        let item = items
            .iter()
            .find(|item| item.label == name)
            .expect("declared column");
        assert!(
            item.detail
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains(ty),
            "{item:?}"
        );
    }
}

#[tokio::test]
async fn json_expression_named_columns_is_not_a_declaration() {
    assert_members("mysql","SELECT j.<cursor> FROM users u,JSON_TABLE(u.columns,'$[*]' COLUMNS(result_id INT PATH '$.id')) j",&["result_id"],&["columns"]).await;
}

#[tokio::test]
async fn openjson_default_shape() {
    assert_members(
        "sqlserver",
        "SELECT j.<cursor> FROM users u CROSS APPLY OPENJSON(u.payload) j",
        &["key", "value", "type"],
        &["id", "payload"],
    )
    .await;
}

#[tokio::test]
async fn declaration_limits_do_not_publish_partial_shapes() {
    let columns = (0..257)
        .map(|index| format!("c{index} INT PATH '$.x'"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT j.<cursor> FROM JSON_TABLE('[]','$[*]' COLUMNS({columns})) j");
    let items = candidates("mysql", &sql).await;
    assert!(
        !items
            .iter()
            .any(|item| item.kind == Some(CompletionItemKind::FIELD)),
        "over-budget declarations must remain unknown"
    );
    let mut nested = "leaf INT PATH '$.x'".to_owned();
    for _ in 0..17 {
        nested = format!("NESTED PATH '$.x' COLUMNS({nested})");
    }
    let sql = format!("SELECT j.<cursor> FROM JSON_TABLE('[]','$[*]' COLUMNS({nested})) j");
    let items = candidates("mysql", &sql).await;
    assert!(
        !items
            .iter()
            .any(|item| item.kind == Some(CompletionItemKind::FIELD)),
        "over-depth declarations must remain unknown"
    );
}

#[tokio::test]
async fn large_whitespace_table_function_keeps_columns_within_budget() {
    let sql = format!(
        "SELECT j.<cursor> FROM JSON_TABLE('[]','$[*]'{} COLUMNS(id INT PATH '$.id')) j",
        " ".repeat(64 * 1024)
    );
    let offset = sql.find("<cursor>").unwrap();
    let position = lsp_position_at_end(&sql[..offset]);
    let text = sql.replace("<cursor>", "");
    let mut parser = crate::parser::SqlParser::new();
    let parse_started = std::time::Instant::now();
    let parsed = parser.parse(&text);
    let parse_time = parse_started.elapsed();
    let syntax_started = std::time::Instant::now();
    let mut syntax_parser = crate::parser::SqlParser::new();
    syntax_parser.parse_syntax_tree(&text).unwrap();
    let syntax_time = syntax_started.elapsed();
    let warm_syntax_started = std::time::Instant::now();
    syntax_parser.parse_syntax_tree(&text).unwrap();
    let warm_syntax_time = warm_syntax_started.elapsed();
    let byte_position = crate::parser::SqlParser::lsp_position_to_byte_position(&text, position);
    let context_started = std::time::Instant::now();
    let tree = parsed.tree.as_ref().unwrap();
    let node = parser.get_node_at_position(tree, byte_position).unwrap();
    let context = parser.analyze_completion_context(node, &text, byte_position);
    let context_time = context_started.elapsed();
    let alias_started = std::time::Instant::now();
    let aliases = crate::parser::SqlParser::relation_aliases_at_position(&text, byte_position);
    eprintln!("SQL phases: diagnostics_parse={parse_time:?}, syntax_cold={syntax_time:?}, syntax_warm={warm_syntax_time:?}, context={context_time:?} ({context:?}), aliases={:?} ({} aliases)",alias_started.elapsed(),aliases.len());
    let shape_started = std::time::Instant::now();
    let schema = augment_schema_with_local_relations(
        corpus_schema(),
        &text,
        position,
        "oxide://query/perf.mysql.sql",
        Some("mysql"),
    );
    let shape_time = shape_started.elapsed();
    let registry = DialectRegistry::new();
    let dialect = registry.get_by_name("mysql").unwrap();
    let dialect_started = std::time::Instant::now();
    let direct = dialect.completion(&text, position, Some(&schema)).await;
    assert!(direct.iter().any(|item| item.label == "id"));
    let dialect_time = dialect_started.elapsed();
    let harness = configured("mysql", &sql);
    let started = std::time::Instant::now();
    let cold = complete_harness(&harness).await;
    let server_cold = started.elapsed();
    let warm_started = std::time::Instant::now();
    let warm = complete_harness(&harness).await;
    let server_warm = warm_started.elapsed();
    for items in [cold, warm] {
        assert!(items.iter().any(|item| item.label == "id"));
        assert!(!items.iter().any(|item| item.label == "name"));
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "bounded 64 KiB whitespace regression: {:?}",
        started.elapsed()
    );
    eprintln!("64 KiB table-function whitespace: shape={shape_time:?}, dialect={dialect_time:?}, server_cold={server_cold:?}, server_warm={server_warm:?}");
}

#[tokio::test]
async fn renamed_projection_keeps_source_type_not_output_name_type() {
    for dialect in DIALECTS {
        let items=candidates(dialect,"WITH projected AS (SELECT u.id AS name FROM users u) SELECT p.<cursor> FROM projected p").await;
        let item = items
            .iter()
            .find(|item| item.label == "name")
            .expect("renamed output");
        assert!(
            item.detail
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains("bigint"),
            "{dialect}: alias must inherit id's type, not the unrelated name column: {item:?}"
        );
    }
}

#[tokio::test]
async fn typed_table_function_star_flows_through_cte_and_derived() {
    for dialect in ["mysql", "oracle", "postgres"] {
        assert_members(dialect,"WITH expanded AS (SELECT j.* FROM JSON_TABLE('[]','$[*]' COLUMNS(city VARCHAR(30) PATH '$.city',amount DECIMAL(10,2) PATH '$.amount')) j) SELECT e.<cursor> FROM expanded e",&["city","amount"],&["name","id"]).await;
    }
    assert_members("postgres","SELECT d.<cursor> FROM (SELECT r.* FROM jsonb_to_record('{}') AS r(city text,postal_code integer)) d",&["city","postal_code"],&["name","id"]).await;
}

#[tokio::test]
async fn multiple_table_function_stars_keep_both_call_shapes() {
    for projection in ["a.*,b.*", "*"] {
        let sql=format!("WITH expanded AS (SELECT {projection} FROM JSON_TABLE('[]','$[*]' COLUMNS(left_key INT PATH '$.id')) a CROSS JOIN JSON_TABLE('[]','$[*]' COLUMNS(right_key VARCHAR(30) PATH '$.id')) b) SELECT e.<cursor> FROM expanded e");
        assert_members("mysql", &sql, &["left_key", "right_key"], &["name", "id"]).await;
    }
}

#[tokio::test]
async fn declared_column_hover_and_definition_share_the_shape() {
    let sql =
        "SELECT r.ci<cursor>ty FROM jsonb_to_record('{}') AS r(\n city text, postal_code integer)";
    let harness = configured("postgres", sql);
    let params =
        serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position});
    let hover = harness
        .service
        .inner()
        .hover(serde_json::from_value(params.clone()).unwrap())
        .await
        .unwrap()
        .expect("typed column hover");
    assert!(serde_json::to_string(&hover)
        .unwrap()
        .to_ascii_lowercase()
        .contains("text"));
    let location = harness
        .service
        .inner()
        .goto_definition(serde_json::from_value(params).unwrap())
        .await
        .unwrap()
        .expect("declared column definition");
    let location = match location {
        tower_lsp::lsp_types::GotoDefinitionResponse::Scalar(value) => value,
        other => panic!("unexpected definition: {other:?}"),
    };
    assert_eq!(location.uri, harness.uri);
    assert_eq!(location.range.start.line, 1, "declaration line is retained");
}

#[tokio::test]
async fn complex_expression_completion_and_hover_keep_derived_types() {
    for (dialect, projection, expected) in [
        ("postgres", "COALESCE(SUM(o.customer_id), 0)", "numeric"),
        (
            "postgres",
            "CAST(CASE WHEN o.total > 0 THEN o.total ELSE 0 END AS decimal(18, 4))",
            "decimal(18, 4)",
        ),
        (
            "postgres",
            "ROW_NUMBER() OVER (PARTITION BY o.status ORDER BY o.created_at)",
            "bigint",
        ),
        ("mysql", "SUM(o.customer_id)", "decimal"),
        ("sqlite", "COUNT(*)", "integer"),
    ] {
        for wrapped in [
            format!("WITH totals AS (SELECT {projection} AS result FROM orders o) SELECT t.re<cursor>sult FROM totals t"),
            format!("SELECT t.re<cursor>sult FROM (SELECT {projection} AS result FROM orders o) t"),
        ] {
            let harness = configured(dialect, &wrapped);
            let items = complete_harness(&harness).await;
            let result = items.iter().find(|item| item.label == "result").unwrap_or_else(|| panic!("{dialect}: {wrapped}: {items:?}"));
            assert!(result.detail.as_ref().is_some_and(|detail| detail.to_ascii_lowercase().contains(expected)), "{dialect}: {wrapped}: {result:?}");
            let hover = harness.service.inner().hover(serde_json::from_value(serde_json::json!({
                "textDocument":{"uri":harness.uri},"position":harness.position,
            })).unwrap()).await.unwrap().expect("derived expression hover");
            assert!(serde_json::to_string(&hover).unwrap().to_ascii_lowercase().contains(expected), "{dialect}: {wrapped}: {hover:?}");
        }
    }
}

#[tokio::test]
async fn expression_inference_uses_configured_dialect_after_switch_on_same_uri() {
    let harness = configured("mysql", "WITH totals AS (SELECT SUM(o.customer_id) AS result FROM orders o) SELECT t.<cursor> FROM totals t");
    let before = complete_harness(&harness).await;
    assert!(before.iter().any(|item| item.label == "result"
        && item
            .detail
            .as_ref()
            .is_some_and(|detail| detail.contains("decimal"))));
    harness
        .service
        .inner()
        .configured_file_dialects
        .insert(harness.uri.to_string(), "postgres".into());
    let after = complete_harness(&harness).await;
    assert!(after.iter().any(|item| item.label == "result"
        && item
            .detail
            .as_ref()
            .is_some_and(|detail| detail.contains("numeric"))));
}

#[tokio::test]
async fn explicit_cte_names_keep_typed_candidates_across_dialects() {
    for dialect in [
        "postgres",
        "mysql",
        "sqlite",
        "hive",
        "clickhouse",
        "oracle",
        "sqlserver",
    ] {
        let items = candidates(dialect,"WITH first(renamed_id, renamed_name) AS (SELECT u.id, u.name FROM users u), second(final_id, final_name) AS (SELECT f.* FROM first f) SELECT s.<cursor> FROM second s").await;
        for (name, ty) in [("final_id", "bigint"), ("final_name", "text")] {
            let item = items
                .iter()
                .find(|item| item.label == name)
                .unwrap_or_else(|| panic!("{dialect}: {items:?}"));
            assert!(
                item.detail
                    .as_ref()
                    .is_some_and(|detail| detail.contains(ty)),
                "{dialect}: {item:?}"
            );
        }
        assert!(!items
            .iter()
            .any(|item| item.label == "renamed_id" || item.label == "renamed_name"));
    }
}

#[tokio::test]
async fn postgres_set_projection_names_types_and_hover_survive_compound_queries() {
    for (body,expected) in [
        ("SELECT 1::integer UNION ALL SELECT 2::bigint","bigint"),
        ("SELECT NULL UNION SELECT NULL INTERSECT SELECT 1","integer"),
        ("SELECT a.* FROM (SELECT 1::integer AS x) a UNION SELECT b.* FROM (SELECT 2::numeric AS y) b","numeric"),
        ("SELECT NULL::integer[]","integer[]"),
    ] {
        let harness=configured("postgres",&format!("/* 🧭 */ WITH merged(result) AS ({body}) SELECT m.re<cursor>sult FROM merged m"));
        let items=complete_harness(&harness).await;
        let item=items.iter().find(|item|item.label=="result").expect("positional candidate");
        assert!(item.detail.as_ref().is_some_and(|detail|detail.contains(expected)),"{body}: {item:?}");
        let hover=harness.service.inner().hover(serde_json::from_value(serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position})).unwrap()).await.unwrap().expect("compound output hover");
        assert!(serde_json::to_string(&hover).unwrap().contains(expected),"{body}: {hover:?}");
    }
}

#[tokio::test]
async fn arithmetic_and_try_cast_candidates_and_hover_use_dialect_semantics() {
    for (dialect, expression, expected) in [
        ("postgres", "SUM(o.customer_id * 2)", "numeric"),
        ("postgres", "COALESCE(SUM(-(o.total * 2)), 0)", "numeric"),
        ("mysql", "SUM(o.customer_id * 2)", "decimal"),
        ("mysql", "o.customer_id / 2", "decimal"),
        (
            "sqlserver",
            "TRY_CAST(o.status AS decimal(18,4)) + o.customer_id",
            "decimal",
        ),
        ("sqlserver", "TRY_CAST(o.status AS bigint)", "bigint"),
    ] {
        for sql in [
            format!("WITH calc(result) AS (SELECT {expression} FROM orders o) SELECT c.re<cursor>sult FROM calc c"),
            format!("SELECT c.re<cursor>sult FROM (SELECT {expression} AS result FROM orders o) c"),
        ] {
            let harness=configured(dialect,&sql);
            let items=complete_harness(&harness).await;
            let item=items.iter().find(|item|item.label=="result").expect("computed field candidate");
            assert!(item.detail.as_ref().is_some_and(|detail|detail.contains(expected)),"{dialect}: {expression}: {item:?}");
            let hover=harness.service.inner().hover(serde_json::from_value(serde_json::json!({"textDocument":{"uri":harness.uri},"position":harness.position})).unwrap()).await.unwrap().expect("computed field hover");
            assert!(serde_json::to_string(&hover).unwrap().contains(expected),"{dialect}: {hover:?}");
        }
    }
}
