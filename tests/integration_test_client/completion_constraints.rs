use super::*;
use super::{execute_values::complete, joined_tables::position};

#[tokio::test]
async fn escape_character_roles_and_edits_reach_standard_catalog_and_document_routes() {
    let mut p = LspProcess::start().await;
    for (index, (profile, operator, expected)) in [
        ("postgres", "LIKE", "text"),
        ("postgres", "NOT SIMILAR TO", "text"),
        ("duckdb", "NOT ILIKE", "varchar"),
        ("mysql", "LIKE", "varchar"),
        ("mariadb", "NOT LIKE", "varchar"),
        ("sqlite", "LIKE", "text"),
        ("sqlserver", "NOT LIKE", "nvarchar"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://escape-role/{index}.sql");
        let prefix = format!("/*😀*/\r\nSELECT 1 WHERE 'oxide_1' {operator} 'oxide#_1' ESCAPE ");
        let text = format!("{prefix}Suffix尾;");
        configure(&mut p, &uri, profile, "escape-role", "integer").await;
        p.open(&uri, &text).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &text, prefix.len(), profile, "escape-role"),
            )
            .await;
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&text,prefix.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        let standard = complete(&mut p, &uri, position(&text, prefix.len())).await;
        for response in [&catalog, &local] {
            assert_eq!(response["valueType"], expected, "{profile}");
            assert_eq!(response["valueRole"], "pattern-escape", "{profile}");
            assert!(
                response.get("constraints").is_none()
                    && response.get("logicalContinuation").is_none()
            );
        }
        for items in [
            catalog["items"].as_array().unwrap(),
            local["items"].as_array().unwrap(),
            &standard,
        ] {
            assert!(
                !items.iter().any(|item| ["TRUE", "FALSE", "0", "''"]
                    .iter()
                    .any(|label| item["label"] == *label)),
                "{profile}: {items:?}"
            );
            let item = items.iter().find(|item| item["label"] == "'#'").unwrap();
            assert_eq!(item["textEdit"]["newText"], "'#'");
            assert_eq!(
                item["textEdit"]["range"]["start"],
                json!(position(&text, prefix.len()))
            );
            assert_eq!(
                item["textEdit"]["range"]["end"],
                json!(position(&text, prefix.len() + "Suffix尾".len()))
            );
        }
    }
    p.stop().await;
}

#[tokio::test]
async fn escape_roles_and_results_respect_versions_without_warming_document_only_metadata() {
    let mut p = LspProcess::start().await;
    for (index, (version, operator, known)) in [
        (None, "LIKE", false),
        (Some("26.5.2"), "LIKE", false),
        (Some("26.6.1.123"), "LIKE", true),
        (Some("26.9"), "SIMILAR TO", false),
        (Some("26.10"), "SIMILAR TO", true),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://escape-version/{index}.sql");
        let token = format!("escape-version-{index}");
        p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":token,"schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","server_version":version,"tables":[],"functions":[]}],"fileSchemas":{(uri.as_str()):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri.as_str()):"clickhouse"}}})).await;
        assert_eq!(
            p.request(
                "oxide/analysisConfiguration",
                json!({"oxideAnalysisToken":token})
            )
            .await["applied"],
            true
        );
        let text = format!("SELECT 1 WHERE 'x' {operator} 'x' ESCAPE ");
        p.open(&uri, &text).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &text, text.len(), "clickhouse", &token),
            )
            .await;
        assert_eq!(
            catalog["valueRole"].as_str(),
            known.then_some("pattern-escape"),
            "{version:?}: {operator}"
        );
        assert_eq!(catalog["valueType"].as_str(), known.then_some("String"));
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"clickhouse","position":position(&text,text.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(local.get("valueRole").is_none() && local.get("valueType").is_none());
    }
    p.stop().await;
}

#[tokio::test]
async fn escaped_cte_predicates_use_live_function_types_without_leaking_them_to_document_mode() {
    let mut p = LspProcess::start().await;
    for (index, (return_type, known)) in
        [("text", true), ("integer", false)].into_iter().enumerate()
    {
        let uri = format!("oxide://escape-return/{index}.sql");
        let token = format!("escape-return-{index}");
        configure(&mut p, &uri, "postgres", &token, return_type).await;
        let text="WITH q AS (SELECT 'x' LIKE 'x' ESCAPE semantic_result() AS flag) SELECT 1 FROM q WHERE q.flag = ";
        p.open(&uri, text).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, text, text.len(), "postgres", &token),
            )
            .await;
        assert_eq!(catalog["valueType"].as_str(), known.then_some("boolean"));
        assert!(catalog.get("valueRole").is_none());
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(text,text.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(local.get("valueType").is_none() && local.get("valueRole").is_none());
    }
    p.stop().await;
}

