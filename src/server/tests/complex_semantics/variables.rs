use super::*;
use crate::SqlParser;

async fn typed(marked: &str, ty: &str) {
    let h = configured("sqlserver", marked);
    if ty != "unknown" {
        let sql = h
            .service
            .inner()
            .document_manager
            .get(h.uri.as_str())
            .unwrap();
        let mut parser = SqlParser::new_with_placeholder_dialect(
            crate::placeholder::SqlPlaceholderDialect::SqlServer,
        );
        let parsed = parser.parse(&sql);
        assert!(
            parsed.diagnostics.is_empty(),
            "{sql}: {:?}",
            parsed.diagnostics
        );
        assert!(!parsed.tree.unwrap().root_node().has_error(), "{sql}");
    }
    let items = complete_harness(&h).await;
    let item = items
        .iter()
        .find(|item| item.label == "value")
        .unwrap_or_else(|| panic!("{marked}: {items:?}"));
    assert!(
        item.detail
            .as_deref()
            .unwrap_or_default()
            .contains(&format!("({ty})")),
        "{marked}: {item:?}"
    );
    assert!(
        serde_json::to_string(&item.documentation)
            .unwrap()
            .contains("Nullable: yes"),
        "{item:?}"
    );
    let hover = h
        .service
        .inner()
        .hover(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&hover).unwrap().contains(ty),
        "{marked}: {hover:?}"
    );
}
#[tokio::test]
async fn variable_types_propagate_through_ctes_derived_scalar_queries_and_sets() {
    for body in [
        "WITH p AS (SELECT @input AS value) SELECT p.va<cursor>lue FROM p;",
        "SELECT d.va<cursor>lue FROM (SELECT @input AS value) d;",
        "WITH p AS (SELECT (SELECT @input) AS value) SELECT p.va<cursor>lue FROM p;",
        "WITH p AS (SELECT @input AS value),q AS (SELECT value FROM p) SELECT q.va<cursor>lue FROM q;",
        "WITH p AS (SELECT @input AS value UNION ALL SELECT CAST(1 AS bigint)) SELECT p.va<cursor>lue FROM p;",
    ] {typed(&format!("/*😀*/ DECLARE @input bigint;\r\n{body}"),"bigint").await;}
}
#[tokio::test]
async fn variable_types_module_parameters_survive_statement_masks_and_unicode_offsets() {
    for marked in [
        "CREATE PROC app.p @input bigint AS BEGIN DECLARE @other int; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p; END;",
        "CREATE OR ALTER FUNCTION app.f(@input bigint) RETURNS TABLE AS RETURN (WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p);",
        "/*😀*/ DECLARE @金额$ bigint;\r\nWITH p AS(SELECT @金额$ AS value) SELECT p.va<cursor>lue FROM p;",
    ] {typed(marked,"bigint").await;}
}
#[tokio::test]
async fn variable_types_feed_case_coalesce_arithmetic_and_conservative_nullability() {
    for expression in [
        "@a+@b",
        "CASE WHEN 1=1 THEN @a ELSE @b END",
        "COALESCE(@a,@b)",
        "TRY_CAST(@a AS bigint)",
    ] {
        typed(&format!("DECLARE @a int=1,@b bigint=2; WITH p AS(SELECT {expression} AS value) SELECT p.va<cursor>lue FROM p;"),"bigint").await;
    }
}
#[tokio::test]
async fn variable_types_keep_max_precision_and_builtin_scalar_names() {
    for ty in [
        "nvarchar(max)",
        "decimal(18,2)",
        "datetime2(3)",
        "uniqueidentifier",
    ] {
        typed(&format!("DECLARE @input {ty}; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;"),ty).await;
    }
}

