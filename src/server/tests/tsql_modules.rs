use super::execute_values::{accepted, fixture, items, response};
use crate::{placeholder::SqlPlaceholderDialect, SqlParser};
use tower_lsp::lsp_types::CompletionItemKind;

fn parse_ok(sql: &str, kind: &str) {
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
    let result = parser.parse(sql);
    assert!(
        result.diagnostics.is_empty(),
        "{sql}\n{:?}",
        result.diagnostics
    );
    let tree = result.tree.unwrap();
    assert!(
        !tree.root_node().has_error(),
        "{}",
        tree.root_node().to_sexp()
    );
    assert!(tree.root_node().to_sexp().contains(kind));
}

#[test]
fn tsql_modules_parse_procedure_headers_blocks_and_complex_bodies() {
    for sql in [
        "CREATE PROC app.p @x int AS SELECT @x; SELECT @x+1;",
        "CREATE PROC app.p @x int AS BEGIN SELECT @x; END; SELECT @x+1;",
    ] {
        parse_ok(sql, "tsql_procedure_body");
        let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
        let tree = parser.parse(sql).tree.unwrap();
        let statements = tree
            .root_node()
            .named_children(&mut tree.root_node().walk())
            .filter(|child| child.kind() != "comment")
            .count();
        assert_eq!(
            statements,
            1,
            "The whole batch belongs to the procedure: {}",
            tree.root_node().to_sexp()
        );
    }
    for prefix in [
        "CREATE PROC",
        "CREATE OR ALTER PROCEDURE",
        "ALTER PROCEDURE",
    ] {
        parse_ok(&format!("{prefix} [app].[例程] @金额 decimal(18,2)=1.25, @结果 nvarchar(max) OUTPUT WITH RECOMPILE AS BEGIN DECLARE @local int=COALESCE(1,2); BEGIN SELECT CASE WHEN @金额>0 THEN @金额 ELSE 0 END AS amount; END; EXEC app.calculate @first=@local; END;"), "tsql_procedure_definition");
    }
    parse_ok(
        "CREATE PROCEDURE app.p @id int AS SELECT @id AS id;",
        "tsql_procedure_definition",
    );
    parse_ok("CREATE PROCEDURE app.p AS BEGIN SET NOCOUNT ON; DECLARE @n int=1; WHILE @n<3 BEGIN SET @n=@n+1; END; IF @n>1 BEGIN SELECT @n; END ELSE BEGIN SELECT 0; END; RETURN 0; END;", "tsql_procedure_definition");
}

#[test]
fn tsql_modules_parse_scalar_and_inline_functions_with_real_parameter_nodes() {
    for prefix in [
        "CREATE FUNCTION",
        "CREATE OR ALTER FUNCTION",
        "ALTER FUNCTION",
    ] {
        parse_ok(&format!("{prefix} app.f(@输入$ AS decimal(18,2)=NULL, @label nvarchar(max)=N'a,b') RETURNS decimal(18,2) AS BEGIN DECLARE @local decimal(18,2)=COALESCE(@输入$,0); RETURN CASE WHEN @local>0 THEN @local ELSE 0 END; END;"), "tsql_function_definition");
    }
    parse_ok("CREATE FUNCTION app.f(@id int) RETURNS TABLE AS RETURN (WITH c AS (SELECT @id AS id) SELECT c.id FROM c WHERE c.id>0);", "tsql_function_definition");
    parse_ok(
        "CREATE FUNCTION app.f() RETURNS int AS BEGIN RETURN 1; END;",
        "tsql_function_definition",
    );
}

#[tokio::test]
async fn tsql_modules_procedure_parameters_feed_exec_values_with_types_and_local_names() {
    for prefix in [
        "CREATE PROC",
        "CREATE OR ALTER PROCEDURE",
        "ALTER PROCEDURE",
    ] {
        let h=fixture(&format!("/*😀*/ {prefix} [app].[owner] @输入$ decimal(18,2)=1, @输出# int OUTPUT AS BEGIN DECLARE @local int=1;\r\nEXEC app.calculate @first=@<cursor>; END;"), "sqlserver");
        let result = items(&h).await;
        for (name, ty) in [
            ("@输入$", "decimal(18,2)"),
            ("@输出#", "int"),
            ("@local", "int"),
        ] {
            let item = result
                .iter()
                .find(|item| item.label == name)
                .unwrap_or_else(|| panic!("missing {name}: {result:?}"));
            assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
            assert!(item.detail.as_deref().unwrap().contains(ty));
            assert!(accepted(&h, item).contains(&format!("@first={name};")));
        }
    }
}

