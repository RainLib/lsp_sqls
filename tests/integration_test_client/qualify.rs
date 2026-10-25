use super::*;
use super::{
    execute_values::{complete, offset},
    joined_tables::{configure, diagnostics, position},
};

#[tokio::test]
async fn native_qualify_typed_outputs_accept_full_edits_and_refresh_without_sql_changes() {
    let mut p = LspProcess::start().await;
    let cases=[
        ("SELECT row_number() OVER(ORDER BY total) AS rn FROM orders QUALIFY r<cursor>n=1","rn","UInt64"),
        ("SELECT rank() OVER(ORDER BY total) AS ranking FROM orders QUALIFY rank<cursor>ing=1","ranking","UInt64"),
        ("SELECT dense_rank() OVER(ORDER BY total) AS ranking FROM orders QUALIFY rank<cursor>ing=1","ranking","UInt64"),
        ("SELECT count() OVER(PARTITION BY id) AS partition_count FROM orders QUALIFY partition_<cursor>count=1","partition_count","UInt64"),
        ("SELECT row_number() OVER w AS rn FROM orders WINDOW w AS (ORDER BY total) QUALIFY r<cursor>n=1","rn","UInt64"),
        ("WITH c AS(SELECT total AS value,id FROM orders) SELECT c.value AS amount,row_number() OVER(ORDER BY c.id) AS rn FROM c JOIN users u ON c.id=u.id QUALIFY rn=1 AND amo<cursor>unt>0","amount","decimal"),
        ("SELECT row_number() OVER() AS `排名` FROM orders QUALIFY `排<cursor>名`=1","排名","UInt64"),
        ("SELECT row_number() OVER() AS outer_rank FROM orders QUALIFY EXISTS(SELECT row_number() OVER() AS inner_rank FROM users QUALIFY inner_<cursor>rank=1)","inner_rank","UInt64"),
    ];
    for (index, (marked, name, ty)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://qualify/{index}.postgres.sql");
        configure(&mut p, &uri, "clickhouse", "decimal").await;
        let marked = format!("/* 😀 中文 */\r\n{marked};");
        let at = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        p.open(&uri, &sql).await;
        let items = complete(&mut p, &uri, position(&sql, at)).await;
        let item = items
            .iter()
            .find(|i| i["label"] == name && i["kind"] == 5)
            .unwrap_or_else(|| panic!("{sql}: {items:?}"));
        assert!(
            item["detail"]
                .as_str()
                .unwrap()
                .contains(&format!("({ty})")),
            "{item}"
        );
        if index == 7 {
            assert!(!items.iter().any(|i| i["label"] == "outer_rank"));
        }
        let edit = &item["textEdit"];
        let from = offset(&sql, &edit["range"]["start"]);
        let to = offset(&sql, &edit["range"]["end"]);
        assert_eq!(edit["range"]["start"]["line"], 1);
        assert!(sql[from..to].contains(name));
        let mut accepted = sql.clone();
        accepted.replace_range(from..to, edit["newText"].as_str().unwrap());
        if index != 6 {
            assert_eq!(accepted, sql);
        }
        p.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":accepted}]}),
        )
        .await;
        p.until(|m| {
            m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"] == uri
                && m["params"]["version"] == 2
        })
        .await;
        assert!(
            !diagnostics(&mut p, &uri)
                .await
                .iter()
                .any(|d| d["severity"] == 1),
            "{accepted}"
        );
    }
    let uri = "oxide://qualify/metadata-refresh.sql";
    let marked="SELECT total AS amount,row_number() OVER() AS rn FROM orders QUALIFY amo<cursor>unt>0 AND rn=1";
    let at = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    configure(&mut p, uri, "clickhouse", "decimal").await;
    p.open(uri, &sql).await;
    for ty in ["decimal", "UInt32", "String"] {
        configure(&mut p, uri, "clickhouse", ty).await;
        let items = complete(&mut p, uri, position(&sql, at)).await;
        assert!(
            items.iter().any(|i| i["label"] == "amount"
                && i["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains(&format!("({ty})")))),
            "{items:?}"
        );
    }
    p.stop().await;
}

#[tokio::test]
async fn native_qualify_profiles_and_current_query_window_are_validated() {
    let mut p = LspProcess::start().await;
    for (index, (profile, body, expected)) in [
        (
            "clickhouse",
            "SELECT total AS amount FROM orders QUALIFY amount>0",
            "QUALIFY_REQUIRES_WINDOW",
        ),
        (
            "clickhouse",
            "SELECT (SELECT row_number() OVER() FROM users) AS rn FROM orders QUALIFY rn=1",
            "QUALIFY_REQUIRES_WINDOW",
        ),
        (
            "postgres",
            "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1",
            "UNSUPPORTED_DIALECT_SYNTAX",
        ),
        (
            "mysql",
            "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1",
            "UNSUPPORTED_DIALECT_SYNTAX",
        ),
        (
            "sqlserver",
            "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1",
            "UNSUPPORTED_DIALECT_SYNTAX",
        ),
        (
            "sqlite",
            "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1",
            "UNSUPPORTED_DIALECT_SYNTAX",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("oxide://qualify/rejected-{index}.sql");
        configure(&mut p, &uri, profile, "decimal").await;
        let sql = format!("/* 😀 */\r\n{body};");
        p.open(&uri, &sql).await;
        let diagnostics = diagnostics(&mut p, &uri).await;
        let d = diagnostics
            .iter()
            .find(|d| d["code"] == expected)
            .unwrap_or_else(|| panic!("{profile}: {diagnostics:?}"));
        assert_eq!(d["severity"], 1);
        assert_eq!(
            d["range"]["start"],
            position(&sql, sql.find("QUALIFY").unwrap())
        );
        assert_eq!(
            d["range"]["end"],
            position(&sql, sql.find("QUALIFY").unwrap() + 7)
        );
    }
    p.stop().await;
}
