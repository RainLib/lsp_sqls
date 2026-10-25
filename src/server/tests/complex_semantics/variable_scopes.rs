use super::super::execute_values::{fixture, items};
use super::*;
use crate::{placeholder::SqlPlaceholderDialect, SqlParser};

#[tokio::test]
async fn variable_scopes_scalar_function_end_hides_parameters_and_body_locals() {
    for suffix in [
        "SELECT @se<cursor>;",
        "SELECT @in<cursor>;",
        "EXEC app.calculate @first=@se<cursor>;",
    ] {
        let marked=format!("CREATE FUNCTION app.f(@secret bigint) RETURNS bigint AS BEGIN DECLARE @inside bigint; RETURN @secret; END; {suffix}");
        let h = fixture(&marked, "sqlserver");
        let list = items(&h).await;
        assert!(
            !list
                .iter()
                .any(|i| matches!(i.label.as_str(), "@secret" | "@inside")),
            "{marked}: {list:?}"
        );
    }
}

#[tokio::test]
async fn variable_scopes_inline_function_end_and_tail_local_are_separate() {
    for body in [
        "(WITH p AS(SELECT @secret AS id) SELECT id FROM p)",
        "SELECT @secret AS id",
    ] {
        let marked=format!("CREATE FUNCTION app.f(@secret bigint) RETURNS TABLE AS RETURN {body}; SELECT @se<cursor>;");
        assert!(
            !items(&fixture(&marked, "mssql"))
                .await
                .iter()
                .any(|i| i.label == "@secret"),
            "{marked}"
        );
        let marked = marked.replace(
            "SELECT @se<cursor>;",
            "DECLARE @safe int; SELECT @sa<cursor>;",
        );
        let list = items(&fixture(&marked, "tsql")).await;
        assert!(
            list.iter().any(|i| i.label == "@safe"),
            "{marked}: {list:?}"
        );
    }
}

#[tokio::test]
async fn variable_scopes_expired_declarations_never_type_tail_projections() {
    for (definition,reference) in [
        ("CREATE FUNCTION app.f(@secret bigint) RETURNS bigint AS BEGIN RETURN @secret; END;","@secret"),
        ("CREATE FUNCTION app.f() RETURNS bigint AS BEGIN DECLARE @inside bigint; RETURN @inside; END;","@inside"),
        ("CREATE FUNCTION app.f(@secret bigint) RETURNS TABLE AS RETURN(SELECT @secret AS id);","@secret"),
    ] {
        let marked=format!("{definition} WITH p AS(SELECT {reference} AS value) SELECT p.va<cursor>lue FROM p;");
        let h=configured("sqlserver",&marked);let list=complete_harness(&h).await;
        assert!(list.iter().any(|i|i.label=="value"&&i.detail.as_deref()==Some("Column: value (unknown)")),"{marked}: {list:?}");
    }
}

#[tokio::test]
async fn variable_scopes_nested_blocks_case_and_function_body_keep_visible_names() {
    for (marked,name) in [
        ("CREATE FUNCTION app.f(@input bigint) RETURNS bigint AS BEGIN BEGIN DECLARE @local bigint=CASE WHEN 1=1 THEN @input ELSE 0 END; END; RETURN COALESCE(@in<cursor>,@local); END;","@input"),
        ("CREATE FUNCTION app.f(@input bigint) RETURNS TABLE AS RETURN(WITH p AS(SELECT CASE WHEN 1=1 THEN @input ELSE 0 END AS value) SELECT value FROM p WHERE value=@in<cursor>);","@input"),
        ("CREATE PROC app.p @input bigint AS BEGIN IF 0=1 BEGIN DECLARE @local bigint; END; SELECT CASE WHEN 1=1 THEN @lo<cursor> ELSE @input END; END; SELECT @input;","@local"),
    ] {
        let h=fixture(marked,"sqlserver");let list=items(&h).await;
        assert!(list.iter().any(|i|i.label==name),"{marked}: {list:?}");
        let mut parser=SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
        assert!(parser.parse(&h.sql).diagnostics.is_empty(),"{}",h.sql);
    }
}

#[tokio::test]
async fn variable_scopes_comments_partial_bodies_and_batch_edges_are_distinct() {
    for marked in [
        "CREATE FUNCTION app.f(@input nvarchar(max)=N'END; GO') RETURNS bigint AS BEGIN /* END */ RETURN @in<cursor>; END;",
        "CREATE FUNCTION app.f(@input bigint) RETURNS bigint AS BEGIN RETURN @in<cursor>",
        "CREATE PROC app.p @input bigint AS BEGIN SELECT @input; END; /* END */ SELECT @in<cursor>;",
    ] {
        let list=items(&fixture(marked,"sqlserver")).await;
        assert!(list.iter().any(|i|i.label=="@input"),"{marked}: {list:?}");
    }
    let marked="CREATE FUNCTION app.f(@secret bigint) RETURNS bigint AS BEGIN RETURN @secret; END;\r\nGO 2 -- batch\r\nDECLARE @input int; SELECT @in<cursor>;";
    let list = items(&fixture(marked, "sqlserver")).await;
    assert!(list.iter().any(|i| i.label == "@input"));
    assert!(!list.iter().any(|i| i.label == "@secret"));
}