#[tokio::test]
async fn typed_value_ranges_and_clickhouse_versions_survive_all_completion_routes() {
    let mut p = LspProcess::start().await;
    for (index, (profile, expression, partial, label)) in [
        ("duckdb", "'oxide' ILIKE 'OX%'", "", "TRUE"),
        ("hive", "'oxide' RLIKE '^ox'", "T", "TRUE"),
        ("clickhouse", "match('oxide','^ox')", "", "0"),
        ("sqlite", "'oxide' GLOB 'ox*'", "", "0"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://pattern-edits/{index}.sql");
        let source=format!("/*😀*/\r\nWITH q AS (SELECT {expression} AS matched) SELECT 1 FROM q WHERE q.matched = ");
        let prefix = format!("{source}{partial}");
        let sql = format!("{prefix}Suffix尾; SELECT 2");
        configure(&mut p, &uri, profile, "pattern-edits", "integer").await;
        p.open(&uri, &sql).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &sql, prefix.len(), profile, "pattern-edits"),
            )
            .await;
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,prefix.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        let standard = complete(&mut p, &uri, position(&sql, prefix.len())).await;
        for items in [
            catalog["items"].as_array().unwrap(),
            local["items"].as_array().unwrap(),
            &standard,
        ] {
            let values = items
                .iter()
                .filter(|item| item["label"] == label)
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 1, "{profile}: {items:?}");
            assert_eq!(
                values[0]["textEdit"]["range"]["start"],
                json!(position(&sql, source.len())),
                "{profile}"
            );
            assert_eq!(
                values[0]["textEdit"]["range"]["end"],
                json!(position(&sql, prefix.len() + "Suffix尾".len())),
                "{profile}"
            );
            assert!(values[0]["textEdit"]["newText"]
                .as_str()
                .unwrap()
                .contains(label));
        }
    }
    for (index, (version, known)) in [(None, false), (Some("26.7"), false), (Some("26.8"), true)]
        .into_iter()
        .enumerate()
    {
        let uri = format!("oxide://negative-regex/{index}.sql");
        let sql = "WITH q AS (SELECT 'oxide' !~ '^db' AS flag) SELECT 1 FROM q WHERE q.flag = ";
        p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":"negative-regex","schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","server_version":version,"tables":[],"functions":[]}],"fileSchemas":{(uri.as_str()):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri.as_str()):"clickhouse"}}})).await;
        p.open(&uri, sql).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, sql, sql.len(), "clickhouse", "negative-regex"),
            )
            .await;
        assert_eq!(
            catalog["valueType"].as_str(),
            known.then_some("UInt8"),
            "{version:?}"
        );
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"clickhouse","position":position(sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(
            local.get("valueType").is_none(),
            "document-only cannot borrow a warm server version"
        );
    }
    p.stop().await;
}

#[tokio::test]
async fn pattern_result_types_reach_standard_and_bound_local_cte_values() {
    let mut p = LspProcess::start().await;
    for (index, (profile, expression, expected)) in [
        ("duckdb", "'oxide' ILIKE 'ox%'", "boolean"),
        ("duckdb", "'oxide' SIMILAR TO '(ox|db).*'", "boolean"),
        ("duckdb", "regexp_matches('oxide','^ox')", "boolean"),
        ("clickhouse", "'oxide' ILIKE 'ox%'", "UInt8"),
        ("clickhouse", "match('oxide','^ox')", "UInt8"),
        ("hive", "'oxide' RLIKE '^ox'", "boolean"),
        ("mysql", "'oxide' REGEXP '^ox'", "bigint"),
        ("sqlite", "'oxide' GLOB 'ox*'", "integer"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://patterns/{index}.sql");
        let sql =
            format!("WITH q AS (SELECT {expression} AS flag) SELECT 1 FROM q WHERE q.flag = ");
        configure(&mut p, &uri, profile, "pattern-result", "integer").await;
        p.open(&uri, &sql).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &sql, sql.len(), profile, "pattern-result"),
            )
            .await;
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        let standard = complete(&mut p, &uri, position(&sql, sql.len())).await;
        for response in [&catalog, &local] {
            assert_eq!(response["valueType"], expected, "{profile}: {expression}");
            assert!(response.get("logicalContinuation").is_none());
        }
        let label = if expected == "boolean" { "TRUE" } else { "0" };
        for items in [
            catalog["items"].as_array().unwrap(),
            local["items"].as_array().unwrap(),
            &standard,
        ] {
            assert!(
                items.iter().any(|item| item["label"] == label),
                "{profile}: {expression}"
            );
            if expected != "boolean" {
                assert!(
                    !items.iter().any(|item| item["label"] == "TRUE"),
                    "{profile}: {expression}"
                );
            }
        }
    }
    p.stop().await;
}

