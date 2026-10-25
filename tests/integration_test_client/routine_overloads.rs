use super::*;

fn function(route: &str, ty: &str) -> Value {
    json!({"name":"pick","parameters":[{"name":route,"data_type":ty,"optional":false}],"return_type":"integer","description":null})
}

async fn configure(
    process: &mut LspProcess,
    uri: &str,
    dialect: &str,
    wrong: &str,
    wanted: &str,
    column: &str,
    include_wanted: bool,
) {
    let id = "99999999-9999-4999-8999-999999999999";
    let mut functions = vec![function("original", wrong)];
    if include_wanted {
        functions.push(function("wanted", wanted));
    }
    process.notify("workspace/didChangeConfiguration",json!({"settings":{
        "schemas":[{"id":id,"catalog":"db","database":"app","tables":[{"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"value","data_type":column,"nullable":true}]}],"functions":functions}],
        "fileSchemas":{(uri):id},"fileDialects":{(uri):dialect}
    }})).await;
}

#[tokio::test]
async fn native_routine_overloads_rank_complex_argument_types_without_dropping_alternatives() {
    let mut process = LspProcess::start().await;
    let cases=[
        ("postgres","SELECT app.pick(1<cursor>)","text","integer"),
        ("postgres","SELECT app.pick(2147483648<cursor>)","int4","int8"),
        ("postgres","SELECT app.pick(TRUE<cursor>)","integer","boolean"),
        ("postgres","SELECT app.pick(1.25<cursor>)","integer","numeric"),
        ("postgres","SELECT app.pick(CAST('x' AS text)<cursor>)","integer","text"),
        ("postgres","SELECT app.pick((COALESCE(NULL,1)+2)<cursor>)","text","int4"),
        ("postgres","SELECT app.pick(CASE WHEN TRUE THEN 2 ELSE 3 END<cursor>)","text","integer"),
        ("postgres","SELECT app.pick(ARRAY[1,2]::integer[]<cursor>)","integer","int4[]"),
        ("postgres","SELECT app.pick(u.id<cursor>) FROM users u","text","bigint"),
        ("postgres","WITH p AS (SELECT id AS v FROM users) SELECT app.pick(p.v<cursor>) FROM p","text","bigint"),
        ("postgres","SELECT app.pick(p.v<cursor>) FROM (SELECT id AS v FROM users) p","text","bigint"),
        ("postgres","SELECT app.pick((SELECT MAX(id) FROM users)<cursor>)","text","bigint"),
        ("postgres","SELECT 1 FROM users u WHERE EXISTS(SELECT app.pick(u.id<cursor>))","text","bigint"),
        ("mysql","SELECT app.pick(1<cursor>)","text","bigint"),
        ("sqlserver","SELECT app.pick(1<cursor>)","bigint","int"),
        ("sqlserver","SELECT app.pick(CAST(1 AS bigint)<cursor>)","int","bigint"),
        ("postgresql","SELECT app.pick(CAST(1 AS integer)<cursor>)","text","int4"),
        ("postgresql","WITH p AS (SELECT CASE WHEN TRUE THEN 1 ELSE 2 END AS v) SELECT app.pick(p.v<cursor>) FROM p","text","integer"),
    ];
    assert_eq!(cases.len(), 18);
    for (index, (dialect, marked, wrong, wanted)) in cases.into_iter().enumerate() {
        let uri = format!("oxide://routine/typed.{index}.mysql.sql");
        configure(&mut process, &uri, dialect, wrong, wanted, "bigint", true).await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked};");
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let result=process.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])})).await;
        assert_eq!(
            result["signatures"].as_array().unwrap().len(),
            2,
            "{dialect}: {sql}: {result}"
        );
        assert_eq!(
            result["signatures"][0]["label"],
            format!("pick(wanted {wanted})"),
            "{dialect}: {sql}: {result}"
        );
        assert_eq!(result["activeParameter"], 0);
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
                .all(|item| item["severity"] != 1),
            "{dialect}: {sql}: {diagnostics}"
        );
    }
    process.stop().await;
}

#[tokio::test]
async fn native_routine_overloads_refresh_field_type_rank_with_unchanged_document() {
    let mut process = LspProcess::start().await;
    let uri = "oxide://routine/typed-refresh.mysql.sql";
    let marked = "/* 😀 */\r\nSELECT app.pick(u.value<cursor>) FROM users u;";
    let offset = marked.find("<cursor>").unwrap();
    let sql = marked.replace("<cursor>", "");
    let rounds = [
        ("bigint", true, true),
        ("text", true, false),
        ("custom_domain", true, false),
        ("int8", true, true),
        ("bigint", false, false),
        ("bigint", true, true),
    ];
    assert_eq!(rounds.len(), 6);
    for (round, (column, include_wanted, wanted)) in rounds.into_iter().enumerate() {
        configure(
            &mut process,
            uri,
            "postgres",
            "text",
            "bigint",
            column,
            include_wanted,
        )
        .await;
        if round == 0 {
            process.open(uri, &sql).await;
        }
        let result=process.request("textDocument/signatureHelp",json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])})).await;
        let route = if wanted {
            "pick(wanted bigint)"
        } else {
            "pick(original text)"
        };
        assert_eq!(
            result["signatures"][0]["label"], route,
            "round {round}: {result}"
        );
        assert_eq!(
            result["signatures"].as_array().unwrap().len(),
            if include_wanted { 2 } else { 1 }
        );
    }
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
            .all(|item| item["severity"] != 1),
        "{diagnostics}"
    );
    process.stop().await;
}