#[tokio::test]
async fn variable_scopes_invalid_nonfirst_modules_do_not_inherit_outer_batch_variables() {
    for module in [
        "CREATE FUNCTION app.f(@input bigint) RETURNS bigint AS BEGIN RETURN @ou<cursor>; END;",
        "CREATE PROC app.p @input bigint AS BEGIN SELECT @ou<cursor>; END;",
    ] {
        let marked = format!("DECLARE @outer bigint; {module}");
        let h = fixture(&marked, "sqlserver");
        let list = items(&h).await;
        assert!(
            !list.iter().any(|i| i.label == "@outer"),
            "{marked}: {list:?}"
        );
        let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
        assert!(!parser.parse(&h.sql).diagnostics.is_empty(), "{}", h.sql);
    }
}

#[tokio::test]
async fn variable_scopes_end_edits_rebind_the_same_document_without_stale_names() {
    let closed="CREATE FUNCTION app.f(@secret bigint) RETURNS bigint AS BEGIN RETURN 1; END; SELECT @se<cursor>;";
    let mut h = configured("sqlserver", closed);
    for (version, marked, visible) in [
        (1, closed.to_owned(), false),
        (2, closed.replace("END; ", ""), true),
        (3, closed.to_owned(), false),
    ] {
        h.position = lsp_position_at_end(&marked[..marked.find("<cursor>").unwrap()]);
        h.service.inner().document_manager.update(
            h.uri.to_string(),
            marked.replace("<cursor>", ""),
            version,
        );
        let list = complete_harness(&h).await;
        assert_eq!(
            list.iter().any(|i| i.label == "@secret"),
            visible,
            "{marked}: {list:?}"
        );
    }
}

#[tokio::test]
async fn variable_scopes_module_boundary_precedes_the_retained_name_budget() {
    let prefix = (0..4097)
        .map(|i| format!("DECLARE @outer{i:04} int; "))
        .collect::<String>();
    let marked=format!("{prefix}CREATE FUNCTION app.f(@input bigint) RETURNS bigint AS BEGIN RETURN @outer0001<cursor>; END;");
    let list = items(&fixture(&marked, "sqlserver")).await;
    assert!(list.is_empty(), "{list:?}");
}

#[tokio::test]
async fn variable_scopes_opaque_module_keywords_never_reset_a_real_procedure_scope() {
    for noise in [
        "SELECT N'CREATE FUNCTION app.fake(@secret int) RETURNS int AS BEGIN END';",
        "/* ALTER PROC app.fake @secret int AS */ SELECT 1;",
        "SELECT [CREATE], [ALTER], [END];",
    ] {
        let marked =
            format!("CREATE PROC app.p @input bigint AS BEGIN {noise} SELECT @in<cursor>; END;");
        let list = items(&fixture(&marked, "sqlserver")).await;
        assert!(
            list.iter().any(|i| i.label == "@input"),
            "{marked}: {list:?}"
        );
        assert!(!list.iter().any(|i| i.label == "@secret"));
    }
}

#[test]
fn variable_scopes_many_statements_diagnostics_stay_responsive() {
    use std::time::{Duration, Instant};
    let mut source = (0..4097)
        .map(|n| format!("DECLARE @outer{n:04} int; "))
        .collect::<String>();
    source.push_str(
        "CREATE FUNCTION app.f(@input bigint) RETURNS bigint AS BEGIN RETURN @outer0001; END;",
    );
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
    let phase = Instant::now();
    assert!(parser.parse_syntax_tree(&source).is_some());
    eprintln!("syntax tree: {:?}", phase.elapsed());
    let phase = Instant::now();
    let parsed = parser.parse(&source);
    eprintln!("syntax diagnostics: {:?}", phase.elapsed());
    assert!(!parsed.diagnostics.is_empty());
    let schema = corpus_schema();
    let phase = Instant::now();
    let _inspections =
        crate::server::diagnostics::sql_inspection_diagnostics(&source, Some(&schema), "sqlserver");
    eprintln!("inspections: {:?}", phase.elapsed());
    assert!(
        phase.elapsed() < Duration::from_secs(5),
        "4097 small statements must not repeatedly scan their full source prefix"
    );
}

#[test]
fn variable_scopes_wrong_dialect_keeps_all_errors_without_rescanning_long_columns() {
    use std::time::{Duration, Instant};
    let source = (0..4097)
        .map(|n| format!("DECLARE @outer{n:04} int; "))
        .collect::<String>();
    let mut parser = SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::Mysql);
    let phase = Instant::now();
    let parsed = parser.parse(&source);
    eprintln!("wrong-dialect 4097 errors: {:?}", phase.elapsed());
    assert!(
        parsed.diagnostics.len() >= 4097,
        "{}",
        parsed.diagnostics.len()
    );
    assert!(phase.elapsed() < Duration::from_secs(5));
}