#[tokio::test]
async fn pattern_rhs_snapshots_supply_text_and_replace_complete_utf16_suffixes() {
    let mut p = LspProcess::start().await;
    for (index, (profile, operator, expected)) in [
        ("postgres", "ILIKE", "text"),
        ("duckdb", "NOT SIMILAR /*😀*/ TO", "varchar"),
        ("mysql", "REGEXP", "varchar"),
        ("mariadb", "RLIKE", "varchar"),
        ("hive", "RLIKE", "string"),
        ("clickhouse", "REGEXP", "String"),
        ("sqlite", "NOT GLOB", "text"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://patterns/rhs-{index}.sql");
        let sql = format!("/*😀*/ SELECT 1 WHERE 'oxide' {operator} ");
        configure(&mut p, &uri, profile, "pattern-rhs", "integer").await;
        p.open(&uri, &sql).await;
        let result = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &sql, sql.len(), profile, "pattern-rhs"),
            )
            .await;
        assert_eq!(result["valueType"], expected, "{profile} {operator}");
        assert!(result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "''"));
    }
    p.stop().await;
}

#[tokio::test]
async fn live_regex_return_binding_precedes_builtin_and_never_warms_document_only_types() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://patterns/live.sql";
    let sql =
        "WITH q AS (SELECT regexp_matches('oxide','^ox') AS flag) SELECT 1 FROM q WHERE q.flag = ";
    for (index, ty) in ["integer", "varchar"].into_iter().enumerate() {
        let token = format!("pattern-live-{index}");
        p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":token,"fileDialects":{(uri):"duckdb"},"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[],"functions":[{"name":"regexp_matches","routine_type":"function","parameters":[{"name":"value","data_type":"varchar","optional":false},{"name":"pattern","data_type":"varchar","optional":false}],"return_type":ty}]}]}})).await;
        assert_eq!(
            p.request(
                "oxide/analysisConfiguration",
                json!({"oxideAnalysisToken":token})
            )
            .await["applied"],
            true
        );
        if index == 0 {
            p.open(uri, sql).await;
        }
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(uri, sql, sql.len(), "duckdb", &token),
            )
            .await;
        assert_eq!(catalog["valueType"], ty);
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"duckdb","position":position(sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert_eq!(local["valueType"], "boolean");
    }
    p.stop().await;
}

#[tokio::test]
async fn analytic_exists_and_case_continuations_share_real_standard_and_bound_snapshots() {
    let mut p = LspProcess::start().await;
    for profile in [
        "postgres",
        "mysql",
        "mariadb",
        "sqlite",
        "sqlite3",
        "duckdb",
        "hive",
        "hql",
        "clickhouse",
        "ch",
    ] {
        for (index, expression) in [
            "EXISTS(SELECT NULL, 'AND OR')",
            "NOT EXISTS(SELECT NULL WHERE 1=0)",
            "NOT (NOT EXISTS(SELECT 1))",
            "EXISTS(SELECT 1) AND NOT EXISTS(SELECT 1 WHERE 1=0)",
            "CASE WHEN 1=1 THEN EXISTS(SELECT 1) ELSE EXISTS(SELECT 2) END",
        ]
        .into_iter()
        .enumerate()
        {
            let uri = format!("oxide://predicates/{profile}-{index}.sql");
            let sql = format!("/*😀*/ SELECT 1 WHERE {expression} ");
            configure(&mut p, &uri, profile, "predicate-contract", "integer").await;
            p.open(&uri, &sql).await;
            let catalog = p
                .request(
                    "oxide/completionSnapshot",
                    params(&uri, &sql, sql.len(), profile, "predicate-contract"),
                )
                .await;
            let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
            let standard = complete(&mut p, &uri, position(&sql, sql.len())).await;
            for response in [&catalog, &local] {
                assert_eq!(
                    response["logicalContinuation"], true,
                    "{profile}: {expression}"
                );
                assert!(
                    response.get("valueType").is_none() && response.get("constraints").is_none()
                );
            }
            for items in [
                catalog["items"].as_array().unwrap(),
                local["items"].as_array().unwrap(),
                &standard,
            ] {
                assert!(
                    items
                        .iter()
                        .any(|item| item["label"] == "AND" && item["kind"] == 14),
                    "{profile}: {expression}"
                );
            }
            assert!(local.get("oxideAnalysisToken").is_none());
        }
    }
    p.stop().await;
}

