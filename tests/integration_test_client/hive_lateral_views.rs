use super::*;
use super::{
    execute_values::{complete, offset},
    joined_tables::{diagnostics, position},
};

async fn configure(p: &mut LspProcess, uri: &str, profile: &str, tags: &str) {
    p.notify("workspace/didChangeConfiguration",json!({"settings":{
        "schemas":[{"id":"aaaaaaaa-1111-4111-8111-111111111111","database":"app","tables":[
            {"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false}]},
            {"name":"orders","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"tags","data_type":tags,"nullable":false},{"name":"grid","data_type":"array<array<decimal(18,2)>>","nullable":false},{"name":"props","data_type":"map<string,array<bigint>>","nullable":false},{"name":"records","data_type":"array<struct<code:string,amount:decimal(18,2)>>","nullable":false},{"name":"payload","data_type":"string","nullable":false}]}
        ],"functions":[]}],"fileSchemas":{(uri):"aaaaaaaa-1111-4111-8111-111111111111"},"fileDialects":{(uri):profile}
    }})).await;
}

#[tokio::test]
async fn native_hive_lateral_views_accept_typed_outputs_ctes_and_unicode() {
    let mut p = LspProcess::start().await;
    for(index,(marked,name,ty))in [
        ("SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(o.tags) e AS item","item","string"),
        ("SELECT e.po<cursor>s FROM orders o LATERAL VIEW posexplode(o.tags) e","pos","int"),
        ("SELECT e.va<cursor>lue FROM orders o LATERAL VIEW explode(o.props) e","value","array<bigint>"),
        ("SELECT e.am<cursor>ount FROM orders o LATERAL VIEW inline(o.records) e","amount","decimal(18,2)"),
        ("SELECT e.na<cursor>me FROM orders o LATERAL VIEW json_tuple(o.payload,'name') e AS name","name","string"),
        ("SELECT e.co<cursor>l FROM orders LATERAL VIEW explode(array('a','b')) e","col","string"),
        ("SELECT e.co<cursor>l1 FROM orders LATERAL VIEW stack(2,'A',1,'B',2) e","col1","int"),
        ("SELECT b.it<cursor>em FROM orders o LATERAL VIEW explode(o.grid) a AS nested_col LATERAL VIEW explode(a.nested_col) b AS item","item","decimal(18,2)"),
        ("WITH r AS(SELECT e.* FROM orders o LATERAL VIEW explode(o.tags) e) SELECT r.co<cursor>l FROM r","col","string"),
        ("SELECT `e.x`.`项<cursor>目` FROM orders o LATERAL VIEW explode(o.tags) `e.x` AS `项目`","项目","string"),
        ("SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(CASE WHEN o.id>0 THEN o.tags ELSE o.tags END) e AS item","item","string"),
    ].into_iter().enumerate() {
        let uri=format!("oxide://hive-views/{index}.postgres.sql");configure(&mut p,&uri,"hive","array<string>").await;
        let marked=format!("/* 😀 中文 */\r\n{marked};");let at=marked.find("<cursor>").unwrap();let sql=marked.replace("<cursor>","");p.open(&uri,&sql).await;
        let items=complete(&mut p,&uri,position(&sql,at)).await;
        let item=items.iter().find(|i|i["label"]==name&&i["kind"]==5).unwrap_or_else(||panic!("{sql}: {items:?}"));
        assert!(item["detail"].as_str().unwrap().contains(&format!("({ty})")),"{item}");
        let edit=&item["textEdit"];let from=offset(&sql,&edit["range"]["start"]);let to=offset(&sql,&edit["range"]["end"]);assert!(sql[from..to].contains(name));
        let mut accepted=sql.clone();accepted.replace_range(from..to,edit["newText"].as_str().unwrap());assert_eq!(accepted,sql);
        p.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":accepted}]})).await;
        p.until(|m|m["method"]=="textDocument/publishDiagnostics"&&m["params"]["uri"]==uri&&m["params"]["version"]==2).await;
        let hover=p.request("textDocument/hover",json!({"textDocument":{"uri":uri},"position":position(&sql,at)})).await;assert!(hover.to_string().contains(ty),"{hover}");
        assert!(!diagnostics(&mut p,&uri).await.iter().any(|d|d["severity"]==1),"{accepted}");
    }
    p.stop().await;
}

#[tokio::test]
async fn native_hive_lateral_views_refresh_outer_and_argument_scope_without_leaks() {
    let mut p = LspProcess::start().await;
    let uri = "oxide://hive-views/refresh.sql";
    let marked = "SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(o.tags) e AS item";
    let at = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    configure(&mut p, uri, "hive", "array<string>").await;
    p.open(uri, &sql).await;
    for (ty, element) in [
        ("array<string>", "string"),
        ("array<bigint>", "bigint"),
        ("array<decimal(18,2)>", "decimal(18,2)"),
    ] {
        configure(&mut p, uri, "hive", ty).await;
        let items = complete(&mut p, uri, position(&sql, at)).await;
        assert!(items.iter().any(|i| i["label"] == "item"
            && i["detail"]
                .as_str()
                .is_some_and(|d| d.contains(&format!("({element})")))));
    }
    for (index, (outer, nullable)) in [("", false), ("OUTER ", true)].into_iter().enumerate() {
        let uri = format!("oxide://hive-views/outer-{index}.sql");
        configure(&mut p, &uri, "hive", "array<string>").await;
        let sql = format!("SELECT e.pos FROM orders o LATERAL VIEW {outer}posexplode(o.tags) e");
        p.open(&uri, &sql).await;
        let hover=p.request("textDocument/hover",json!({"textDocument":{"uri":uri},"position":position(&sql,sql.find("e.pos").unwrap()+3)})).await;
        assert!(
            hover.to_string().contains(if nullable {
                "Nullable: yes"
            } else {
                "Nullable: no"
            }),
            "{hover}"
        );
    }
    for(index,marked)in[
        "SELECT * FROM orders o LATERAL VIEW explode(b.it<cursor>em) a AS value LATERAL VIEW explode(o.tags) b AS item",
        "SELECT * FROM orders o LATERAL VIEW explode(a.it<cursor>em) a AS item",
    ].into_iter().enumerate(){let uri=format!("oxide://hive-views/scope-{index}.sql");configure(&mut p,&uri,"hive","array<string>").await;let at=marked.find("<cursor>").unwrap();let sql=marked.replace("<cursor>","");p.open(&uri,&sql).await;assert!(!complete(&mut p,&uri,position(&sql,at)).await.iter().any(|i|i["kind"]==5));}
    for profile in ["mysql", "postgres", "sqlserver", "sqlite", "clickhouse"] {
        configure(&mut p, uri, profile, "array<string>").await;
        assert!(diagnostics(&mut p, uri)
            .await
            .iter()
            .any(|d| d["code"] == "UNSUPPORTED_DIALECT_SYNTAX"));
        assert!(!complete(&mut p, uri, position(&sql, at))
            .await
            .iter()
            .any(|i| i["label"] == "item" && i["kind"] == 5));
    }
    configure(&mut p, uri, "hive", "array<string>").await;
    assert!(complete(&mut p, uri, position(&sql, at))
        .await
        .iter()
        .any(|i| i["label"] == "item"
            && i["detail"].as_str().is_some_and(|d| d.contains("(string)"))));
    p.stop().await;
}