#[tokio::test]
async fn tsql_modules_expression_sites_offer_parameters_and_batch_locals() {
    for statement in [
        "SELECT @in<cursor> AS value;",
        "SELECT CASE WHEN 1=1 THEN @in<cursor> ELSE 0 END;",
        "WITH c AS (SELECT 1 AS id) SELECT c.id FROM c WHERE c.id=@in<cursor>;",
        "SELECT COALESCE(@in<cursor>,1);",
        "SELECT 1 WHERE EXISTS(SELECT 1 WHERE 1=@in<cursor>);",
        "RETURN @in<cursor>;",
    ] {
        let h = fixture(
            &format!("CREATE PROC app.p @input int AS BEGIN {statement} END;"),
            "tsql",
        );
        let result = items(&h).await;
        let item = result
            .iter()
            .find(|item| item.label == "@input")
            .unwrap_or_else(|| panic!("{statement}: {result:?}"));
        assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
        assert!(accepted(&h, item).contains("@input"));
    }
    let h = fixture("DECLARE @input int; SELECT @in<cursor>put;", "mssql");
    let result = items(&h).await;
    let item = result
        .iter()
        .find(|item| item.label == "@input")
        .expect("batch local");
    assert_eq!(accepted(&h, item), "DECLARE @input int; SELECT @input;");
}

#[tokio::test]
async fn tsql_modules_parameters_are_batch_and_definition_scoped() {
    for marked in [
        "CREATE PROC app.p @secret int AS BEGIN SELECT @secret; END;\nGO\nEXEC app.calculate @first=@se<cursor>;",
        "CREATE PROC app.p @secret int AS BEGIN SELECT @secret; END;\nGO 2 -- batch\nCREATE PROC app.q @second int AS BEGIN EXEC app.calculate @first=@se<cursor>; END;",
        "CREATE FUNCTION app.f(@secret int) RETURNS int AS BEGIN RETURN @secret; END;\nGO\nSELECT @se<cursor>;",
        "/* CREATE PROC app.fake @secret int AS */ DECLARE @safe int; EXEC app.calculate @first=@se<cursor>;",
        "CREATE PROC app.p @safe int AS BEGIN SELECT N'DECLARE @secret int;'; EXEC app.calculate @first=@se<cursor>; END;",
        "CREATE PROC app.p @safe int AS BEGIN EXEC app.calculate @first=@fu<cursor>; DECLARE @future int; END;",
    ] {
        let h=fixture(marked,"sqlserver");
        let result=items(&h).await;
        assert!(!result.iter().any(|item|matches!(item.label.as_str(),"@secret"|"@future")),"{marked}: {result:?}");
    }
    let h=fixture("CREATE PROC app.p @firstLocal int AS BEGIN SELECT @firstLocal; END;\nGO\nCREATE PROC app.q @secondLocal int AS BEGIN EXEC app.calculate @first=@se<cursor>; END;","sqlserver");
    let result = items(&h).await;
    assert!(result.iter().any(|item| item.label == "@secondLocal"));
}

