use super::*;

async fn configure(process: &mut LspProcess, uri: &str, dialect: &str) {
    let id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    process.notify("workspace/didChangeConfiguration", json!({"settings":{
        "schemas":[{"id":id,"database":"app","tables":[{"name":"users","columns":[{"name":"id","data_type":"bigint","nullable":false},{"name":"name","data_type":"text","nullable":true}]}],"functions":[]}],
        "fileSchemas":{(uri):id},"fileDialects":{(uri):dialect},"completionPreferences":{"functionCase":"preserve"}
    }})).await;
}

fn expand_snippet(snippet: &str) -> String {
    let mut output = String::new();
    let mut rest = snippet;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let end = rest[start..].find('}').unwrap() + start;
        let placeholder = &rest[start + 2..end];
        let label = placeholder
            .split_once(':')
            .map(|(_, label)| label)
            .unwrap_or("");
        output.push_str(match label {
            "column" => "u.id",
            "lambda" => "(x,y)->x+y",
            "array" => "[1,2]",
            "arrays" => "[3,4]",
            _ => "0",
        });
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    output
}

#[tokio::test]
async fn native_routine_completion_edits_preserve_arrays_arguments_and_window_clauses() {
    let mut process = LspProcess::start().await;
    let cases = [
        (
            "postgres",
            "SELECT ARRAY[coa<cursor>lesce]",
            "COALESCE",
            3,
            false,
            true,
        ),
        (
            "postgres",
            "SELECT u.name ILIKE ANY(ARRAY[con<cursor>cat]) FROM users u",
            "CONCAT",
            3,
            false,
            true,
        ),
        (
            "postgres",
            "SELECT ARRAY[coa<cursor>lesce(NULL,1)]",
            "COALESCE",
            3,
            true,
            true,
        ),
        (
            "postgres",
            "SELECT 1\r\n\r\nSELECT ARRAY[coa<cursor>lesce /* c */ (NULL,1)]",
            "COALESCE",
            3,
            true,
            false,
        ),
        (
            "postgres",
            "SELECT ARRAY[row_n<cursor>umber] FROM users u",
            "ROW_NUMBER",
            5,
            false,
            true,
        ),
        (
            "postgres",
            "SELECT ARRAY[row_n<cursor>umber() OVER(ORDER BY u.id)] FROM users u",
            "ROW_NUMBER",
            5,
            true,
            true,
        ),
        (
            "clickhouse",
            "SELECT [arrayM<cursor>ap]",
            "arrayMap",
            6,
            false,
            true,
        ),
        (
            "clickhouse",
            "SELECT [toI<cursor>nt64('1')]",
            "toInt64",
            3,
            true,
            true,
        ),
        (
            "hive",
            "SELECT array(con<cursor>cat)",
            "CONCAT",
            3,
            false,
            true,
        ),
        (
            "mysql",
            "SELECT JSON_ARRAY(con<cursor>cat('x','y'))",
            "CONCAT",
            3,
            true,
            true,
        ),
        (
            "sqlserver",
            "SELECT con<cursor>cat('x','y')",
            "CONCAT",
            3,
            true,
            true,
        ),
        ("sqlite", "SELECT ab<cursor>s(-1)", "ABS", 2, true, true),
    ];
    assert_eq!(cases.len(), 12);
    for (index, (dialect, marked, name, prefix_len, existing, valid_sql)) in
        cases.into_iter().enumerate()
    {
        let uri = format!("oxide://query/routine-edit.{index}.mysql.sql");
        configure(&mut process, &uri, dialect).await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked}");
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let params = json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])});
        let before = process
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        let before_errors = before["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["severity"] == 1 || item["code"] == "PARSE_ERROR")
            .count();
        let response = process.request("textDocument/completion", params).await;
        let item = response
            .as_array()
            .or_else(|| response["items"].as_array())
            .unwrap()
            .iter()
            .find(|item| {
                item["label"]
                    .as_str()
                    .is_some_and(|label| label.eq_ignore_ascii_case(name))
            })
            .unwrap_or_else(|| panic!("{dialect}: {marked}: {response}"));
        let start = offset - prefix_len;
        let end = start + name.len();
        assert_eq!(
            item["textEdit"]["range"],
            json!({"start":sql_lsp::position::lsp_position_at_end(&sql[..start]),"end":sql_lsp::position::lsp_position_at_end(&sql[..end])}),
            "{marked}: {item}"
        );
        let replacement = item["textEdit"]["newText"].as_str().unwrap();
        if existing {
            assert!(replacement.eq_ignore_ascii_case(name));
            assert_eq!(item["insertTextFormat"], 1);
        } else {
            assert!(replacement.contains("${"));
            assert_eq!(item["insertTextFormat"], 2);
        }
        let mut accepted = sql.clone();
        accepted.replace_range(start..end, &expand_snippet(replacement));
        process.notify("textDocument/didChange", json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":accepted}]})).await;
        process
            .until(|message| {
                message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["uri"] == uri
                    && message["params"]["version"] == 2
            })
            .await;
        let diagnostics = process
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await;
        let errors = diagnostics["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["severity"] == 1 || item["code"] == "PARSE_ERROR")
            .count();
        if valid_sql {
            assert_eq!(errors, 0, "{dialect}: accepted {accepted}: {diagnostics}");
        } else {
            assert!(before_errors > 0);
            assert_eq!(
                errors, before_errors,
                "existing separator error must survive completion: {diagnostics}"
            );
        }
    }
    process.stop().await;
}

#[tokio::test]
async fn native_routine_signatures_and_literal_boundaries_use_configured_dialects() {
    let mut process = LspProcess::start().await;
    let signatures = [
        (
            "postgres",
            "SELECT ARRAY[COALESCE('x,y', <cursor>'fallback')]",
            "COALESCE",
            1,
        ),
        (
            "postgres",
            "SELECT COALESCE((1+<cursor>2),3)",
            "COALESCE",
            0,
        ),
        (
            "postgres",
            "SELECT CONCAT(ARRAY[1,2],<cursor>'tail')",
            "CONCAT",
            1,
        ),
        (
            "postgres",
            "SELECT COALESCE /* outer /* inner */ still */ (<cursor>NULL,1)",
            "COALESCE",
            0,
        ),
        ("clickhouse", "SELECT [toInt64(<cursor>'1')]", "toInt64", 0),
        (
            "hive",
            "SELECT array(concat('x', <cursor>'y'))",
            "CONCAT",
            1,
        ),
        (
            "mysql",
            "SELECT JSON_ARRAY(CONCAT('x',<cursor>'y'))",
            "CONCAT",
            1,
        ),
        ("sqlserver", "SELECT CONCAT('x',<cursor>'y')", "CONCAT", 1),
        ("sqlite", "SELECT ABS(<cursor>-1)", "ABS", 0),
    ];
    assert_eq!(signatures.len(), 9);
    for (index, (dialect, marked, name, parameter)) in signatures.into_iter().enumerate() {
        let uri = format!("oxide://query/routine-signature.{index}.mysql.sql");
        configure(&mut process, &uri, dialect).await;
        let marked = format!("/* 😀 */\r\n/* 😀 */ {marked}");
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let response = process.request("textDocument/signatureHelp", json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])})).await;
        assert!(
            response["signatures"][0]["label"]
                .as_str()
                .is_some_and(|label| label.to_lowercase().starts_with(&name.to_lowercase())),
            "{dialect}: {marked}: {response}"
        );
        assert_eq!(
            response["activeParameter"], parameter,
            "{dialect}: {marked}: {response}"
        );
    }
    let excluded = [
        ("postgres", "SELECT ARRAY['coa<cursor>']"),
        ("postgres", "SELECT ARRAY[$q$ coa<cursor> $q$]"),
        ("postgres", "SELECT ARRAY[/* coa<cursor> */1]"),
        ("clickhouse", "SELECT ['it\\'s coa<cursor>']"),
        ("clickhouse", "SELECT 1 // coa<cursor>"),
        ("clickhouse", "SELECT $tag$ coa<cursor> $tag$"),
        ("sqlserver", "SELECT [coa<cursor>] FROM users"),
        ("sqlite", "SELECT [coa<cursor>] FROM users"),
        ("hive", "SELECT array(\"coa<cursor>\")"),
    ];
    assert_eq!(excluded.len(), 9);
    for (index, (dialect, marked)) in excluded.into_iter().enumerate() {
        let uri = format!("oxide://query/routine-excluded.{index}.mysql.sql");
        configure(&mut process, &uri, dialect).await;
        let offset = marked.find("<cursor>").unwrap();
        let sql = marked.replace("<cursor>", "");
        process.open(&uri, &sql).await;
        let response = process.request("textDocument/completion", json!({"textDocument":{"uri":uri},"position":sql_lsp::position::lsp_position_at_end(&sql[..offset])})).await;
        assert!(
            response
                .as_array()
                .or_else(|| response["items"].as_array())
                .unwrap()
                .iter()
                .all(|item| item["kind"] != 3),
            "{dialect}: {marked}: {response}"
        );
    }
    process.stop().await;
}
