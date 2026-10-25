use super::*;

#[tokio::test]
async fn hive_lateral_view_entry_lists_keep_functions_keywords_and_arguments_distinct() {
    for (marked, name, kind) in [
        (
            "SELECT * FROM orders o LAT<cursor>",
            "LATERAL VIEW",
            CompletionItemKind::KEYWORD,
        ),
        (
            "SELECT * FROM orders o LATERAL V<cursor>",
            "VIEW",
            CompletionItemKind::KEYWORD,
        ),
        (
            "SELECT * FROM orders o LATERAL VIEW exp<cursor>",
            "explode",
            CompletionItemKind::FUNCTION,
        ),
        (
            "SELECT * FROM orders o LATERAL VIEW OUTER pos<cursor>",
            "posexplode",
            CompletionItemKind::FUNCTION,
        ),
        (
            "SELECT * FROM orders o LATERAL VIEW explode(o.ta<cursor>gs) e AS item",
            "tags",
            CompletionItemKind::FIELD,
        ),
    ] {
        let h = hive(marked);
        let items = complete_harness(&h).await;
        assert!(
            items
                .iter()
                .any(|i| i.label == name && i.kind == Some(kind)),
            "{marked}: {items:?}"
        );
        if kind == CompletionItemKind::FUNCTION {
            assert!(
                !items
                    .iter()
                    .any(|i| i.kind == Some(CompletionItemKind::FIELD)
                        || i.kind == Some(CompletionItemKind::CLASS)),
                "{marked}: {items:?}"
            );
        }
    }
}
fn hive(marked: &str) -> Harness {
    let h = configured("hive", marked);
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    let table = schema
        .tables
        .iter_mut()
        .find(|t| t.name == "orders")
        .unwrap();
    for (name, ty) in [
        ("tags", "array<string>"),
        ("grid", "array<array<decimal(18,2)>>"),
        ("props", "map<string,array<bigint>>"),
        ("records", "array<struct<code:string,amount:decimal(18,2)>>"),
        ("payload", "string"),
    ] {
        table.columns.push(Column {
            name: name.into(),
            data_type: ty.into(),
            nullable: false,
            ..Default::default()
        });
    }
    server.schema_manager.update(id, schema);
    h
}
async fn typed(marked: &str, name: &str, ty: &str) -> CompletionItem {
    let h = hive(marked);
    let items = complete_harness(&h).await;
    let item = items
        .iter()
        .find(|i| i.label == name && i.kind == Some(CompletionItemKind::FIELD))
        .unwrap_or_else(|| panic!("{marked}: missing {name}: {items:?}"));
    assert!(
        item.detail
            .as_deref()
            .is_some_and(|d| d.contains(&format!("({ty})"))),
        "{marked}: {item:?}"
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
        .unwrap()
        .unwrap_or_else(|| panic!("{marked}: missing hover"));
    assert!(
        serde_json::to_string(&hover).unwrap().contains(ty),
        "{marked}: {hover:?}"
    );
    item.clone()
}

#[tokio::test]
async fn hive_lateral_view_explode_and_posexplode_bind_array_element_and_position_types() {
    let h = hive("SELECT e.<cursor> FROM orders o LATERAL VIEW explode(o.tags) e AS item");
    assert!(complete_harness(&h)
        .await
        .iter()
        .any(|i| i.label == "item" && i.detail.as_deref().is_some_and(|d| d.contains("(string)"))));
    typed(
        "SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(o.tags) e AS item",
        "item",
        "string",
    )
    .await;
    typed(
        "SELECT e.it<cursor>em FROM orders o LATERAL VIEW posexplode(o.grid) e AS position,item",
        "item",
        "array<decimal(18,2)>",
    )
    .await;
    typed("SELECT e.po<cursor>sition FROM orders o LATERAL VIEW posexplode(o.tags) e AS position,item","position","int").await;
}
#[tokio::test]
async fn hive_lateral_view_map_inline_and_tuple_preserve_output_arity_and_types() {
    for (marked,name,ty) in [
        ("SELECT e.ke<cursor>y FROM orders o LATERAL VIEW explode(o.props) e","key","string"),
        ("SELECT e.va<cursor>lue FROM orders o LATERAL VIEW explode(o.props) e","value","array<bigint>"),
        ("SELECT e.am<cursor>ount FROM orders o LATERAL VIEW inline(o.records) e","amount","decimal(18,2)"),
        ("SELECT e.na<cursor>me FROM orders o LATERAL VIEW json_tuple(o.payload,'name','role') e AS name,role","name","string"),
    ] {typed(marked,name,ty).await;}
}
#[tokio::test]
async fn hive_lateral_view_chains_use_only_preceding_outputs() {
    typed("SELECT b.va<cursor>lue FROM orders o LATERAL VIEW explode(o.grid) a AS nested_col LATERAL VIEW explode(a.nested_col) b AS value","value","decimal(18,2)").await;
    typed("SELECT b.va<cursor>lue FROM orders o LATERAL VIEW explode(o.props) a AS key,nested_col LATERAL VIEW explode(a.nested_col) b AS value","value","bigint").await;
    for marked in [
        "SELECT * FROM orders o LATERAL VIEW explode(b.va<cursor>lue) a AS values LATERAL VIEW explode(o.tags) b AS value",
        "SELECT * FROM orders o LATERAL VIEW explode(a.it<cursor>em) a AS item",
    ] {
        let h=hive(marked);let items=complete_harness(&h).await;assert!(!items.iter().any(|i|i.kind==Some(CompletionItemKind::FIELD)),"{marked}: {items:?}");
    }
}
#[tokio::test]
async fn hive_lateral_view_defaults_star_cte_and_derived_outputs_share_inference() {
    for marked in [
        "WITH r AS(SELECT e.* FROM orders o LATERAL VIEW explode(o.tags) e) SELECT r.co<cursor>l FROM r",
        "SELECT r.co<cursor>l FROM (SELECT e.* FROM orders o LATERAL VIEW explode(o.tags) e) r",
        "WITH q AS(SELECT tags AS data FROM orders),r AS(SELECT e.* FROM q LATERAL VIEW explode(q.data) e) SELECT r.co<cursor>l FROM r",
    ] {typed(marked,"col","string").await;}
    typed(
        "SELECT e.po<cursor>s FROM orders o LATERAL VIEW posexplode(o.tags) e",
        "pos",
        "int",
    )
    .await;
    typed(
        "SELECT e.va<cursor>l FROM orders o LATERAL VIEW posexplode(o.tags) e",
        "val",
        "string",
    )
    .await;
}
#[tokio::test]
async fn hive_lateral_view_outer_null_extends_only_view_outputs() {
    for (outer, nullable) in [("", false), ("OUTER ", true)] {
        let item = typed(
            &format!("SELECT e.po<cursor>s FROM orders o LATERAL VIEW {outer}posexplode(o.tags) e"),
            "pos",
            "int",
        )
        .await;
        assert!(serde_json::to_string(&item).unwrap().contains(if nullable {
            "Nullable: yes"
        } else {
            "Nullable: no"
        }));
    }
}
#[tokio::test]
async fn hive_lateral_view_query_scopes_and_unknown_udtfs_do_not_bind_catalog_names() {
    let marked = "SELECT e.it<cursor>em FROM orders o LATERAL VIEW app.explode(o.tags) e AS item";
    typed(marked, "item", "unknown").await;
    for marked in [
        "SELECT e.it<cursor>em FROM orders o WHERE EXISTS(SELECT 1 FROM orders t LATERAL VIEW explode(t.tags) e AS item)",
        "SELECT * FROM orders o LATERAL VIEW explode(o.tags) a AS item WHERE EXISTS(SELECT b.it<cursor>em FROM orders t LATERAL VIEW explode(t.tags) b AS value)",
    ] {let h=hive(marked);assert!(!complete_harness(&h).await.iter().any(|i|i.label=="item"&&i.kind==Some(CompletionItemKind::FIELD)),"{marked}");}
}
#[tokio::test]
async fn hive_lateral_view_unicode_quoted_leaf_edits_preserve_the_original_query() {
    let marked="/* 😀 */\r\nSELECT `e.x`.`项<cursor>目` FROM orders o LATERAL VIEW explode(o.tags) `e.x` AS `项目`";
    let item = typed(marked, "项目", "string").await;
    let Some(CompletionTextEdit::Edit(edit)) = item.text_edit else {
        panic!("missing edit")
    };
    let sql = marked.replace("<cursor>", "");
    let from = position_to_byte_offset(&sql, edit.range.start);
    let to = position_to_byte_offset(&sql, edit.range.end);
    assert_eq!(&sql[from..to], "`项目`");
    assert_eq!(
        format!("{}{}{}", &sql[..from], edit.new_text, &sql[to..]),
        sql
    );
}
#[tokio::test]
async fn hive_lateral_view_same_sql_metadata_refresh_replaces_element_types() {
    let h = hive("SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(o.tags) e AS item");
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    for ty in ["array<string>", "array<bigint>", "array<decimal(18,2)>"] {
        let mut schema = server.schema_manager.get(id).unwrap();
        schema
            .tables
            .iter_mut()
            .find(|t| t.name == "orders")
            .unwrap()
            .columns
            .iter_mut()
            .find(|c| c.name == "tags")
            .unwrap()
            .data_type = ty.into();
        server.schema_manager.update(id, schema);
        let items = complete_harness(&h).await;
        let element = &ty[6..ty.len() - 1];
        assert!(
            items.iter().any(|i| i.label == "item"
                && i.detail
                    .as_deref()
                    .is_some_and(|d| d.contains(&format!("({element})")))),
            "{ty}: {items:?}"
        );
    }
}

#[tokio::test]
async fn hive_lateral_view_constant_constructors_and_stack_are_typed() {
    for (marked,name,ty)in[
        ("SELECT e.co<cursor>l FROM orders LATERAL VIEW explode(array('a','b')) e","col","string"),
        ("SELECT e.va<cursor>l FROM orders LATERAL VIEW posexplode(array(1,2)) e","val","int"),
        ("SELECT e.co<cursor>l FROM orders LATERAL VIEW explode(array(cast(1 AS bigint))) e","col","bigint"),
        ("SELECT e.va<cursor>lue FROM orders LATERAL VIEW explode(map('k',array(1,2))) e","value","array<int>"),
        ("SELECT e.co<cursor>de FROM orders LATERAL VIEW inline(array(named_struct('code','A','amount',1))) e","code","string"),
        ("SELECT e.co<cursor>l1 FROM orders LATERAL VIEW stack(2,'A',1,'B',2) e","col1","int"),
        ("SELECT e.c<cursor>0 FROM orders LATERAL VIEW json_tuple('{\"a\":1}','a') e","c0","string"),
    ]{typed(marked,name,ty).await;}
}

#[tokio::test]
async fn hive_lateral_view_case_and_metadata_udf_inputs_share_expression_inference() {
    typed("SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(CASE WHEN o.id>0 THEN o.tags ELSE o.tags END) e AS item","item","string").await;
    let h =
        hive("SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(make_tags(o.id)) e AS item");
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    let mut schema = server.schema_manager.get(id).unwrap();
    schema.functions.push(serde_json::from_value(serde_json::json!({"name":"make_tags","routine_type":"function","parameters":[{"name":"id","data_type":"bigint","optional":false,"mode":"IN"}],"return_type":"array<bigint>"})).unwrap());
    server.schema_manager.update(id, schema);
    assert!(complete_harness(&h)
        .await
        .iter()
        .any(|i| i.label == "item" && i.detail.as_deref().is_some_and(|d| d.contains("(bigint)"))));
}

#[tokio::test]
async fn hive_lateral_view_set_branches_and_right_join_sources_keep_their_shapes() {
    typed("SELECT e.item FROM orders o LATERAL VIEW explode(o.tags) e AS item UNION ALL SELECT e.it<cursor>em FROM orders p LATERAL VIEW explode(p.grid) e AS item","item","array<decimal(18,2)>").await;
    typed("SELECT e.it<cursor>em FROM users u JOIN orders o LATERAL VIEW explode(o.tags) e AS item ON u.id=o.id","item","string").await;
    typed("SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(o.tags) e AS item WHERE o.id>0 ORDER BY e.item","item","string").await;
}

#[tokio::test]
async fn hive_lateral_view_other_profiles_keep_errors_and_never_borrow_hive_rows() {
    use tower_lsp::lsp_types::NumberOrString;
    for profile in ["postgres", "mysql", "sqlserver", "sqlite", "clickhouse"] {
        let h = configured(
            profile,
            "SELECT e.it<cursor>em FROM orders o LATERAL VIEW explode(o.tags) e AS item",
        );
        assert!(!complete_harness(&h)
            .await
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD) && i.label == "item"));
        let server = h.service.inner();
        let dialect = server.get_dialect_for_file(h.uri.as_str()).unwrap();
        let sql = server.document_manager.get(h.uri.as_str()).unwrap();
        let diagnostics =
            super::super::super::diagnostics::document_diagnostics(&*dialect, &sql, None).await;
        assert!(diagnostics.iter().any(|d|matches!(d.code.as_ref(),Some(NumberOrString::String(s))if s=="UNSUPPORTED_DIALECT_SYNTAX")),"{profile}: {diagnostics:?}");
    }
}