#[test]
fn tsql_modules_invalid_headers_values_and_foreign_profiles_keep_diagnostics() {
    for sql in [
        "CREATE PROC app.p plain int AS BEGIN SELECT 1; END;",
        "CREATE PROC app.p @x int=COALESCE(1,2) AS BEGIN SELECT 1; END;",
        "CREATE FUNCTION app.f(@x int OUTPUT) RETURNS int AS BEGIN RETURN @x; END;",
        "CREATE FUNCTION app.f(@x int) RETURNS int AS BEGIN RETURN; END;",
        "CREATE PROC app.p @x int AS BEGIN SELECT @x;",
        "CREATE FUNCTION app.f(@x int) RETURNS int AS $$SELECT 1$$;",
        "CREATE FUNCTION app.f(@x int) RETURNS int AS 'SELECT 1';",
    ] {
        let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
        assert!(!parser.parse(sql).diagnostics.is_empty(), "{sql}");
    }
    for dialect in [
        SqlPlaceholderDialect::Postgres,
        SqlPlaceholderDialect::Mysql,
    ] {
        let mut parser = SqlParser::new_with_placeholder_dialect(dialect);
        assert!(!parser
            .parse("CREATE PROC app.p @x int AS BEGIN SELECT @x; END;")
            .diagnostics
            .is_empty());
    }
}

#[tokio::test]
async fn tsql_modules_large_parameter_lists_keep_late_exact_matches_and_duplicates_unknown() {
    let parameters = (0..2100)
        .map(|n| format!("@arg{n:04} int"))
        .collect::<Vec<_>>()
        .join(",");
    let h=fixture(&format!("CREATE PROC app.p {parameters} AS BEGIN EXEC app.calculate @first=@arg2099<cursor>; END;"),"sqlserver");
    let result = items(&h).await;
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0].label, "@arg2099");
    parse_ok(&h.sql, "tsql_procedure_definition");
    let h=fixture("CREATE PROC app.p @dup int AS BEGIN DECLARE @dup nvarchar(20); EXEC app.calculate @first=@du<cursor>; END;","sqlserver");
    assert!(!items(&h).await.iter().any(|item| item.label == "@dup"));
}

#[tokio::test]
async fn tsql_modules_same_line_control_flow_and_unterminated_exec_values_keep_parameters() {
    for body in [
        "BEGIN EXEC app.calculate @first=@in<cursor> END",
        "BEGIN IF 1=1 EXEC app.calculate @first=@in<cursor>; ELSE SELECT 0; END",
        "BEGIN WHILE 1=0 BEGIN EXEC app.calculate @first=@in<cursor> END END",
    ] {
        let h = fixture(
            &format!("CREATE PROC app.p @input int AS {body};"),
            "sqlserver",
        );
        let result = items(&h).await;
        let item = result
            .iter()
            .find(|item| item.label == "@input")
            .unwrap_or_else(|| panic!("{body}: {result:?}"));
        parse_ok(&accepted(&h, item), "tsql_procedure_definition");
    }
    for marked in ["CREATE FUNCTION app.f(@input int) RETURNS int AS BEGIN RETURN @in<cursor>; END;", "CREATE FUNCTION app.f(@input int) RETURNS int AS BEGIN RETURN COALESCE(@in<cursor>,0); END;", "CREATE FUNCTION app.f(@input int) RETURNS TABLE AS RETURN (SELECT @in<cursor> AS id);"] {
        let h=fixture(marked,"mssql");let result=items(&h).await;
        let item=result.iter().find(|item|item.label=="@input").unwrap_or_else(||panic!("{marked}: {result:?}"));
        parse_ok(&accepted(&h,item),"tsql_function_definition");
    }
}

#[tokio::test]
async fn tsql_modules_exclude_declaration_names_relations_headers_and_readonly_values() {
    for marked in [
        "DECLARE @input int; DECLARE @in<cursor> int;",
        "DECLARE @input int; DECLARE @local int,@in<cursor> int;",
        "DECLARE @input int; SELECT * FROM @in<cursor>;",
        "DECLARE @input int; SELECT t.@in<cursor> FROM app.users t;",
        "DECLARE @input int; SELECT 1 AS @in<cursor>;",
        "DECLARE @input TABLE(id int); SELECT @in<cursor>;",
        "DECLARE @input CURSOR; SELECT @in<cursor>;",
        "CREATE PROC app.p @input int, @in<cursor> int AS BEGIN SELECT 1; END;",
        "CREATE PROC app.p @input app.InputTable READONLY AS BEGIN SELECT @in<cursor>; END;",
    ] {
        let h = fixture(marked, "sqlserver");
        let result = items(&h).await;
        assert!(
            !result.iter().any(|item| item.label == "@input"),
            "{marked}: {result:?}"
        );
    }
    for profile in ["postgres", "mysql"] {
        let h = fixture("DECLARE @input int; SELECT @in<cursor>;", profile);
        assert!(!items(&h).await.iter().any(|item| item.label == "@input"));
    }
}