#[tokio::test]
async fn predicate_projection_value_types_propagate_through_local_ctes_without_catalog_borrowing() {
    let mut p = LspProcess::start().await;
    for (profile, expected) in [
        ("postgres", "boolean"),
        ("mysql", "bigint"),
        ("sqlite", "integer"),
        ("duckdb", "boolean"),
        ("hive", "boolean"),
        ("clickhouse", "UInt8"),
    ] {
        let uri = format!("oxide://predicates/{profile}-cte.sql");
        let sql="WITH q AS (SELECT CASE WHEN 1=1 THEN EXISTS(SELECT NULL) ELSE NOT EXISTS(SELECT 1 WHERE 1=0) END AS flag) SELECT 1 FROM q WHERE q.flag = ";
        configure(&mut p, &uri, profile, "predicate-projection", "integer").await;
        p.open(&uri, sql).await;
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert_eq!(local["valueType"], expected, "{profile}");
        assert!(local.get("logicalContinuation").is_none());
        let label = if matches!(profile, "postgres" | "duckdb" | "hive") {
            "TRUE"
        } else {
            "0"
        };
        assert!(
            local["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["label"] == label),
            "{profile}"
        );
    }
    p.stop().await;
}

#[tokio::test]
async fn analytic_exists_keyword_edits_keep_multiline_utf16_and_full_replacement_suffix() {
    let mut p = LspProcess::start().await;
    for profile in ["duckdb", "hive", "clickhouse"] {
        let uri = format!("oxide://predicates/{profile}-suffix.sql");
        let prefix = "/*😀*/\r\nSELECT 1 WHERE NOT EXISTS(SELECT NULL WHERE 1=0) O";
        let sql = format!("{prefix}Suffix;");
        configure(&mut p, &uri, profile, "predicate-edits", "integer").await;
        p.open(&uri, &sql).await;
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,prefix.len()),"preferences":{"keywordCase":"lower","functionCase":"lower","tableAlias":"none"}})).await;
        assert_eq!(local["logicalContinuation"], true, "{profile}");
        let items = local["items"].as_array().unwrap();
        let item = items
            .iter()
            .find(|item| item["label"] == "or" && item["kind"] == 14)
            .expect("real logical keyword");
        assert_eq!(item["textEdit"]["newText"], "or ");
        assert_eq!(
            item["textEdit"]["range"]["start"],
            json!(position(&sql, prefix.len() - 1))
        );
        assert_eq!(
            item["textEdit"]["range"]["end"],
            json!(position(&sql, sql.len() - 1))
        );
        assert!(!items
            .iter()
            .any(|item| item["label"] == "and" && item["kind"] == 14));
    }
    p.stop().await;
}
async fn configure(p: &mut LspProcess, uri: &str, profile: &str, token: &str, ty: &str) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":token,"schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[{"name":"users","columns":[{"name":"value","data_type":"integer","nullable":false}]}],"functions":[{"name":"semantic_result","routine_type":"function","parameters":[],"return_type":ty}]}],"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):profile},"completionPreferences":{"keywordCase":"upper"}}})).await;
    assert_eq!(
        p.request(
            "oxide/analysisConfiguration",
            json!({"oxideAnalysisToken":token})
        )
        .await["applied"],
        true
    );
}
fn params(uri: &str, text: &str, prefix: usize, profile: &str, token: &str) -> Value {
    json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(text,prefix),"oxideAnalysisToken":token,"context":{"triggerKind":1}})
}
#[tokio::test]
async fn scalar_logical_continuation_profiles_match_standard_catalog_and_document_results() {
    let mut p = LspProcess::start().await;
    for (index, (profile, expression, expected)) in [
        ("postgres", "CAST(1 AS INTEGER)", false),
        ("postgresql", "CAST(TRUE AS BOOLEAN)", true),
        ("mysql", "CAST(1 AS DECIMAL(8,2))", true),
        ("mariadb", "CAST(1 AS DECIMAL(8,2))", true),
        ("sqlserver", "CAST(1 AS BIT)", false),
        ("mssql", "CAST(1 AS BIT)", false),
        ("sqlite", "CAST(1 AS INTEGER)", true),
        ("sqlite3", "CAST(1 AS INTEGER)", true),
        ("duckdb", "CAST(1 AS INTEGER)", true),
        ("duckdb", "CAST('true' AS VARCHAR)", true),
        ("duckdb", "CAST('2026-01-01' AS DATE)", false),
        ("hive", "CAST(TRUE AS BOOLEAN)", true),
        ("hql", "CAST(1 AS INT)", false),
        ("clickhouse", "CAST(1 AS UInt8)", true),
        ("ch", "CAST(1 AS Decimal64(2))", false),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://logical/{profile}-{index}.sql");
        let sql = format!("/*😀*/ SELECT 1 WHERE {expression} ");
        configure(&mut p, &uri, profile, "logical-profile", "integer").await;
        p.open(&uri, &sql).await;
        let snapshot = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &sql, sql.len(), profile, "logical-profile"),
            )
            .await;
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        let standard = complete(&mut p, &uri, position(&sql, sql.len())).await;
        for response in [&snapshot, &local] {
            assert_eq!(
                response["logicalContinuation"], expected,
                "{profile} {expression}"
            );
            assert!(response.get("valueType").is_none() && response.get("constraints").is_none());
        }
        for items in [
            snapshot["items"].as_array().unwrap(),
            local["items"].as_array().unwrap(),
            &standard,
        ] {
            assert_eq!(
                // ClickHouse also registers and(...) as a function. Its
                // expression template is separate from an infix operator.
                items
                    .iter()
                    .any(|item| item["label"] == "AND"
                        && matches!(item["kind"].as_u64(), Some(14 | 24))),
                expected,
                "{profile} {expression}"
            );
        }
        if profile == "ch" {
            assert!(local["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["label"] == "AND"
                    && item["kind"] == 3
                    && item["insertText"]
                        .as_str()
                        .is_some_and(|text| text.starts_with("AND("))));
        }
    }
    p.stop().await;
}
#[tokio::test]
async fn scalar_logical_knowledge_refreshes_without_borrowing_warm_document_functions() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://logical/live.sql";
    let sql = "SELECT 1 WHERE semantic_result() ";
    configure(&mut p, uri, "postgres", "logical-integer", "integer").await;
    p.open(uri, sql).await;
    for (token, ty, expected) in [
        ("logical-integer", "integer", false),
        ("logical-boolean", "boolean", true),
    ] {
        configure(&mut p, uri, "postgres", token, ty).await;
        let result = p
            .request(
                "oxide/completionSnapshot",
                params(uri, sql, sql.len(), "postgres", token),
            )
            .await;
        assert_eq!(result["logicalContinuation"], expected);
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(local.get("logicalContinuation").is_none());
        assert!(local.get("oxideAnalysisToken").is_none());
    }
    p.stop().await;
}
#[tokio::test]
async fn scalar_logical_keyword_edits_preserve_utf16_suffix_and_case() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://logical/edit.sql";
    let prefix = "/*😀*/ SELECT 1 WHERE CAST(TRUE AS BOOLEAN) A";
    let sql = format!("{prefix}Suffix;");
    configure(&mut p, uri, "postgres", "logical-edit", "boolean").await;
    p.open(uri, &sql).await;
    let snapshot = p
        .request(
            "oxide/completionSnapshot",
            params(uri, &sql, prefix.len(), "postgres", "logical-edit"),
        )
        .await;
    assert_eq!(snapshot["logicalContinuation"], true);
    let item = snapshot["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "AND")
        .unwrap();
    assert_eq!(
        item["textEdit"]["range"]["start"],
        position(&sql, prefix.len() - 1)
    );
    assert_eq!(
        item["textEdit"]["range"]["end"],
        position(&sql, sql.len() - 1)
    );
    assert_eq!(item["textEdit"]["newText"], "AND ");
    let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(&sql,prefix.len()),"preferences":{"keywordCase":"lower","functionCase":"upper","tableAlias":"none"}})).await;
    let item = local["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "and")
        .unwrap();
    assert_eq!(item["textEdit"]["newText"], "and ");
    assert_eq!(local["logicalContinuation"], true);
    p.stop().await;
}
#[tokio::test]
async fn schema_free_typed_values_match_standard_and_both_snapshot_paths() {
    let mut p = LspProcess::start().await;
    for (profile, expression) in [
        ("postgres", "STRPOS('ab','b')"),
        ("mysql", "CAST(1 AS DECIMAL(12,2))"),
        ("sqlserver", "CAST(1 AS INT)"),
        ("sqlite", "CAST(1 AS INTEGER)"),
        ("duckdb", "CAST(1 AS INTEGER)"),
        ("hive", "CAST(1 AS INT)"),
        ("clickhouse", "CAST(1 AS Int64)"),
        ("postgresql", "STRPOS('ab','b')"),
        ("pgsql", "STRPOS('ab','b')"),
        ("psql", "STRPOS('ab','b')"),
        ("mssql", "CAST(1 AS INT)"),
        ("tsql", "CAST(1 AS INT)"),
        ("sqlite3", "CAST(1 AS INTEGER)"),
        ("hql", "CAST(1 AS INT)"),
        ("ch", "CAST(1 AS Int64)"),
        ("mariadb", "CAST(1 AS DECIMAL(12,2))"),
    ] {
        let uri = format!("oxide://no-catalog/{profile}.sql");
        let sql = format!("/*😀*/ SELECT {expression} >= ");
        let token = "no-catalog-types";
        p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":token,"defaultDialect":profile,"schemas":[],"fileSchemas":{},"fileDialects":{(uri.as_str()):profile}}})).await;
        assert_eq!(
            p.request(
                "oxide/analysisConfiguration",
                json!({"oxideAnalysisToken":token})
            )
            .await["applied"],
            true
        );
        p.open(&uri, &sql).await;
        let snapshot = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &sql, sql.len(), profile, token),
            )
            .await;
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(snapshot["valueType"].is_string(), "{profile}");
        assert_eq!(snapshot["valueType"], local["valueType"], "{profile}");
        let standard = complete(&mut p, &uri, position(&sql, sql.len())).await;
        for items in [
            snapshot["items"].as_array().unwrap(),
            local["items"].as_array().unwrap(),
            &standard,
        ] {
            assert!(items.iter().any(|item| item["label"] == "0"), "{profile}");
            assert!(
                !items.iter().any(|item| item["label"] == "TRUE"),
                "{profile}"
            );
        }
    }
    p.stop().await;
}
#[tokio::test]
async fn completion_snapshots_reject_invalid_utf16_positions_before_issuing_knowledge() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://position/check.sql";
    let sql = "/*😀*/\r\nSELECT CAST(1 AS INTEGER) = ";
    configure(&mut p, uri, "postgres", "position-check", "integer").await;
    p.open(uri, sql).await;
    for method in ["oxide/documentCompletions", "oxide/completionSnapshot"] {
        for at in [
            json!({"line":0,"character":3}),
            json!({"line":0,"character":7}),
            json!({"line":1,"character":29}),
            json!({"line":4,"character":0}),
            json!({"line":4294967295_u32,"character":4294967295_u32}),
        ] {
            p.sequence += 1;
            let id = p.sequence;
            let mut request =
                json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":at});
            if method == "oxide/completionSnapshot" {
                request["oxideAnalysisToken"] = json!("position-check");
            } else {
                request["preferences"] =
                    json!({"keywordCase":"upper","functionCase":"upper","tableAlias":"none"});
            }
            p.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":request}))
                .await;
            let response = p.until(|message| message["id"] == id).await;
            assert_eq!(response["error"]["code"], -32602, "{response}");
            assert!(response.get("result").is_none());
        }
        let mut request = json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":{"line":1,"character":28}});
        if method == "oxide/completionSnapshot" {
            request["oxideAnalysisToken"] = json!("position-check");
        } else {
            request["preferences"] =
                json!({"keywordCase":"upper","functionCase":"upper","tableAlias":"none"});
        }
        assert!(p.request(method, request).await["valueType"].is_string());
    }
    p.stop().await;
}
#[tokio::test]
async fn comparison_value_types_cover_profiles_and_standard_document_snapshots() {
    let mut p = LspProcess::start().await;
    for (profile, expression) in [
        ("postgres", "STRPOS('ab','b')"),
        ("mysql", "CAST(1 AS DECIMAL(12,2))"),
        ("sqlserver", "CAST(1 AS INT)"),
        ("sqlite", "CAST(1 AS INTEGER)"),
        ("duckdb", "CAST(1 AS INTEGER)"),
        ("hive", "CAST(1 AS INT)"),
        ("clickhouse", "CAST(1 AS Int64)"),
    ] {
        let uri = format!("oxide://value/{profile}.sql");
        configure(&mut p, &uri, profile, "value-current", "integer").await;
        let sql = format!("/*😀*/ SELECT 1 WHERE {expression} >= ");
        p.open(&uri, &sql).await;
        let catalog = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &sql, sql.len(), profile, "value-current"),
            )
            .await;
        assert!(catalog["valueType"].is_string(), "{profile}: {catalog}");
        assert!(catalog.get("constraints").is_none());
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":profile,"position":position(&sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert_eq!(local["valueType"], catalog["valueType"], "{profile}");
        for response in [&catalog, &local] {
            let items = response["items"].as_array().unwrap();
            assert!(
                items.iter().any(|item| item["label"] == "0"),
                "{profile}: {response}"
            );
            assert!(
                !items.iter().any(|item| item["label"] == "TRUE"),
                "{profile}: {response}"
            );
        }
        let standard = complete(&mut p, &uri, position(&sql, sql.len())).await;
        assert!(
            standard.iter().any(|item| item["label"] == "0"),
            "{profile}"
        );
        assert!(
            !standard.iter().any(|item| item["label"] == "TRUE"),
            "{profile}"
        );
    }
    p.stop().await;
}
#[tokio::test]
async fn comparison_catalog_refresh_and_unknown_document_types_remain_independent() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://value/live.sql";
    let sql = "SELECT semantic_result() = ";
    configure(&mut p, uri, "postgres", "value-integer", "pg_catalog.int4").await;
    p.open(uri, sql).await;
    for (token, data_type, truth) in [
        ("value-integer", "pg_catalog.int4", false),
        ("value-bool", "boolean", true),
    ] {
        configure(&mut p, uri, "postgres", token, data_type).await;
        let result = p
            .request(
                "oxide/completionSnapshot",
                params(uri, sql, sql.len(), "postgres", token),
            )
            .await;
        assert_eq!(result["valueType"], data_type);
        assert_eq!(
            result["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["label"] == "TRUE"),
            truth
        );
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(local.get("valueType").is_none());
        assert!(local.get("oxideAnalysisToken").is_none());
    }
    p.stop().await;
}
#[tokio::test]
async fn catalog_constraints_refresh_without_authorizing_document_only_functions() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://constraint/live.mysql.sql";
    configure(
        &mut p,
        uri,
        "postgres",
        "constraint-integer",
        "pg_catalog.int4",
    )
    .await;
    let sql = "/*😀*/ SELECT semantic_result() IS ";
    p.open(uri, sql).await;
    for (token, ty, truth) in [
        ("constraint-integer", "pg_catalog.int4", false),
        ("constraint-boolean", "boolean", true),
    ] {
        configure(&mut p, uri, "postgres", token, ty).await;
        let result = p
            .request(
                "oxide/completionSnapshot",
                params(uri, sql, sql.len(), "postgres", token),
            )
            .await;
        assert_eq!(result["oxideAnalysisToken"], token);
        assert_eq!(result["constraints"]["slot"], "is");
        assert_eq!(
            result["constraints"]["allowedKeywords"]
                .as_array()
                .unwrap()
                .iter()
                .any(|label| label == "TRUE"),
            truth
        );
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert!(local["constraints"]["allowedKeywords"]
            .as_array()
            .unwrap()
            .iter()
            .any(|label| label == "TRUE"));
        assert!(local.get("oxideAnalysisToken").is_none());
        assert!(local["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("data").is_none()));
    }
    p.stop().await;
}
#[tokio::test]
async fn snapshot_guards_reject_stale_configuration_document_and_profile() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://constraint/guard.mysql.sql";
    configure(&mut p, uri, "postgres", "constraint-current", "integer").await;
    let sql = "SELECT TRUE IS ";
    p.open(uri, sql).await;
    for (index, patch) in [
        json!({"version":2}),
        json!({"oxideAnalysisToken":"old"}),
        json!({"dialect":"mysql"}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut value = params(uri, sql, sql.len(), "postgres", "constraint-current");
        for (key, value_patch) in patch.as_object().unwrap() {
            value[key] = value_patch.clone();
        }
        let id = 10000 + index;
        p.send(json!({"jsonrpc":"2.0","id":id,"method":"oxide/completionSnapshot","params":value}))
            .await;
        let response = p.until(|message| message["id"] == id).await;
        assert_eq!(response["error"]["code"], -32801, "{response}");
    }
    let result = p
        .request(
            "oxide/completionSnapshot",
            params(uri, sql, sql.len(), "postgres", "constraint-current"),
        )
        .await;
    assert_eq!(result["version"], 1);
    p.stop().await;
}
#[tokio::test]
async fn complete_constraints_remain_independent_of_prefix_and_client_keyword_case() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://constraint/prefix.mysql.sql";
    configure(&mut p, uri, "postgres", "constraint-prefix", "integer").await;
    let prefix = "/*😀*/ SELECT STRPOS('a','b') IS N";
    let sql = prefix.to_owned() + "Suffix;";
    p.open(uri, &sql).await;
    let result = p
        .request(
            "oxide/completionSnapshot",
            params(uri, &sql, prefix.len(), "postgres", "constraint-prefix"),
        )
        .await;
    assert!(result["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["label"].as_str().unwrap().starts_with('N')));
    assert!(result["constraints"]["allowedKeywords"]
        .as_array()
        .unwrap()
        .iter()
        .any(|label| label == "DISTINCT FROM"));
    let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(&sql,prefix.len()),"preferences":{"keywordCase":"lower","functionCase":"lower","tableAlias":"none"}})).await;
    assert!(local["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["label"].as_str().unwrap().starts_with('n')));
    assert_eq!(local["constraints"], result["constraints"]);
    p.stop().await;
}
#[tokio::test]
async fn sqlite_operand_completion_and_standard_lsp_data_remain_compatible() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://constraint/sqlite.mysql.sql";
    configure(&mut p, uri, "sqlite", "constraint-sqlite", "integer").await;
    let sql = "SELECT * FROM users WHERE value IS ";
    p.open(uri, sql).await;
    let standard = complete(&mut p, uri, position(sql, sql.len())).await;
    let result = p
        .request(
            "oxide/completionSnapshot",
            params(uri, sql, sql.len(), "sqlite", "constraint-sqlite"),
        )
        .await;
    assert!(result.get("constraints").is_none());
    let mut labels = standard
        .iter()
        .map(|item| item["label"].clone())
        .collect::<Vec<_>>();
    let mut snapshot = result["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["label"].clone())
        .collect::<Vec<_>>();
    labels.sort_by_key(Value::to_string);
    snapshot.sort_by_key(Value::to_string);
    assert_eq!(labels, snapshot);
    p.stop().await;
}

#[tokio::test]
async fn explicit_constraints_preserve_each_profiles_truth_and_versioned_operators() {
    let mut p = LspProcess::start().await;
    for (index, (profile, version, truth, distinct)) in [
        ("postgres", None, false, true),
        ("mysql", None, true, false),
        ("hive", None, true, false),
        ("duckdb", None, true, true),
        ("sqlserver", Some("15.0.4382"), false, false),
        ("sqlserver", Some("16.0.4195"), false, true),
        ("clickhouse", Some("25.9.1"), false, false),
        ("clickhouse", Some("25.10.1"), false, true),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://constraint/profile-{index}.mysql.sql");
        p.notify("workspace/didChangeConfiguration",json!({"settings":{"oxideAnalysisToken":"profile-constraint","schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","server_version":version,"tables":[],"functions":[]}],"fileSchemas":{(uri.as_str()):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri.as_str()):profile},"completionPreferences":{"keywordCase":"upper"}}})).await;
        let sql = if matches!(profile, "hive" | "duckdb") {
            "SELECT TRUE IS "
        } else {
            "SELECT 1 IS "
        };
        p.open(&uri, sql).await;
        let result = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, sql, sql.len(), profile, "profile-constraint"),
            )
            .await;
        let keywords = result["constraints"]["allowedKeywords"].as_array().unwrap();
        assert_eq!(
            keywords.iter().any(|value| value == "TRUE"),
            truth,
            "{profile} {version:?}"
        );
        assert_eq!(
            keywords.iter().any(|value| value == "NOT DISTINCT FROM"),
            distinct,
            "{profile} {version:?}"
        );
        if profile == "hive" {
            assert!(!keywords.iter().any(|value| value == "UNKNOWN"));
        }
        p.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await;
    }
    p.stop().await;
}

