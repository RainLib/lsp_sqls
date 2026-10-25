use super::*;
use super::{execute_values::complete, joined_tables::position};

#[tokio::test]
async fn catalog_value_literals_follow_explicit_profiles_over_the_uri_extension() {
    let mut p = LspProcess::start().await;
    for (index, (profile, ty, expected)) in [
        ("sqlserver", "bit", vec!["0", "1"]),
        ("mssql", "bit", vec!["0", "1"]),
        ("postgres", "bit(3)", vec!["B'000'", "B'001'"]),
        ("postgres", "bit varying(8)", vec!["B'0'", "B'1'"]),
        ("mysql", "BIT(3)", vec!["B'000'", "B'001'"]),
        ("duckdb", "BIT", vec!["'0'::BIT", "'1'::BIT"]),
        ("sqlite", "BIT", vec!["0", "1"]),
        ("postgres", "boolean[]", vec!["NULL"]),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://dialect-values/{index}.mysql.sql");
        p.notify("workspace/didChangeConfiguration",json!({"settings":{
            "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[{"name":"jobs","columns":[{"name":"flags","data_type":ty,"nullable":true},{"name":"TRUE","data_type":"int","nullable":false}]}],"functions":[]}],
            "fileSchemas":{(&uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(&uri):profile},"completionPreferences":{"keywordCase":"lower"}
        }})).await;
        let sql = "/* 中文😀 */\r\nSELECT * FROM jobs WHERE flags = ";
        p.open(&uri, sql).await;
        let items = complete(&mut p, &uri, position(sql, sql.len())).await;
        for label in expected {
            assert!(
                items.iter().any(|item| item["label"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(label))),
                "{profile} {ty}: missing {label}: {items:?}"
            );
        }
        assert!(
            !items.iter().any(|item| item["kind"] == 14
                && ["TRUE", "FALSE"].iter().any(|label| item["label"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(label)))),
            "{profile} {ty}: {items:?}"
        );
        assert!(
            items.iter().any(|item| item["label"] == "null"),
            "keyword casing remains lower: {items:?}"
        );
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn document_only_value_literals_use_local_types_without_catalog_authority() {
    let mut p = LspProcess::start().await;
    for (index, (profile, marked, expected)) in [
        (
            "sqlserver",
            "DECLARE @jobs TABLE(flags bit); SELECT * FROM @jobs WHERE flags = |;",
            vec!["0", "1"],
        ),
        (
            "mssql",
            "DECLARE @jobs TABLE(flags bit); SELECT * FROM @jobs WHERE flags IN (1, |);",
            vec!["0", "1"],
        ),
        (
            "postgres",
            "WITH q AS(SELECT CAST(1 AS bit(3)) AS flags) SELECT * FROM q WHERE flags = |;",
            vec!["B'000'", "B'001'"],
        ),
        (
            "duckdb",
            "WITH q AS(SELECT CAST('0' AS BIT) AS flags) SELECT * FROM q WHERE flags = |;",
            vec!["'0'::BIT", "'1'::BIT"],
        ),
    ]
    .iter()
    .enumerate()
    {
        let uri = format!("oxide://document-values/{index}.mysql.sql");
        let at = marked.find('|').unwrap();
        let sql = marked.replace('|', "");
        p.open(&uri, &sql).await;
        let response=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,at),"preferences":{"keywordCase":"lower","functionCase":"lower","tableAlias":"none"}})).await;
        let items = response["items"].as_array().unwrap();
        for label in expected {
            assert!(
                items.iter().any(|item| item["label"] == *label),
                "{profile}: missing {label}: {items:?}"
            );
        }
        assert!(
            !items.iter().any(|item| item["kind"] == 14
                && ["true", "false"].contains(&item["label"].as_str().unwrap_or_default())),
            "{profile}: {items:?}"
        );
        assert!(items
            .iter()
            .all(|item| item.get("data").is_none() && item.get("command").is_none()));
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}
