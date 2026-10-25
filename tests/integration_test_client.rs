//! Bounded protocol tests of the compiled server, never a nested cargo build.
use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

#[path = "integration_test_client/ambiguity_scopes.rs"]
mod ambiguity_scopes;
#[path = "integration_test_client/analysis_configuration.rs"]
mod analysis_configuration;
#[path = "integration_test_client/analysis_consistency.rs"]
mod analysis_consistency;
#[path = "integration_test_client/analysis_provenance.rs"]
mod analysis_provenance;
#[path = "integration_test_client/auxiliary_documents.rs"]
mod auxiliary_documents;
#[path = "integration_test_client/call_completion.rs"]
mod call_completion;
#[path = "integration_test_client/catalog_types.rs"]
mod catalog_types;
#[path = "integration_test_client/completion_constraints.rs"]
mod completion_constraints;
#[path = "integration_test_client/default_schemas.rs"]
mod default_schemas;
#[path = "integration_test_client/dialect_values.rs"]
mod dialect_values;
#[path = "integration_test_client/document_bindings.rs"]
mod document_bindings;
#[path = "integration_test_client/document_completions.rs"]
mod document_completions;
#[path = "integration_test_client/document_semantics.rs"]
mod document_semantics;
#[path = "integration_test_client/document_structure_snapshot.rs"]
mod document_structure_snapshot;
#[path = "integration_test_client/execute_calls.rs"]
mod execute_calls;
#[path = "integration_test_client/execute_values.rs"]
mod execute_values;
#[path = "integration_test_client/expression_probes.rs"]
mod expression_probes;
#[path = "integration_test_client/hive_lateral_views.rs"]
mod hive_lateral_views;
#[path = "integration_test_client/inspection_regions.rs"]
mod inspection_regions;
#[path = "integration_test_client/inspection_scopes.rs"]
mod inspection_scopes;
#[path = "integration_test_client/join_operands.rs"]
mod join_operands;
#[path = "integration_test_client/joined_tables.rs"]
mod joined_tables;
#[path = "integration_test_client/lateral_groups.rs"]
mod lateral_groups;
#[path = "integration_test_client/metadata_search.rs"]
mod metadata_search;
#[path = "integration_test_client/named_arguments.rs"]
mod named_arguments;
#[path = "integration_test_client/pattern_continuations.rs"]
mod pattern_continuations;
#[path = "integration_test_client/predicate_slots.rs"]
mod predicate_slots;
#[path = "integration_test_client/qualify.rs"]
mod qualify;
#[path = "integration_test_client/server_namespaces.rs"]
mod server_namespaces;
#[path = "integration_test_client/shared_syntax.rs"]
mod shared_syntax;
#[path = "integration_test_client/tsql_modules.rs"]
mod tsql_modules;
#[path = "integration_test_client/udf_outputs.rs"]
mod udf_outputs;
#[path = "integration_test_client/variable_scopes.rs"]
mod variable_scopes;
#[path = "integration_test_client/variable_types.rs"]
mod variable_types;

#[path = "integration_test_client/predicate_outputs.rs"]
mod predicate_outputs;
#[path = "integration_test_client/quantified.rs"]
mod quantified;
#[path = "integration_test_client/query_grammar.rs"]
mod query_grammar;
#[path = "integration_test_client/routine_identity.rs"]
mod routine_identity;
#[path = "integration_test_client/routine_inputs.rs"]
mod routine_inputs;
#[path = "integration_test_client/routine_modes.rs"]
mod routine_modes;
#[path = "integration_test_client/routine_overloads.rs"]
mod routine_overloads;

struct LspProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    sequence: u64,
}
impl LspProcess {
    async fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sql-lsp"))
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut process = Self {
            child,
            stdin,
            stdout,
            sequence: 0,
        };
        let result = process
            .request(
                "initialize",
                json!({"processId":null,"rootUri":null,"capabilities":{}}),
            )
            .await;
        assert_eq!(result["capabilities"]["documentFormattingProvider"], true);
        process.notify("initialized", json!({})).await;
        process
    }
    async fn send(&mut self, value: Value) {
        let body = serde_json::to_vec(&value).unwrap();
        timeout(Duration::from_secs(10), async {
            self.stdin
                .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
                .await
                .unwrap();
            self.stdin.write_all(&body).await.unwrap();
            self.stdin.flush().await.unwrap();
        })
        .await
        .expect("LSP write deadline");
    }
    async fn notify(&mut self, method: &str, params: Value) {
        let mut message = json!({"jsonrpc":"2.0","method":method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message).await;
    }
    async fn frame(&mut self) -> Value {
        let mut length = None;
        let mut header_bytes = 0;
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).await.unwrap() > 0,
                "unexpected LSP EOF"
            );
            header_bytes += line.len();
            assert!(header_bytes <= 8192, "oversized LSP header");
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let length = length
            .filter(|length| *length > 0 && *length <= 4 * 1024 * 1024)
            .expect("bounded LSP content length");
        let mut body = vec![0; length];
        self.stdout.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }
    async fn until(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        timeout(Duration::from_secs(10), async {
            loop {
                let message = self.frame().await;
                if message.get("method").is_some() && message.get("id").is_some() {
                    self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":null}))
                        .await;
                } else if predicate(&message) {
                    return message;
                }
            }
        })
        .await
        .expect("LSP response deadline")
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.sequence += 1;
        let id = self.sequence;
        let mut message = json!({"jsonrpc":"2.0","id":id,"method":method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message).await;
        let result = self.until(|message| message["id"] == id).await;
        assert!(result.get("error").is_none(), "{method}: {result}");
        result["result"].clone()
    }
    async fn open(&mut self, uri: &str, sql: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"mysql","version":1,"text":sql}}),
        )
        .await;
        self.until(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .await;
    }
    async fn stop(mut self) {
        self.request("shutdown", Value::Null).await;
        self.notify("exit", Value::Null).await;
        self.stdin.shutdown().await.unwrap();
        // ChildStdin::shutdown does not release the owned pipe handle. The
        // stdio server also needs EOF to finish its transport read loop.
        drop(self.stdin);
        assert!(timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("LSP shutdown deadline")
            .unwrap()
            .success());
    }
}