#[tokio::test]
async fn truth_knowledge_separates_confirmed_boolean_nonboolean_and_unresolved_types() {
    let mut p = LspProcess::start().await;
    for (index, (expression, known)) in [
        ("CAST(TRUE AS BOOLEAN)", Some(true)),
        ("CAST(1 AS INTEGER)", Some(false)),
        ("TRIM('x')", None),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://constraint/truth-{index}.mysql.sql");
        configure(&mut p, &uri, "postgres", "truth-knowledge", "integer").await;
        let sql = format!("/*😀*/ SELECT {expression} IS ");
        p.open(&uri, &sql).await;
        let result = p
            .request(
                "oxide/completionSnapshot",
                params(&uri, &sql, sql.len(), "postgres", "truth-knowledge"),
            )
            .await;
        match known {
            Some(value) => assert_eq!(result["constraints"]["truthTests"], value),
            None => assert!(result["constraints"].get("truthTests").is_none()),
        }
        let local=p.request("oxide/documentCompletions",json!({"textDocument":{"uri":uri},"version":1,"dialect":"postgres","position":position(&sql,sql.len()),"preferences":{"keywordCase":"upper","functionCase":"upper","tableAlias":"none"}})).await;
        assert_eq!(local["constraints"], result["constraints"]);
    }
    p.stop().await;
}