fn add_overloads(h: &Harness) {
    let server = h.service.inner();
    let mut schema = server.get_schema_for_file(h.uri.as_str()).unwrap();
    schema.database = "app".into();
    schema.functions = [
        ("wrong", "int", "nvarchar(max)"),
        ("wanted", "bigint", "bigint"),
    ]
    .into_iter()
    .map(|(name, ty, result)| Function {
        name: "pick".into(),
        routine_type: Some("function".into()),
        return_type: result.into(),
        description: None,
        parameters: vec![FunctionParameter {
            name: name.into(),
            data_type: ty.into(),
            optional: false,
            mode: Some("IN".into()),
        }],
    })
    .collect();
    server.schema_manager.register(schema);
}
#[tokio::test]
async fn variable_types_choose_signature_overloads_and_udf_return_shapes() {
    let h = configured(
        "sqlserver",
        "DECLARE @input bigint; SELECT app.pick(@input<cursor>);",
    );
    add_overloads(&h);
    let help = h
        .service
        .inner()
        .signature_help(
            serde_json::from_value(
                serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(help.signatures[0].label.contains("wanted"), "{help:?}");
    assert_eq!(help.signatures.len(), 2);
    let h=configured("sqlserver","DECLARE @input bigint; WITH p AS(SELECT app.pick(@input) AS value) SELECT p.va<cursor>lue FROM p;");
    add_overloads(&h);
    let items = complete_harness(&h).await;
    assert!(
        items.iter().any(|item| item.label == "value"
            && item
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("(bigint)"))),
        "{items:?}"
    );
}
#[tokio::test]
async fn variable_types_do_not_cross_go_or_borrow_future_duplicate_and_opaque_declarations() {
    for marked in [
        "WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p; DECLARE @input bigint;",
        "DECLARE @input int; DECLARE @input bigint; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;",
        "DECLARE @input bigint;\nGO\nWITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;",
        "/* DECLARE @input bigint; */ WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;",
        "CREATE PROC app.p @input app.TableType READONLY AS BEGIN WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p; END;",
        "DECLARE @input app.UnknownType; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;",
    ] {typed(marked,"unknown").await;}
    typed("DECLARE @input bigint;\nGO\nDECLARE @input int; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;","int").await;
}

#[tokio::test]
async fn variable_types_do_not_confuse_sigil_columns_with_unquoted_variables() {
    for (reference, ty) in [
        ("@missing", "unknown"),
        ("[@missing]", "int"),
        ("@input", "bigint"),
    ] {
        let h=configured("sqlserver",&format!("DECLARE @input bigint; WITH p AS(SELECT {reference} AS value FROM users) SELECT p.va<cursor>lue FROM p;"));
        let mut schema = h
            .service
            .inner()
            .get_schema_for_file(h.uri.as_str())
            .unwrap();
        schema.tables[0]
            .columns
            .extend(["@missing", "@input"].map(|name| Column {
                name: name.into(),
                data_type: "int".into(),
                ..Default::default()
            }));
        h.service.inner().schema_manager.register(schema);
        let items = complete_harness(&h).await;
        assert!(
            items.iter().any(|item| item.label == "value"
                && item
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains(&format!("({ty})")))),
            "{reference}: {items:?}"
        );
    }
}
#[tokio::test]
async fn variable_types_refresh_declarations_and_profiles_without_stale_bindings() {
    let mut marked =
        "DECLARE @input int; WITH p AS(SELECT @input AS value) SELECT p.va<cursor>lue FROM p;"
            .to_owned();
    let mut h = configured("sqlserver", &marked);
    for (version, dialect, ty) in [
        (1, "sqlserver", "int"),
        (2, "sqlserver", "bigint"),
        (2, "mysql", "unknown"),
        (2, "mssql", "bigint"),
    ] {
        if version == 2 {
            marked = marked.replace("@input int;", "@input bigint;");
        }
        h.position = lsp_position_at_end(&marked[..marked.find("<cursor>").unwrap()]);
        h.service
            .inner()
            .configured_file_dialects
            .insert(h.uri.to_string(), dialect.into());
        h.service.inner().document_manager.update(
            h.uri.to_string(),
            marked.replace("<cursor>", ""),
            version,
        );
        let items = complete_harness(&h).await;
        assert!(
            items.iter().any(|item| item.label == "value"
                && item
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains(&format!("({ty})")))),
            "{dialect}/{version}: {items:?}"
        );
    }
}
