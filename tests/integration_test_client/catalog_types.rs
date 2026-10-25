use super::*;
use super::{execute_values::complete, joined_tables::position};
async fn configure(p: &mut LspProcess, uri: &str, ty: &str) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
        "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","functions":[],"tables":[{"name":"jobs","columns":[{"name":"value","data_type":ty,"nullable":true}]}]}],
        "fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):"postgres"},"completionPreferences":{"keywordCase":"upper"}
    }})).await;
}
#[tokio::test]
async fn catalog_column_types_control_standard_values_without_leaking_into_document_scope() {
    let mut p = LspProcess::start().await;
    for (index, (ty, truth)) in [
        ("int2", false),
        ("INT4", false),
        ("pg_catalog.int4", false),
        ("\"pg_catalog\".\"int8\"", false),
        ("pg_catalog.bool", true),
        ("\"pg_catalog\".\"bool\"", true),
        ("pg_catalog._bool", false),
        ("pg_catalog.bool [2]", false),
        ("pg_catalog.inet", false),
        ("pg_catalog.bytea", false),
        ("public.bool", true),
        ("pg_catalog.\"BOOL\"", true),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://catalog-types/{index}.mysql.sql");
        configure(&mut p, &uri, ty).await;
        let sql = "/*😀*/ SELECT * FROM jobs j WHERE j.value IS ";
        p.open(&uri, sql).await;
        let items = complete(&mut p, &uri, position(sql, sql.len())).await;
        assert_eq!(
            items.iter().any(|item| item["label"] == "TRUE"),
            *truth,
            "{ty}: {items:?}"
        );
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(
            local["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["label"] == "TRUE"),
            "document endpoint borrowed {ty}"
        );
        if index < 10 {
            let values = "/*😀*/ SELECT * FROM jobs j WHERE j.value = ";
            p.notify(
                "textDocument/didChange",
                json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":values}]}),
            )
            .await;
            let items = complete(&mut p, &uri, position(values, values.len())).await;
            assert_eq!(
                items.iter().any(|item| item["label"] == "TRUE"),
                *truth,
                "value {ty}: {items:?}"
            );
            if index < 4 {
                assert!(
                    items.iter().any(|item| item["label"] == "0"),
                    "numeric {ty}: {items:?}"
                );
            }
        }
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}
#[tokio::test]
async fn catalog_casts_compose_in_standard_and_document_only_truth_hints() {
    let mut p = LspProcess::start().await;
    for (index, (expression, truth)) in [
        ("CAST(1 AS pg_catalog.int4)", false),
        ("CAST(1 AS \"pg_catalog\".\"int4\")", false),
        ("CAST(1 AS pg_catalog.int4) + 2", false),
        (
            "COALESCE(CAST(1 AS pg_catalog.int2), CAST(2 AS INTEGER))",
            false,
        ),
        (
            "CASE WHEN TRUE THEN CAST(1 AS pg_catalog.int4) ELSE CAST(2 AS INTEGER) END",
            false,
        ),
        ("SUM(CAST(1 AS pg_catalog.int4))", false),
        ("(SELECT MAX(CAST(1 AS pg_catalog.int4)))", false),
        ("CAST(TRUE AS pg_catalog.bool)", true),
        ("CAST(TRUE AS \"pg_catalog\".\"bool\")", true),
        ("CAST('127.0.0.1' AS pg_catalog.inet)", false),
        ("CAST(NULL AS pg_catalog._bool)", false),
        ("CAST(1 AS public.int4)", true),
        ("CAST(TRUE AS pg_catalog.\"BOOL\")", true),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://catalog-types/cast-{index}.mysql.sql");
        configure(&mut p, &uri, "boolean").await;
        let prefix = format!("/*😀*/ SELECT {expression} IS ");
        let sql = prefix.clone() + ";";
        p.open(&uri, &sql).await;
        for local in [false, true] {
            let items = if local {
                p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(&sql,prefix.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await["items"].as_array().unwrap().clone()
            } else {
                complete(&mut p, &uri, position(&sql, prefix.len())).await
            };
            assert_eq!(
                items.iter().any(|item| item["label"] == "TRUE"),
                *truth,
                "{expression} local={local}: {items:?}"
            );
            assert!(items.iter().any(|item| item["label"] == "NULL"));
        }
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}