#[tokio::test]
async fn native_server_initializes_and_shuts_down_cleanly() {
    LspProcess::start().await.stop().await;
}

#[tokio::test]
async fn native_arithmetic_and_try_cast_keep_computed_candidate_and_hover_types() {
    let mut process = LspProcess::start().await;
    for (dialect, expression, expected) in [
        (
            "postgres",
            "COALESCE(SUM(-(CAST(2 AS bigint) * 3)), 0)",
            "numeric",
        ),
        ("mysql", "SUM(2 * 3)", "decimal"),
        (
            "sqlserver",
            "TRY_CAST('bad' AS decimal(18,4)) + 2",
            "decimal",
        ),
    ] {
        let uri = format!("oxide://query/computed.{dialect}.sql");
        process
            .notify(
                "workspace/didChangeConfiguration",
                json!({"settings":{"fileDialects":{(&uri):dialect}}}),
            )
            .await;
        let prefix = format!("/* 🧭 */ WITH calc(result) AS (SELECT {expression}) SELECT c.re");
        process
            .open(&uri, &format!("{prefix}sult FROM calc c"))
            .await;
        let params = json!({"textDocument":{"uri":uri},"position":{"line":0,"character":prefix.encode_utf16().count()}});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let items = response
            .as_array()
            .or_else(|| response.get("items").and_then(Value::as_array))
            .expect("completion list");
        let item = items
            .iter()
            .find(|item| item["label"] == "result")
            .expect("native computed field");
        assert!(
            item["detail"].as_str().unwrap().contains(expected),
            "{dialect}: {item}"
        );
        let hover = process.request("textDocument/hover", params).await;
        assert!(!hover.is_null());
        assert!(hover.to_string().contains(expected), "{dialect}: {hover}");
    }
    process.stop().await;
}

#[tokio::test]
async fn native_positional_cte_set_and_postfix_casts_keep_candidate_and_hover_types() {
    let mut process = LspProcess::start().await;
    for (index, body, expected) in [
        (
            0,
            "SELECT 1::integer, COUNT(*) UNION ALL SELECT 2::bigint, 3::numeric",
            "numeric",
        ),
        (1, "SELECT 1, NULL::integer[]", "integer[]"),
    ] {
        let uri = format!("oxide://query/positional{index}.mysql.sql");
        process
            .notify(
                "workspace/didChangeConfiguration",
                json!({"settings":{"fileDialects":{(&uri):"postgres"}}}),
            )
            .await;
        let prefix = format!("/* 🧭 */ WITH merged(ident,result) AS ({body}) SELECT m.re");
        process
            .open(&uri, &format!("{prefix}sult FROM merged m"))
            .await;
        let params = json!({"textDocument":{"uri":uri},"position":{"line":0,"character":prefix.encode_utf16().count()}});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let items = response
            .as_array()
            .or_else(|| response.get("items").and_then(Value::as_array))
            .expect("completion list");
        let item = items
            .iter()
            .find(|item| item["label"] == "result")
            .expect("native positional candidate");
        assert!(
            item["detail"].as_str().unwrap().contains(expected),
            "{item}"
        );
        let hover = process.request("textDocument/hover", params).await;
        assert!(!hover.is_null());
        assert!(hover.to_string().contains(expected), "{hover}");
    }
    process.stop().await;
}

#[tokio::test]
async fn native_complex_expression_completion_and_hover_use_configured_dialect() {
    let mut process = LspProcess::start().await;
    // Both the suffix and didOpen language disagree with the explicit override.
    let uri = "oxide://query/expression.mysql.sql";
    process
        .notify(
            "workspace/didChangeConfiguration",
            json!({"settings":{
                "fileDialects":{(uri):"postgres"}
            }}),
        )
        .await;
    let prefix = "/* 🔎 */ WITH totals AS (SELECT COALESCE(SUM(CAST(1 AS integer)), 0) AS result) SELECT t.re";
    process
        .open(uri, &format!("{prefix}sult FROM totals t"))
        .await;
    let params = json!({"textDocument":{"uri":uri},"position":{"line":0,"character":prefix.encode_utf16().count()}});
    let response = process
        .request("textDocument/completion", params.clone())
        .await;
    let items = response
        .as_array()
        .or_else(|| response.get("items").and_then(Value::as_array))
        .expect("completion list");
    let result = items
        .iter()
        .find(|item| item["label"] == "result")
        .expect("native expression candidate");
    assert!(
        result["detail"].as_str().unwrap().contains("bigint"),
        "{result}"
    );
    let hover = process.request("textDocument/hover", params).await;
    assert!(!hover.is_null());
    assert!(hover.to_string().contains("bigint"), "{hover}");
    process.stop().await;
}

