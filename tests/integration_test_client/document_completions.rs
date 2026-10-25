use super::*;
const URI: &str = "oxide://local/completion.sql";
fn params(text: &str, dialect: &str) -> Value {
    let at = text.find('|').unwrap();
    let prefix = &text[..at];
    json!({"textDocument":{"uri":URI},"version":1,"dialect":dialect,"position":{"line":prefix.bytes().filter(|b|*b==b'\n').count(),"character":prefix.rsplit('\n').next().unwrap().encode_utf16().count()},"preferences":{"keywordCase":"lower","functionCase":"lower","tableAlias":"none"}})
}
#[tokio::test]
async fn local_completion_wire_preserves_suffix_types_and_has_no_catalog_resolve() {
    let mut p = LspProcess::start().await;
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":"private","defaultDialect":"postgres","fileSchemas":{(URI):"00000000-0000-4000-8000-000000000001"},"schemas":[{"id":"00000000-0000-4000-8000-000000000001","database":"private","functions":[],"tables":[{"name":"hidden","columns":[{"name":"secret_value","data_type":"integer","nullable":false}]}]}]}})).await;
    let marked = "/*漢😀*/ WITH a AS (SELECT CAST(1 AS INTEGER) AS local_id), q AS (SELECT a.* FROM a) SELECT q.loc|Suffix FROM q; SELECT h.secret_value FROM hidden h;";
    p.open(URI, &marked.replace('|', "")).await;
    let physical = p
        .request(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":URI}}),
        )
        .await;
    assert!(
        physical
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "hidden"),
        "warm physical positive control: {physical}"
    );
    for dialect in [
        "postgres",
        "mysql",
        "sqlserver",
        "hive",
        "clickhouse",
        "duckdb",
        "mariadb",
        "mssql",
    ] {
        let result = p
            .request("oxide/documentCompletions", params(marked, dialect))
            .await;
        assert_eq!(result["uri"], URI);
        assert_eq!(result["version"], 1);
        assert_eq!(result["dialect"], dialect);
        assert!(result.get("oxideAnalysisToken").is_none());
        let items = result["items"].as_array().unwrap();
        let field = items
            .iter()
            .find(|item| item["label"] == "local_id")
            .unwrap();
        assert!(field.to_string().to_uppercase().contains("INTEGER"));
        assert!(
            field["textEdit"]["range"]["end"]["character"]
                .as_u64()
                .unwrap()
                > params(marked, dialect)["position"]["character"]
                    .as_u64()
                    .unwrap()
        );
        assert!(items.iter().all(|item| item.get("data").is_none()
            && item.get("command").is_none()
            && item.get("additionalTextEdits").is_none()));
        assert!(!result.to_string().contains("secret_value"));
    }
    p.stop().await;
}
#[tokio::test]
async fn local_completion_wire_rejects_stale_unknown_and_authority_inputs() {
    let mut p = LspProcess::start().await;
    p.open(URI, "SEL").await;
    for (key, value, expected) in [
        ("version", json!(0), -32801),
        ("dialect", json!("POSTGRES"), -32602),
        ("oxideAnalysisToken", json!("private"), -32602),
        ("schemas", json!([]), -32602),
    ] {
        let mut request = params("SEL|", "postgres");
        request[key] = value;
        p.sequence += 1;
        let id = p.sequence;
        p.send(
            json!({"jsonrpc":"2.0","id":id,"method":"oxide/documentCompletions","params":request}),
        )
        .await;
        let result = p.until(|frame| frame["id"] == id).await;
        assert_eq!(result["error"]["code"], expected, "{key}: {result}");
    }
    let result = p
        .request("oxide/documentCompletions", params("SEL|", "postgres"))
        .await;
    assert!(result["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["label"] == "select"));
    p.stop().await;
}
#[tokio::test]
async fn local_table_variable_wire_uses_current_batch_and_qualified_row_shape() {
    let mut p = LspProcess::start().await;
    let marked = "DECLARE @left TABLE(left_id int); DECLARE @right TABLE(right_id decimal(19,4)); SELECT r.ri|Suffix FROM @left l JOIN @right r ON 1=1;";
    p.open(URI, &marked.replace('|', "")).await;
    let result = p
        .request("oxide/documentCompletions", params(marked, "mssql"))
        .await;
    let items = result["items"].as_array().unwrap();
    assert!(items.iter().any(|item| item["label"] == "right_id"
        && item.to_string().to_uppercase().contains("DECIMAL(19,4)")));
    assert!(!items.iter().any(|item| item["label"] == "left_id"));
    p.stop().await;
}

#[tokio::test]
async fn quoted_table_variable_constraint_columns_keep_identity_and_safe_member_edits() {
    let mut p = LspProcess::start().await;
    let marked = "DECLARE @local TABLE([UNIQUE] int, unique_count nvarchar(42)); SELECT l.UNIQUE|Suffix FROM @local l;";
    p.open(URI, &marked.replace('|', "")).await;
    let result = p
        .request("oxide/documentCompletions", params(marked, "sqlserver"))
        .await;
    let field = result["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "UNIQUE")
        .unwrap();
    assert!(
        field.to_string().to_uppercase().contains("INTEGER")
            || field.to_string().to_uppercase().contains("INT")
    );
    let edit = &field["textEdit"];
    assert!(
        edit["range"]["end"]["character"].as_u64().unwrap()
            > params(marked, "sqlserver")["position"]["character"]
                .as_u64()
                .unwrap()
    );
    assert!(
        edit["newText"]
            .as_str()
            .is_some_and(|text| text.contains("[UNIQUE]") || text.contains("\"UNIQUE\"")),
        "{field}"
    );
    p.stop().await;
}
