use super::*;
use super::{execute_values::complete, joined_tables::position};
async fn configure(p: &mut LspProcess, uri: &str, functions: Vec<Value>) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
  "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","functions":functions,"tables":[{"name":"jobs","columns":[{"name":"flag","data_type":"boolean","nullable":true},{"name":"amount","data_type":"integer","nullable":false},{"name":"label","data_type":"text","nullable":true}]}]}],
  "fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):"postgres"},"completionPreferences":{"keywordCase":"upper"}
 }})).await;
}

#[tokio::test]
async fn postgres_expression_truth_hints_share_ast_cast_case_function_and_query_types() {
    let mut p = LspProcess::start().await;
    for (index, (expression, truth, local_truth)) in [
        ("CAST(j.amount AS INTEGER)", false, false),
        ("j.amount::numeric(12,2)", false, false),
        ("CASE WHEN j.flag THEN 1 ELSE 2 END", false, false),
        ("COALESCE(j.amount, 0)", false, true),
        ("LOWER(j.label)", false, false),
        ("LENGTH(j.label)", false, false),
        ("REPLACE(j.label,'a','b')", false, false),
        ("NULLIF(j.label,'x')", false, true),
        ("(j.amount + 1)", false, true),
        ("CAST(j.flag AS BOOLEAN)", true, true),
        ("BOOL_OR(j.flag)", true, true),
        ("(SELECT MAX(x.amount) FROM jobs x)", false, true),
        ("(SELECT BOOL_OR(x.flag) FROM jobs x)", true, true),
        ("(SELECT CAST(1 AS INTEGER))", false, false),
        ("(SELECT CAST(TRUE AS BOOLEAN))", true, true),
        ("'true'", true, true),
        ("CAST('true' AS TEXT)", false, false),
        ("unresolved_function(j.amount)", true, true),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://expression-probes/{index}.mysql.sql");
        configure(&mut p, &uri, vec![]).await;
        let prefix = format!("/*😀 字段*/ SELECT * FROM jobs j WHERE {expression} IS ");
        let sql = prefix.clone() + ";";
        p.open(&uri, &sql).await;
        for local in [false, true] {
            let items = if local {
                p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(&sql,prefix.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await["items"].as_array().unwrap().clone()
            } else {
                complete(&mut p, &uri, position(&sql, prefix.len())).await
            };
            // The document endpoint has no authority to infer a physical column's
            // result. Pure casts and literal functions still retain their known type.
            assert_eq!(
                items.iter().any(|item| item["label"] == "TRUE"),
                if local { *local_truth } else { *truth },
                "{expression} local={local}: {items:?}"
            );
            assert!(items.iter().any(|item| item["label"] == "NULL"));
            assert!(items.iter().all(|item| item["kind"] == 14));
        }
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn live_scalar_return_metadata_wins_but_document_only_probes_keep_builtin_scope() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://expression-probes/metadata.mysql.sql";
    let declaration = |ty: &str| json!({"name":"lower","routine_type":"function","parameters":[{"name":"value","data_type":"text","optional":false}],"return_type":ty});
    let sql = "SELECT LOWER('value') IS ";
    configure(&mut p, uri, vec![declaration("boolean")]).await;
    p.open(uri, sql).await;
    for (ty, truth) in [("boolean", true), ("integer", false), ("record", true)] {
        configure(&mut p, uri, vec![declaration(ty)]).await;
        let items = complete(&mut p, uri, position(sql, sql.len())).await;
        assert_eq!(
            items.iter().any(|item| item["label"] == "TRUE"),
            truth,
            "live {ty}: {items:?}"
        );
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(!local["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "TRUE"));
        assert!(local["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("data").is_none()));
    }
    p.stop().await;
}

#[tokio::test]
async fn a_short_is_prefix_in_a_large_document_keeps_bounded_keyword_fallback() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://expression-probes/large.mysql.sql";
    configure(&mut p, uri, vec![]).await;
    let prefix = "SELECT CAST(1 AS INTEGER) IS ";
    let sql = format!("{prefix};{}", " ".repeat(600_000));
    p.open(uri, &sql).await;
    let items = complete(&mut p, uri, position(&sql, prefix.len())).await;
    assert!(items.iter().any(|item| item["label"] == "NULL"));
    assert!(items.iter().all(|item| item["kind"] == 14));
    p.stop().await;
}