#[tokio::test]
async fn native_completion_keeps_json_table_shape_across_cte() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://query/complex.mysql.sql";
    let schema_id = "33333333-3333-4333-8333-333333333333";
    process.notify("workspace/didChangeConfiguration", json!({"settings":{
        "schemas":[{"id":schema_id,"database":"app","tables":[],"functions":[],"source_uri":null}],
        "fileSchemas":{(uri):schema_id},"fileDialects":{(uri):"mysql"}
    }})).await;
    let prefix = "WITH expanded AS (SELECT j.* FROM JSON_TABLE('[]','$[*]' COLUMNS(city VARCHAR(30) PATH '$.city',amount DECIMAL(10,2) PATH '$.amount')) j) SELECT e.";
    process
        .open(uri, &format!("{prefix} FROM expanded e"))
        .await;
    let items = process.request("textDocument/completion", json!({"textDocument":{"uri":uri},"position":{"line":0,"character":prefix.encode_utf16().count()}})).await;
    let items = items
        .as_array()
        .or_else(|| items.get("items").and_then(Value::as_array))
        .expect("completion list");
    for (name, ty) in [("city", "varchar(30)"), ("amount", "decimal(10,2)")] {
        let item = items
            .iter()
            .find(|item| item["label"] == name)
            .expect("native declared field");
        assert_eq!(item["kind"], 5);
        assert!(item["detail"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains(ty));
    }
    process.stop().await;
}

#[tokio::test]
async fn native_schema_refresh_keeps_same_sql_and_replaces_candidates() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://query/refresh.mysql.sql";
    let schema_id = "33333333-3333-4333-8333-333333333333";
    let settings = |name: &str| {
        json!({"settings":{
            "schemas":[{"id":schema_id,"database":"app","tables":[{"name":"users","columns":[{"name":name,"data_type":"text","nullable":true,"primary_key":false,"unique":false,"indexed":false}]}],"functions":[],"source_uri":null}],
            "fileSchemas":{(uri):schema_id},"fileDialects":{(uri):"mysql"}
        }})
    };
    process
        .notify(
            "workspace/didChangeConfiguration",
            settings("original_name"),
        )
        .await;
    process.open(uri, "SELECT u. FROM users u").await;
    let params = json!({"textDocument":{"uri":uri},"position":{"line":0,"character":9}});
    let first = process
        .request("textDocument/completion", params.clone())
        .await;
    assert!(first
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["label"] == "original_name"));
    process
        .notify(
            "workspace/didChangeConfiguration",
            settings("refreshed_name"),
        )
        .await;
    process
        .until(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .await;
    let next = process.request("textDocument/completion", params).await;
    let items = next.as_array().unwrap();
    assert!(items.iter().any(|item| item["label"] == "refreshed_name"));
    assert!(!items.iter().any(|item| item["label"] == "original_name"));
    process.stop().await;
}

#[tokio::test]
async fn native_search_completion_hover_and_signatures_keep_semantics() {
    let mut process = LspProcess::start().await;
    for (index, dialect, expression, expected) in [
        (
            0,
            "postgres",
            "ts_rank(to_tsvector('oxide'),websearch_to_tsquery('studio OR sql'))",
            "real",
        ),
        (
            1,
            "postgres",
            "to_tsquery('oxide')&&phraseto_tsquery('studio sql')",
            "tsquery",
        ),
        (
            2,
            "mysql",
            "MATCH(u.name) AGAINST ('+oxide -legacy' IN BOOLEAN MODE)",
            "double",
        ),
    ] {
        let uri = format!("oxide://query/search{index}.{dialect}.sql");
        process
            .notify(
                "workspace/didChangeConfiguration",
                json!({"settings":{"fileDialects":{(&uri):dialect}}}),
            )
            .await;
        let prefix = format!(
            "/* 🧭 */ WITH searched(result) AS (SELECT {expression} FROM users u) SELECT s.re"
        );
        process
            .open(&uri, &format!("{prefix}sult FROM searched s"))
            .await;
        let params = json!({"textDocument":{"uri":uri},"position":{"line":0,"character":prefix.encode_utf16().count()}});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let items = response
            .as_array()
            .or_else(|| response.get("items").and_then(Value::as_array))
            .unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == "result")
            .expect("native search result");
        assert!(
            item["detail"].as_str().unwrap().contains(expected),
            "{dialect}: {item}"
        );
        let hover = process.request("textDocument/hover", params).await;
        assert!(hover.to_string().contains(expected), "{hover}");
    }
    let uri = "oxide://query/search.signature.pg.sql";
    process
        .notify(
            "workspace/didChangeConfiguration",
            json!({"settings":{"fileDialects":{uri:"postgres"}}}),
        )
        .await;
    let prefix = "SELECT ts_rank(to_tsvector('oxide'),websearch_to_tsquery('english',";
    process.open(uri, &format!("{prefix}''))")).await;
    let result=process.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":{"line":0,"character":prefix.encode_utf16().count()}})).await;
    assert_eq!(result["activeParameter"], 1);
    assert!(result["signatures"]
        .to_string()
        .to_ascii_lowercase()
        .contains("websearch_to_tsquery"));
    process.stop().await;
}

#[tokio::test]
async fn native_vendor_fulltext_rowset_and_oracle_score_keep_types_and_diagnostics() {
    let mut process = LspProcess::start().await;
    for (index,dialect,prefix,suffix,expected) in [
        (0,"sqlserver","/* 🧭 */ WITH hits AS (SELECT ft.* FROM CONTAINSTABLE(users,(name,email),N'ISABOUT(oxide WEIGHT(.8),studio WEIGHT(.2))',LANGUAGE 1033,25) ft(document_key,relevance)) SELECT h.re","levance FROM hits h ORDER BY h.relevance DESC","int"),
        (1,"oracle","/* 🧭 */ SELECT r.re","levance FROM (SELECT SCORE(7) AS relevance FROM users u WHERE CONTAINS(u.name,'NEAR((oxide,studio),5)',7)>0) r ORDER BY r.relevance DESC","number"),
    ] {
        let uri=format!("oxide://query/vendor_search{index}.mysql.sql");
        process.notify("workspace/didChangeConfiguration",json!({"settings":{"fileDialects":{(&uri):dialect}}})).await;
        process.open(&uri,&format!("{prefix}{suffix}")).await;
        let params=json!({"textDocument":{"uri":uri},"position":{"line":0,"character":prefix.encode_utf16().count()}});
        let response=process.request("textDocument/completion",params.clone()).await;
        let items=response.as_array().or_else(||response.get("items").and_then(Value::as_array)).unwrap();
        let item=items.iter().find(|item|item["label"]=="relevance").expect("native vendor search candidate");
        assert!(item["detail"].as_str().unwrap().contains(expected),"{dialect}: {item}");
        let hover=process.request("textDocument/hover",params).await;
        assert!(hover.to_string().contains(expected),"{dialect}: {hover}");
        let diagnostics=process.request("textDocument/diagnostic",json!({"textDocument":{"uri":uri}})).await;
        assert_eq!(diagnostics["items"].as_array().expect("pull diagnostic items").len(),0,"{dialect}: {diagnostics}");
    }
    process.stop().await;
}