#[tokio::test]
async fn tsql_modules_parameter_caps_and_expression_lists_signal_requery() {
    let parameters = (0..2100)
        .map(|n| format!("@arg{n:04} int"))
        .collect::<Vec<_>>()
        .join(",");
    let h = fixture(
        &format!("CREATE PROC app.p {parameters} AS BEGIN SELECT @arg<cursor>; END;"),
        "sqlserver",
    );
    let tower_lsp::lsp_types::CompletionResponse::List(list) = response(&h).await else {
        panic!("overflow must be a list");
    };
    assert!(list.is_incomplete);
    assert_eq!(list.items.len(), 100);
    let h = fixture(
        &format!(
            "CREATE PROC app.p {parameters},@overflow int AS BEGIN SELECT @arg2099<cursor>; END;"
        ),
        "sqlserver",
    );
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
    assert!(parser.parse(&h.sql).diagnostics.iter().any(|item| item.code
        == Some(tower_lsp::lsp_types::NumberOrString::String(
            "TOO_MANY_TSQL_PARAMETERS".into()
        ))));
    assert!(items(&h).await.is_empty());
}

#[test]
fn tsql_modules_batches_options_and_postgres_functions_remain_distinct() {
    parse_ok("CREATE PROC app.p @x int AS BEGIN SELECT @x; END\r\nGO 2 -- batch\r\nCREATE OR ALTER FUNCTION app.f(@x int) RETURNS int WITH SCHEMABINDING AS BEGIN RETURN @x; END\nGO\nSELECT 1;","tsql_procedure_definition");
    parse_ok(
        "CREATE PROC app.p @x int WITH EXECUTE AS OWNER,RECOMPILE AS BEGIN SELECT @x; END;",
        "tsql_procedure_definition",
    );
    for sql in [
        "SELECT 1; CREATE PROC app.p AS BEGIN SELECT 1; END;",
        "GO 0\nSELECT 1;",
        "SELECT 1; GO\nSELECT 2;",
        "CREATE FUNCTION app.f(@x int) RETURNS int WITH RECOMPILE AS BEGIN RETURN @x; END;",
        "CREATE PROC app.p @x app.InputTable READONLY OUTPUT AS BEGIN SELECT 1; END;",
    ] {
        let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
        assert!(!parser.parse(sql).diagnostics.is_empty(), "{sql}");
    }
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Postgres);
    let sql="CREATE FUNCTION app.f(input integer) RETURNS integer AS $$ BEGIN RETURN input; END; $$ LANGUAGE plpgsql;";
    let result = parser.parse(sql);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(result
        .tree
        .unwrap()
        .root_node()
        .to_sexp()
        .contains("create_function"));
}

#[tokio::test]
async fn tsql_modules_defaults_share_dialect_rules_and_keep_opaque_text_out_of_names() {
    let marked="CREATE PROC app.p @input nvarchar(max)=N'AS, GO '' DECLARE @secret int; ',@other int=-1 AS BEGIN SELECT @in<cursor>; END;";
    let h = fixture(marked, "sqlserver");
    parse_ok(&h.sql, "tsql_procedure_definition");
    assert!(items(&h).await.iter().any(|item| item.label == "@input"));
    for default in [
        "E'bad'",
        "$q$bad$q$",
        "DATE'2026-01-01'",
        "TRUE",
        "FALSE",
        "1abc",
    ] {
        let h = fixture(
            &format!(
                "CREATE PROC app.p @input nvarchar(30)={default} AS BEGIN SELECT @in<cursor>; END;"
            ),
            "sqlserver",
        );
        let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
        assert!(!parser.parse(&h.sql).diagnostics.is_empty(), "{default}");
        assert!(
            !items(&h).await.iter().any(|item| item.label == "@input"),
            "{default}"
        );
    }
}