#[tokio::test]
async fn hive_lateral_view_cold_metadata_and_duplicate_outputs_are_conservative() {
    let h = hive("SELECT e.po<cursor>s FROM unloaded o LATERAL VIEW OUTER posexplode(o.tags) e");
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    server.schema_manager.remove(id);
    let items = complete_harness(&h).await;
    let pos = items.iter().find(|i| i.label == "pos").unwrap();
    assert!(pos.detail.as_deref().is_some_and(|s| s.contains("(int)")));
    assert!(serde_json::to_string(pos)
        .unwrap()
        .contains("Nullable: yes"));
    let h =
        hive("SELECT e.it<cursor>em FROM orders o LATERAL VIEW posexplode(o.tags) e AS item,item");
    assert!(!complete_harness(&h).await.iter().any(|i| i.label == "item"));
    typed(
        "SELECT e.it<cursor>em FROM orders o LATERAL VIEW posexplode(o.props) e AS pos,item",
        "item",
        "unknown",
    )
    .await;
}

#[tokio::test]
async fn hive_lateral_view_unknown_constructor_values_keep_known_row_structure() {
    for(marked,name)in[
        ("SELECT e.co<cursor>l FROM orders o LATERAL VIEW explode(array(unavailable(o.id))) e","col"),
        ("SELECT e.va<cursor>lue FROM orders o LATERAL VIEW explode(map('k',unavailable(o.id))) e","value"),
        ("SELECT e.co<cursor>de FROM orders o LATERAL VIEW inline(array(named_struct('code',unavailable(o.id)))) e","code"),
    ] {typed(marked,name,"unknown").await;}
}