#[tokio::test]
async fn native_recursive_anchor_types_reach_members_and_outer_projections() {
    let mut process = LspProcess::start().await;
    for (dialect, keyword, anchor, expected) in [
        ("postgres", "RECURSIVE", "CAST(1 AS integer)", "integer"),
        ("mysql", "RECURSIVE", "CAST(1 AS SIGNED)", "SIGNED"),
        ("sqlserver", "", "CAST(1 AS int)", "int"),
    ] {
        for member in [false, true] {
            let uri = format!("oxide://query/recursive.{dialect}.{member}.mysql.sql");
            process
                .notify(
                    "workspace/didChangeConfiguration",
                    json!({"settings":{"fileDialects":{(&uri):dialect}}}),
                )
                .await;
            let sql=format!("/* 🧭 */ WITH {keyword} walk(value) AS (SELECT {anchor} UNION ALL SELECT w.value+1 FROM walk w WHERE w.value<25), projected AS (SELECT w.* FROM walk w) SELECT r.value FROM (SELECT * FROM projected) r");
            let byte = if member {
                sql.find("w.value+1").unwrap() + 4
            } else {
                sql.rfind("r.value").unwrap() + 4
            };
            process.open(&uri, &sql).await;
            let params = json!({"textDocument":{"uri":uri},"position":{"line":0,"character":sql[..byte].encode_utf16().count()}});
            let response = process
                .request("textDocument/completion", params.clone())
                .await;
            let items = response
                .as_array()
                .or_else(|| response.get("items").and_then(Value::as_array))
                .unwrap();
            let item = items
                .iter()
                .find(|item| item["label"] == "value")
                .expect("native recursive column");
            assert!(
                item["detail"].as_str().unwrap().contains(expected),
                "{dialect} {member}: {item}"
            );
            let hover = process.request("textDocument/hover", params).await;
            assert!(
                hover.to_string().contains(expected),
                "{dialect} {member}: {hover}"
            );
            let diagnostics = process
                .request(
                    "textDocument/diagnostic",
                    json!({"textDocument":{"uri":uri}}),
                )
                .await;
            assert!(
                diagnostics["items"].as_array().unwrap().is_empty(),
                "{dialect}: {diagnostics}"
            );
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_namespaces_keep_catalog_and_cte_types_distinct() {
    let mut process = LspProcess::start().await;
    let schema_id = "44444444-4444-4444-8444-444444444444";
    for dialect in ["postgres", "mysql", "sqlite", "hive", "clickhouse"] {
        for (alias, expected) in [("p", "text"), ("c", "bigint")] {
            let uri = format!("oxide://query/namespace.{dialect}.{alias}.mysql.sql");
            process.notify("workspace/didChangeConfiguration",json!({"settings":{
                "schemas":[{"id":schema_id,"database":"app","tables":[{"name":"users","object_type":"BASE TABLE","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":true}]}],"functions":[],"source_uri":null}],
                "fileSchemas":{(&uri):schema_id},"fileDialects":{(&uri):dialect}
            }})).await;
            let sql=format!("/* 🧭 */{}WITH users AS (SELECT id AS name FROM app.users) SELECT {alias}.name FROM app.users p JOIN users c ON p.id=c.name", if alias=="p" {" "} else {"\r\n"});
            let byte = sql.find(&format!("SELECT {alias}.na")).unwrap()
                + format!("SELECT {alias}.na").len();
            process.open(&uri, &sql).await;
            let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..byte])});
            let response = process
                .request("textDocument/completion", params.clone())
                .await;
            let items = response
                .as_array()
                .or_else(|| response.get("items").and_then(Value::as_array))
                .unwrap();
            let item = items
                .iter()
                .find(|item| item["label"] == "name")
                .expect("native namespace column");
            assert!(
                item["detail"].as_str().unwrap().contains(expected),
                "{dialect} {alias}: {item}"
            );
            let hover = process.request("textDocument/hover", params).await;
            assert!(
                hover.to_string().contains(expected),
                "{dialect} {alias}: {hover}"
            );
            assert_eq!(
                hover["range"]["start"]["line"],
                if alias == "p" { 0 } else { 1 }
            );
            let diagnostics = process
                .request(
                    "textDocument/diagnostic",
                    json!({"textDocument":{"uri":uri}}),
                )
                .await;
            assert!(
                diagnostics["items"].as_array().unwrap().is_empty(),
                "{dialect}: {diagnostics}"
            );
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_visibility_and_nested_with_keep_scoped_semantics() {
    let mut process = LspProcess::start().await;
    let schema_id = "55555555-5555-4555-8555-555555555555";
    for dialect in ["postgres", "mysql", "sqlite", "hive", "clickhouse"] {
        for (index,(marked,expected)) in [
            ("SELECT i<cursor>d FROM orders",Some(("id","orders.id","bigint"))),
            ("SELECT audit_<cursor>secret FROM users",None),
            ("SELECT i<cursor>d FROM users u JOIN users v ON u.id=v.id",None),
            ("SELECT us<cursor>ers FROM (SELECT id AS users FROM app.users) d",Some(("users","d.users","bigint"))),
            ("WITH c AS (SELECT id AS outer_key FROM app.users) SELECT x.inner_<cursor>name FROM (WITH c AS (SELECT name AS inner_name FROM app.users) SELECT c.* FROM c) x",Some(("inner_name","x.inner_name","text"))),
        ].into_iter().enumerate() {
            let uri=format!("oxide://query/visibility.{dialect}.{index}.mysql.sql");
            process.notify("workspace/didChangeConfiguration",json!({"settings":{
                "schemas":[{"id":schema_id,"database":"app","tables":[
                    {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":true}]},
                    {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"total","data_type":"decimal","nullable":true}]},
                    {"name":"audit_events","columns":[{"name":"audit_secret","data_type":"text","nullable":true}]}
                ],"functions":[]}],
                "fileSchemas":{(&uri):schema_id},"fileDialects":{(&uri):dialect}
            }})).await;
            let marked=format!("/* 🧭 */{}{marked}",if index%2==0 {"\r\n"}else{" "});
            let byte=marked.find("<cursor>").unwrap();
            let sql=marked.replace("<cursor>","");
            process.open(&uri,&sql).await;
            let params=json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..byte])});
            let completion=process.request("textDocument/completion",params.clone()).await;
            let hover=process.request("textDocument/hover",params).await;
            if let Some((name,origin,ty))=expected {
                let items=completion.as_array().or_else(||completion.get("items").and_then(Value::as_array)).unwrap();
                let item=items.iter().find(|item|item["label"]==name).unwrap_or_else(||panic!("{dialect}: {marked}: {completion}"));
                assert!(item["detail"].as_str().is_some_and(|detail|detail.contains(ty)),"{dialect}: {item}");
                let text=hover.to_string();
                assert!(text.contains("Column") && text.contains(origin) && text.contains(ty),"{dialect}: {marked}: {hover}");
                assert_eq!(hover["range"]["start"]["line"],if index%2==0 {1}else{0});
            } else {
                assert!(hover.is_null(),"{dialect}: {marked}: {hover}");
                if index==2 {
                    let text=completion.to_string();
                    assert!(text.contains("u.id") && text.contains("v.id"),"{dialect}: {completion}");
                }
            }
            let diagnostics=process.request("textDocument/diagnostic",json!({"textDocument":{"uri":uri}})).await;
            let items=diagnostics["items"].as_array().unwrap();
            if index==2 {
                assert_eq!(items.len(),1,"{dialect}: {marked}: {diagnostics}");
                assert_eq!(items[0]["code"],"OXIDE004");assert_eq!(items[0]["severity"],2);
            } else {assert!(items.is_empty(),"{dialect}: {marked}: {diagnostics}");}
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_correlation_respects_scope_edges_and_outer_field_types() {
    let mut process = LspProcess::start().await;
    let schema_id = "66666666-6666-4666-8666-666666666666";
    let mut cases = Vec::new();
    for dialect in ["postgres", "mysql", "sqlite", "hive", "clickhouse"] {
        for (sql,name,expected) in [
            ("SELECT * FROM orders o WHERE EXISTS(SELECT to<cursor>tal FROM users u)","total",Some(("orders.total","decimal"))),
            ("SELECT * FROM orders o WHERE EXISTS(SELECT to<cursor>tal FROM (SELECT name AS total FROM users) d)","total",Some(("d.total","text"))),
            ("SELECT users.na<cursor>me FROM orders o","name",None),
            ("SELECT * FROM orders o JOIN (SELECT o.to<cursor>tal) d ON TRUE","total",None),
            ("WITH c AS (SELECT o.to<cursor>tal) SELECT * FROM orders o","total",None),
            ("SELECT * FROM orders o WHERE EXISTS(SELECT 1 FROM users u WHERE EXISTS(SELECT to<cursor>tal))","total",Some(("orders.total","decimal"))),
        ] {cases.push((dialect,sql,name,expected));}
    }
    for dialect in ["postgres", "mysql"] {
        cases.push((
            dialect,
            "SELECT * FROM orders o CROSS JOIN LATERAL(SELECT to<cursor>tal) d",
            "total",
            Some(("orders.total", "decimal")),
        ));
        cases.push((dialect,"SELECT * FROM users u CROSS JOIN LATERAL(SELECT o.to<cursor>tal) d CROSS JOIN orders o","total",None));
    }
    for sql in [
        "SELECT * FROM orders o CROSS APPLY(SELECT o.to<cursor>tal) d",
        "SELECT * FROM orders o OUTER APPLY(SELECT to<cursor>tal) d",
    ] {
        cases.push(("sqlserver", sql, "total", Some(("orders.total", "decimal"))));
    }
    assert_eq!(cases.len(), 36);
    for (index, (dialect, marked, name, expected)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://query/correlation.{index}.mysql.sql");
        process.notify("workspace/didChangeConfiguration",json!({"settings":{
            "schemas":[{"id":schema_id,"database":"app","tables":[
                {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":true}]},
                {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"total","data_type":"decimal","nullable":true}]}
            ],"functions":[]}],"fileSchemas":{(&uri):schema_id},"fileDialects":{(&uri):dialect}
        }})).await;
        let marked = format!(
            "/* 🧭 */{}{marked}",
            if index % 2 == 0 { "\r\n" } else { " " }
        );
        let byte = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..byte])});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let items = response
            .as_array()
            .or_else(|| response.get("items").and_then(Value::as_array))
            .unwrap();
        let hover = process.request("textDocument/hover", params).await;
        if let Some((origin, ty)) = expected {
            assert!(
                items.iter().any(|item| (item["label"] == name
                    || item["label"]
                        .as_str()
                        .is_some_and(|label| label.ends_with(&format!(".{name}"))))
                    && item["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.contains(ty))),
                "{dialect}: {marked}: {response}"
            );
            let text = hover.to_string();
            assert!(
                text.contains(origin) && text.contains(ty),
                "{dialect}: {marked}: {hover}"
            );
            assert_eq!(
                hover["range"]["start"]["line"],
                if index % 2 == 0 { 1 } else { 0 }
            );
            let diagnostics = process
                .request(
                    "textDocument/diagnostic",
                    json!({"textDocument":{"uri":uri}}),
                )
                .await;
            // SELECT * may emit OXIDE003 hints; valid correlation must have
            // neither parser errors nor error-severity semantic diagnostics.
            assert!(
                diagnostics["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|item| item["severity"] != 1 && item["code"] != "PARSE_ERROR"),
                "{dialect}: {marked}: {diagnostics}"
            );
        } else {
            assert!(hover.is_null(), "{dialect}: {marked}: {hover}");
            assert!(
                !items.iter().any(|item| item["label"] == name),
                "{dialect}: {marked}: {response}"
            );
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_correlated_outputs_propagate_types_through_lateral_apply_and_ctes() {
    let mut process = LspProcess::start().await;
    let schema_id = "77777777-7777-4777-8777-777777777777";
    let cases=[
        ("postgres","SELECT d.am<cursor>ount FROM orders o CROSS JOIN LATERAL(SELECT o.total AS amount) d","amount","decimal",None),
        ("postgres","SELECT b.am<cursor>ount FROM orders o CROSS JOIN LATERAL(SELECT o.total AS first_value) a CROSS JOIN LATERAL(SELECT a.first_value AS amount) b","amount","decimal",None),
        ("postgres","WITH result AS (SELECT d.* FROM orders o CROSS JOIN LATERAL(SELECT o.*) d) SELECT r.to<cursor>tal FROM result r","total","decimal",None),
        ("postgres","WITH amounts AS (SELECT o.id,d.amount FROM orders o CROSS JOIN LATERAL(SELECT COALESCE(SUM(i.total),0) AS amount FROM orders i WHERE i.customer_id=o.customer_id) d), ranked AS (SELECT id,amount,ROW_NUMBER() OVER(ORDER BY amount DESC) AS pos FROM amounts) SELECT r.am<cursor>ount FROM ranked r","amount","numeric",None),
        ("postgres","SELECT d.am<cursor>ount FROM orders o CROSS JOIN(SELECT o.total AS amount) d","amount","unknown",None),
        ("postgres","SELECT d.am<cursor>ount FROM users u CROSS JOIN LATERAL(SELECT o.total AS amount) d CROSS JOIN orders o","amount","unknown",None),
        ("mysql","SELECT d.am<cursor>ount FROM orders o CROSS JOIN LATERAL(SELECT total AS amount) d","amount","decimal",None),
        ("mysql","SELECT b.output_<cursor>key FROM orders o CROSS JOIN LATERAL(SELECT o.id AS first_key) a CROSS JOIN LATERAL(SELECT a.first_key AS output_key) b","output_key","bigint",None),
        ("mysql","WITH result(amount,output_key) AS (SELECT d.* FROM orders o CROSS JOIN LATERAL(SELECT o.total,o.id) d) SELECT r.am<cursor>ount FROM result r","amount","decimal",None),
        ("sqlserver","SELECT d.am<cursor>ount FROM orders o CROSS APPLY(SELECT o.total AS amount) d","amount","decimal",None),
        ("sqlserver","SELECT d.am<cursor>ount FROM orders o OUTER APPLY(SELECT o.total AS amount) d","amount","decimal",None),
        ("sqlserver","SELECT b.output_<cursor>key FROM orders o CROSS APPLY(SELECT o.id AS first_key) a OUTER APPLY(SELECT a.first_key AS output_key) b","output_key","bigint",None),
        ("sqlserver","WITH result(amount,output_key) AS (SELECT d.* FROM orders o CROSS APPLY(SELECT o.total,o.id) d) SELECT r.am<cursor>ount FROM result r","amount","decimal",None),
        ("sqlserver","SELECT d.va<cursor>lue FROM orders o OUTER APPLY(SELECT 1 AS value) d","value","int",Some(true)),
    ];
    assert_eq!(cases.len(), 14);
    for (index, (dialect, marked, name, ty, nullable)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://query/correlated-output.{index}.mysql.sql");
        process.notify("workspace/didChangeConfiguration",json!({"settings":{
            "schemas":[{"id":schema_id,"database":"app","tables":[
                {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":true}]},
                {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"total","data_type":"decimal","nullable":false},{"name":"customer_id","data_type":"bigint","nullable":false}]}
            ],"functions":[]}],"fileSchemas":{(&uri):schema_id},"fileDialects":{(&uri):dialect}
        }})).await;
        let marked = format!(
            "/* 🧭 */{}{marked}",
            if index % 2 == 0 { "\r\n" } else { " " }
        );
        let byte = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..byte])});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let items = response
            .as_array()
            .or_else(|| response.get("items").and_then(Value::as_array))
            .unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == name)
            .unwrap_or_else(|| panic!("{dialect}: {marked}: {response}"));
        assert!(
            item["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(&format!("({ty})"))),
            "{dialect}: {marked}: {item}"
        );
        let hover = process.request("textDocument/hover", params).await;
        assert!(
            hover.to_string().contains(ty),
            "{dialect}: {marked}: {hover}"
        );
        assert_eq!(
            hover["range"]["start"]["line"],
            if index % 2 == 0 { 1 } else { 0 }
        );
        if let Some(nullable) = nullable {
            assert!(
                item.to_string().contains(if nullable {
                    "Nullable: yes"
                } else {
                    "Nullable: no"
                }),
                "{item}"
            );
        }
        if ty != "unknown" {
            let diagnostics = process
                .request(
                    "textDocument/diagnostic",
                    json!({"textDocument":{"uri":uri}}),
                )
                .await;
            assert!(
                diagnostics["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|item| item["severity"] != 1 && item["code"] != "PARSE_ERROR"),
                "{dialect}: {marked}: {diagnostics}"
            );
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_quoted_identifier_completion_edits_and_types() {
    let mut process = LspProcess::start().await;
    let schema_id = "88888888-8888-4888-8888-888888888888";
    let cases = [
        (
            "postgres",
            r#"SELECT u."display.na<cursor>me" FROM users u"#,
            "display.name",
            r#"SELECT u."display.name" FROM users u"#,
            true,
        ),
        (
            "postgres",
            r#"SELECT "o.x".na<cursor>me FROM users AS "o.x""#,
            "name",
            r#"SELECT "o.x".name FROM users AS "o.x""#,
            true,
        ),
        (
            "postgres",
            r#"SELECT u."quote""na<cursor>me" FROM users u"#,
            "quote\"name",
            r#"SELECT u."quote""name" FROM users u"#,
            true,
        ),
        (
            "mysql",
            "SELECT u.`display.na<cursor>me` FROM users u",
            "display.name",
            "SELECT u.`display.name` FROM users u",
            true,
        ),
        (
            "mysql",
            "SELECT u.`tick``na<cursor>me` FROM users u",
            "tick`name",
            "SELECT u.`tick``name` FROM users u",
            true,
        ),
        (
            "mysql",
            "SELECT `o.x`.na<cursor>me FROM users AS `o.x`",
            "name",
            "SELECT `o.x`.name FROM users AS `o.x`",
            true,
        ),
        (
            "sqlserver",
            "SELECT [o.x].na<cursor>me FROM users AS [o.x]",
            "name",
            "SELECT [o.x].name FROM users AS [o.x]",
            true,
        ),
        ("sqlserver", "SELECT [u].[na<cursor>me] FROM [users] [u]", "name", "SELECT [u].name FROM [users] [u]", true),
        ("sqlserver", "SELECT [o.x].[display]]na<cursor>me] FROM [users] [o.x]", "display]name", "SELECT [o.x].[display]]name] FROM [users] [o.x]", true),
        ("sqlserver", "WITH [c.x] AS (SELECT [u].[name] AS [result] FROM [users] [u]) SELECT [c.x].[re<cursor>sult] FROM [c.x]", "result", "WITH [c.x] AS (SELECT [u].[name] AS [result] FROM [users] [u]) SELECT [c.x].result FROM [c.x]", true),
        ("sqlserver", "SELECT [d.x].[re<cursor>sult] FROM (SELECT [u].[name] AS [result] FROM [users] [u]) [d.x]", "result", "SELECT [d.x].result FROM (SELECT [u].[name] AS [result] FROM [users] [u]) [d.x]", true),
        ("sqlserver", "SELECT [d.x].[re<cursor>sult] FROM [users] [u] CROSS APPLY(SELECT [u].[name] AS [result]) [d.x]", "result", "SELECT [d.x].result FROM [users] [u] CROSS APPLY(SELECT [u].[name] AS [result]) [d.x]", true),
        ("sqlserver", "SELECT [order].[na<cursor>me] FROM [users] AS [order]", "name", "SELECT [order].name FROM [users] AS [order]", true),
    ];
    assert_eq!(cases.len(), 13);
    for (index, (dialect, marked, name, expected, syntax_and_hover)) in
        cases.into_iter().enumerate()
    {
        let uri = format!("oxide://query/quoted-names.{index}.mysql.sql");
        process.notify("workspace/didChangeConfiguration", json!({"settings":{
            "schemas":[{"id":schema_id,"database":"app","tables":[{"name":"users","columns":
                (["name","display.name","quote\"name","tick`name","display]name"].map(|name|json!({"name":name,"data_type":"text","nullable":true})))
            }],"functions":[]}],"fileSchemas":{(&uri):schema_id},"fileDialects":{(&uri):dialect}
        }})).await;
        let prefix = "/* 😀 */\r\n";
        let marked = format!("{prefix}{marked}");
        let offset = marked.find("<cursor>").unwrap();
        let mut sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let items = response
            .as_array()
            .or_else(|| response["items"].as_array())
            .unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == name)
            .unwrap_or_else(|| panic!("{dialect}: {marked}: {response}"));
        assert!(item["detail"].as_str().unwrap().contains("text"), "{item}");
        let edit = &item["textEdit"];
        // All edits occur on line 1 after a CRLF and an emoji. SQL on that line
        // is ASCII, so its UTF-16 member positions map directly to byte offsets.
        assert_eq!(edit["range"]["start"]["line"], 1);
        assert_eq!(edit["range"]["end"]["line"], 1);
        let start = prefix.len() + edit["range"]["start"]["character"].as_u64().unwrap() as usize;
        let end = prefix.len() + edit["range"]["end"]["character"].as_u64().unwrap() as usize;
        sql.replace_range(start..end, edit["newText"].as_str().unwrap());
        assert_eq!(sql, format!("{prefix}{expected}"), "{dialect}: {item}");
        if syntax_and_hover {
            let hover = process.request("textDocument/hover", params).await;
            assert!(
                hover.to_string().contains("Column") && hover.to_string().contains("text"),
                "{dialect}: {hover}"
            );
            let diagnostics = process
                .request(
                    "textDocument/diagnostic",
                    json!({"textDocument":{"uri":uri}}),
                )
                .await;
            assert!(
                diagnostics["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|item| item["severity"] != 1 && item["code"] != "PARSE_ERROR"),
                "{dialect}: {marked}: {diagnostics}"
            );
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_same_text_profile_switch_replaces_bracket_analysis() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://query/profile-switch.mysql.sql";
    let id = "99999999-9999-4999-8999-999999999999";
    let text = "/* 😀 */ SELECT [u].[name] FROM [users] [u]";
    let cursor = text.find("name").unwrap() + 2;
    for (index, dialect) in ["postgres", "sqlserver", "postgres", "tsql"]
        .into_iter()
        .cycle()
        .take(24)
        .enumerate()
    {
        process.notify("workspace/didChangeConfiguration",json!({"settings":{
            "schemas":[{"id":id,"database":"app","tables":[{"name":"users","columns":[{"name":"name","data_type":"text","nullable":true}]}],"functions":[]}],
            "fileSchemas":{(uri):id},"fileDialects":{(uri):dialect}
        }})).await;
        if index == 0 {
            process.open(uri, text).await;
        }
        let diagnostics = process
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        let error = diagnostics["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["severity"] == 1 || item["code"] == "PARSE_ERROR");
        assert_eq!(error, dialect == "postgres", "{dialect}: {diagnostics}");
        if dialect != "postgres" {
            let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&text[..cursor])});
            let response = process
                .request("textDocument/completion", params.clone())
                .await;
            let items = response
                .as_array()
                .or_else(|| response["items"].as_array())
                .unwrap();
            assert!(
                items.iter().any(|item| item["label"] == "name"
                    && item["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.contains("text"))),
                "{response}"
            );
            let hover = process.request("textDocument/hover", params).await;
            assert!(
                hover.to_string().contains("SQL Server Column")
                    && hover.to_string().contains("text"),
                "{hover}"
            );
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_scalar_output_types_follow_scope_and_shared_inference() {
    let mut process = LspProcess::start().await;
    let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let cases=[
        ("postgres","SELECT (SELECT u.name) AS value FROM users u","text"),
        ("postgres","SELECT (SELECT (SELECT u.name)) AS value FROM users u","text"),
        ("postgres","SELECT COALESCE((SELECT u.name), 'fallback') AS value FROM users u","text"),
        ("postgres","SELECT CASE WHEN u.id>0 THEN (SELECT u.id) ELSE '1' END AS value FROM users u","bigint"),
        ("postgres","SELECT (SELECT u.id WHERE FALSE UNION ALL SELECT u.id) AS value FROM users u","bigint"),
        ("postgres","SELECT (SELECT u.total FROM orders u LIMIT 1) AS value FROM users u","decimal"),
        ("postgres","SELECT (WITH inner_value AS (SELECT id FROM users) SELECT id FROM inner_value LIMIT 1) AS value","bigint"),
        ("mysql","SELECT (SELECT u.name) AS value FROM users u","text"),
        ("mysql","SELECT (SELECT COUNT(*) FROM orders o WHERE o.customer_id=u.id) AS value FROM users u","bigint"),
        ("sqlite","SELECT (SELECT u.name) AS value FROM users u","text"),
        ("sqlserver","SELECT (SELECT [u].[name]) AS [value] FROM [users] [u]","text"),
        ("sqlserver","SELECT (SELECT COUNT(*) FROM orders o WHERE o.customer_id=u.id) AS value FROM users u","int"),
        ("postgres","SELECT (SELECT id,name FROM users) AS value","unknown"),
        ("postgres","SELECT (SELECT u.name FROM missing_table u) AS value FROM users u","unknown"),
        ("postgres","SELECT d.value FROM users u CROSS JOIN(SELECT (SELECT u.name) AS value) d","unknown"),
        ("postgres","SELECT (SELECT id FROM users u JOIN orders o ON u.id=o.customer_id) AS value","unknown"),
    ];
    assert_eq!(cases.len(), 16);
    for (index, (dialect, body, ty)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://query/scalar-output.{index}.mysql.sql");
        process.notify("workspace/didChangeConfiguration",json!({"settings":{
            "schemas":[{"id":id,"database":"app","tables":[
                {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":false}]},
                {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"total","data_type":"decimal","nullable":false},{"name":"customer_id","data_type":"bigint","nullable":false}]}
            ],"functions":[]}],"fileSchemas":{(&uri):id},"fileDialects":{(&uri):dialect}
        }})).await;
        let marked = format!(
            "/* 😀 */{}WITH result AS ({body}) SELECT r.va<cursor>lue FROM result r",
            if index % 2 == 0 { "\r\n" } else { " " }
        );
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])});
        let response = process
            .request("textDocument/completion", params.clone())
            .await;
        let items = response
            .as_array()
            .or_else(|| response["items"].as_array())
            .unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == "value")
            .unwrap_or_else(|| panic!("{dialect}: {sql}: {response}"));
        assert!(
            item["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(&format!("({ty})"))),
            "{dialect}: {sql}: {item}"
        );
        assert!(!item.to_string().contains("__oxide_untyped_string__"));
        let hover = process.request("textDocument/hover", params).await;
        assert!(hover.to_string().contains(ty), "{dialect}: {sql}: {hover}");
        assert_eq!(
            hover["range"]["start"]["line"],
            if index % 2 == 0 { 1 } else { 0 }
        );
        if ty != "unknown" {
            let diagnostics = process
                .request(
                    "textDocument/diagnostic",
                    json!({"textDocument":{"uri":uri}}),
                )
                .await;
            assert!(
                diagnostics["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|item| item["severity"] != 1 && item["code"] != "PARSE_ERROR"),
                "{dialect}: {sql}: {diagnostics}"
            );
        }
    }
    process.stop().await;
}
