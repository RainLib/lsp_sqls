use super::*;
fn parameter(name: &str, ty: &str, optional: bool, mode: Option<&str>) -> FunctionParameter {
    FunctionParameter {
        name: name.into(),
        data_type: ty.into(),
        optional,
        mode: mode.map(str::to_owned),
    }
}
fn function(name: &str, input: &str, result: &str) -> Function {
    Function {
        name: name.into(),
        routine_type: Some("function".into()),
        parameters: vec![parameter("value", input, false, None)],
        return_type: result.into(),
        description: None,
    }
}
fn schema(functions: Vec<Function>) -> Schema {
    let mut schema = semantic_test_schema();
    schema.database = "app".into();
    schema.catalog = Some("db".into());
    schema.functions = functions;
    schema
}
fn output(dialect: &str, sql: &str, functions: Vec<Function>) -> Column {
    let columns = super::super::local_relations::infer_query_output_columns(
        sql,
        &schema(functions),
        "oxide://udf/output.sql",
        1,
        Some(dialect),
    );
    assert_eq!(columns.len(), 1, "{sql}: {columns:?}");
    columns.into_iter().next().unwrap()
}

#[test]
fn udf_outputs_keep_scalar_declared_types_and_quoted_identity_across_profiles() {
    for (dialect, reference, name) in [
        ("postgres", "calc", "calc"),
        ("postgres", "app.calc", "calc"),
        ("postgres", "db.app.calc", "calc"),
        ("postgres", "\"CaLc\"", "CaLc"),
        ("postgres", "app.\"a.b\"", "a.b"),
        ("postgres", "app /*x*/ . \"a\"\"b\"", "a\"b"),
        ("mysql", "`app`.`ca``lc`", "ca`lc"),
        ("sqlserver", "[db].[app].[ca]]lc]", "ca]lc"),
    ] {
        let column = output(
            dialect,
            &format!("SELECT {reference}(1) AS result"),
            vec![function(name, "integer", "numeric(12,2)")],
        );
        assert_eq!(column.data_type, "numeric(12,2)", "{reference}");
        assert!(column.nullable);
    }
}
#[test]
fn udf_outputs_feed_nested_expressions_ctes_derived_and_scalar_queries() {
    for sql in [
        "WITH p AS (SELECT app.calc(1) AS value) SELECT value FROM p",
        "SELECT d.value FROM (SELECT app.calc(1) AS value) d",
        "SELECT (SELECT app.calc(1)) AS result",
        "SELECT CASE WHEN TRUE THEN app.calc(1) ELSE 2::bigint END AS result",
        "SELECT COALESCE(app.calc(1),2::bigint) AS result",
        "SELECT app.calc(app.calc(1)) AS result",
        "WITH RECURSIVE p(value) AS (SELECT app.calc(1) UNION ALL SELECT value FROM p WHERE FALSE) SELECT value FROM p",
    ] {
        assert_eq!(output("postgres",sql,vec![function("calc","integer","bigint"),function("calc","bigint","bigint")]).data_type,"bigint","{sql}");
    }
}
#[test]
fn udf_outputs_choose_only_proven_exact_overloads_or_shared_returns() {
    for (expression, functions, wanted) in [
        (
            "calc(1)",
            vec![
                function("calc", "text", "uuid"),
                function("calc", "integer", "bigint"),
            ],
            "bigint",
        ),
        (
            "calc(CAST('x' AS text))",
            vec![
                function("calc", "integer", "bigint"),
                function("calc", "text", "uuid"),
            ],
            "uuid",
        ),
        (
            "calc(NULL)",
            vec![
                function("calc", "integer", "bigint"),
                function("calc", "text", "uuid"),
            ],
            "unknown",
        ),
        (
            "calc(NULL)",
            vec![
                function("calc", "integer", "bigint"),
                function("calc", "text", "int8"),
            ],
            "bigint",
        ),
        (
            "calc('x')",
            vec![
                function("calc", "integer", "bigint"),
                function("calc", "text", "uuid"),
            ],
            "unknown",
        ),
    ] {
        assert_eq!(
            output(
                "postgres",
                &format!("SELECT {expression} AS result"),
                functions
            )
            .data_type,
            wanted,
            "{expression}"
        );
    }
}
#[test]
fn udf_outputs_live_metadata_blocks_builtin_guesses_but_not_sql_constructs() {
    for (sql, functions, wanted) in [
        (
            "SELECT count(1) AS result",
            vec![function("count", "integer", "uuid")],
            "uuid",
        ),
        (
            "SELECT sum(1) AS result",
            vec![function("sum", "integer", "uuid")],
            "uuid",
        ),
        (
            "SELECT coalesce(1,2) AS result",
            vec![function("coalesce", "integer", "uuid")],
            "integer",
        ),
        (
            "SELECT \"coalesce\"(1) AS result",
            vec![function("coalesce", "integer", "uuid")],
            "uuid",
        ),
        (
            "SELECT missing.sum(1) AS result",
            vec![function("sum", "integer", "uuid")],
            "unknown",
        ),
        (
            "SELECT pg_catalog.count(1) AS result",
            vec![function("count", "integer", "uuid")],
            "bigint",
        ),
    ] {
        assert_eq!(
            output("postgres", sql, functions).data_type,
            wanted,
            "{sql}"
        );
    }
    let mut procedure = function("count", "integer", "uuid");
    procedure.routine_type = Some("procedure".into());
    let column = output("postgres", "SELECT count(1) AS result", vec![procedure]);
    assert_eq!(column.data_type, "bigint");
    assert!(!column.nullable);
    let mut procedure = function("to_tsvector", "text", "uuid");
    procedure.routine_type = Some("procedure".into());
    assert_eq!(
        output(
            "postgres",
            "SELECT to_tsvector('x') AS result",
            vec![procedure]
        )
        .data_type,
        "tsvector"
    );
}
#[test]
fn udf_outputs_unknown_pseudo_set_and_invalid_bindings_remain_unknown() {
    for result in [
        "void",
        "record",
        "SETOF integer",
        "TABLE (value integer)",
        "anyelement",
        "anyarray",
        "anycompatible",
        "unknown",
        "",
    ] {
        assert_eq!(
            output(
                "postgres",
                "SELECT calc(1) AS result",
                vec![function("calc", "integer", result)]
            )
            .data_type,
            "unknown",
            "{result}"
        );
    }
    for sql in [
        "SELECT calc() AS result",
        "SELECT calc(1,2) AS result",
        "SELECT calc(missing=>1) AS result",
        "SELECT calc(value=>1,value=>2) AS result",
        "SELECT \"CALC\"(1) AS result",
        "SELECT other.calc(1) AS result",
        "SELECT foreign.app.calc(1) AS result",
    ] {
        assert_eq!(
            output("postgres", sql, vec![function("calc", "integer", "bigint")]).data_type,
            "unknown",
            "{sql}"
        );
    }
    let mut procedure = function("calc", "integer", "bigint");
    procedure.routine_type = Some("procedure".into());
    assert_eq!(
        output("postgres", "SELECT calc(1) AS result", vec![procedure]).data_type,
        "unknown"
    );
}
#[test]
fn udf_outputs_reuse_named_default_output_and_variadic_bindings() {
    let mut fixed = function("calc", "integer", "bigint");
    fixed.parameters = vec![
        parameter("out", "text", false, Some("OUT")),
        parameter("first", "integer", false, None),
        parameter("second", "text", true, None),
    ];
    for sql in [
        "SELECT calc(1) AS result",
        "SELECT calc(second=>CAST('x' AS text),first=>1) AS result",
    ] {
        assert_eq!(
            output("postgres", sql, vec![fixed.clone()]).data_type,
            "bigint",
            "{sql}"
        );
    }
    let mut var = function("calc", "integer[]", "bigint");
    var.parameters[0].mode = Some("VARIADIC".into());
    for sql in [
        "SELECT calc(1,2,3) AS result",
        "SELECT calc(VARIADIC value=>ARRAY[1,2]::integer[]) AS result",
        "SELECT calc(VARIADIC(ARRAY[1,2]::integer[])) AS result",
    ] {
        assert_eq!(
            output("postgres", sql, vec![var.clone()]).data_type,
            "bigint",
            "{sql}"
        );
    }
}
#[test]
fn udf_outputs_large_metadata_keeps_late_exact_match_and_bounded_types() {
    let mut functions = (0..1000)
        .map(|_| function("calc", "text", "uuid"))
        .collect::<Vec<_>>();
    functions.push(function("calc", "integer", "bigint"));
    let started = std::time::Instant::now();
    assert_eq!(
        output("postgres", "SELECT calc(1) AS result", functions).data_type,
        "bigint"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(
        output(
            "postgres",
            "SELECT calc(1) AS result",
            vec![function("calc", "integer", &"custom".repeat(1000))]
        )
        .data_type,
        "unknown"
    );
}

#[tokio::test]
async fn udf_outputs_remote_namespaces_are_unique_catalog_scoped_and_refresh_live() {
    use tower_lsp::LspService;
    let (service, _socket) = LspService::new(crate::server::SqlLspServer::new);
    let server = service.inner();
    let uri = Url::parse("oxide://udf/remote.mysql.sql").unwrap();
    let id = server
        .schema_manager
        .register(schema(vec![function("calc", "integer", "text")]));
    server.configured_file_schemas.insert(uri.to_string(), id);
    server
        .configured_file_dialects
        .insert(uri.to_string(), "postgres".into());
    let mut remote = schema(vec![function("calc", "integer", "bigint")]);
    remote.id = SchemaId::new();
    remote.database = "analytics".into();
    let remote_id = server.schema_manager.register(remote.clone());
    let sql = "WITH p AS (SELECT analytics.calc(1) AS value) SELECT p. FROM p";
    let cursor = sql.find("p. FROM").unwrap() + 2;
    let ty = || {
        server
            .get_schema_for_position(uri.as_str(), sql, lsp_position_at_end(&sql[..cursor]))
            .unwrap()
            .tables
            .into_iter()
            .find(|table| table.name == "p")
            .unwrap()
            .columns[0]
            .data_type
            .clone()
    };
    assert_eq!(ty(), "bigint");
    remote.functions[0].return_type = "uuid".into();
    server.schema_manager.update(remote_id, remote.clone());
    assert_eq!(ty(), "uuid");
    remote.id = SchemaId::new();
    let duplicate = server.schema_manager.register(remote);
    assert_eq!(ty(), "unknown");
    server.schema_manager.remove(duplicate);
    assert_eq!(ty(), "uuid");
    server.schema_manager.remove(remote_id);
    assert_eq!(ty(), "unknown");
}

#[test]
fn udf_outputs_prefer_pg_fixed_exact_calls_and_reject_polymorphic_arrays() {
    let mut variadic = function("calc", "integer[]", "uuid");
    variadic.parameters[0].mode = Some("VARIADIC".into());
    assert_eq!(
        output(
            "postgres",
            "SELECT calc(1) AS value",
            vec![variadic, function("calc", "integer", "bigint")]
        )
        .data_type,
        "bigint"
    );
    for ty in ["anyelement[]", "pg_catalog.anycompatible[]", "record[]"] {
        assert_eq!(
            output(
                "postgres",
                "SELECT calc(1) AS value",
                vec![function("calc", "integer", ty)]
            )
            .data_type,
            "unknown",
            "{ty}"
        );
    }
}
