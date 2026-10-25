use super::*;
use tower_lsp::lsp_types::{DiagnosticSeverity, NumberOrString};

fn field<'a>(items: &'a [CompletionItem], name: &str) -> &'a CompletionItem {
    items
        .iter()
        .find(|item| item.kind == Some(CompletionItemKind::FIELD) && item.label == name)
        .unwrap_or_else(|| panic!("missing {name}: {items:?}"))
}

#[tokio::test]
async fn qualify_window_aliases_have_clickhouse_types_and_complete_leaf_edits() {
    for (expression, ty) in [
        ("row_number() OVER(ORDER BY total)", "UInt64"),
        ("rank() OVER(ORDER BY total)", "UInt64"),
        ("dense_rank() OVER(ORDER BY total)", "UInt64"),
        ("count() OVER(PARTITION BY customer_id)", "UInt64"),
    ] {
        let marked = format!(
            "/* 😀 */\r\nSELECT {expression} AS position FROM orders QUALIFY pos<cursor>ition <= 3"
        );
        let harness = configured("clickhouse", &marked);
        let items = complete_harness(&harness).await;
        let item = field(&items, "position");
        assert!(
            item.detail
                .as_deref()
                .unwrap_or("")
                .contains(&format!("({ty})")),
            "{marked}: {item:?}"
        );
        assert_eq!(items.iter().filter(|i| i.label == "position").count(), 1);
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("{item:?}")
        };
        let sql = marked.replace("<cursor>", "");
        let start = position_to_byte_offset(&sql, edit.range.start);
        let end = position_to_byte_offset(&sql, edit.range.end);
        assert_eq!(&sql[start..end], "position");
        assert_eq!(
            format!("{}{}{}", &sql[..start], edit.new_text, &sql[end..]),
            sql
        );
    }
}

#[tokio::test]
async fn qualify_uses_cte_join_projection_types_and_named_windows() {
    let marked="WITH recent AS (SELECT total AS amount, customer_id FROM orders) SELECT r.amount AS amount_due, row_number() OVER w AS rn FROM recent r JOIN users u ON r.customer_id=u.id WINDOW w AS (PARTITION BY u.id ORDER BY r.amount DESC) QUALIFY rn=1 AND amount_<cursor>due>0";
    let items = candidates("clickhouse", marked).await;
    assert!(field(&items, "amount_due")
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("(decimal)"));
    let items = candidates(
        "clickhouse",
        &marked
            .replace("amount_<cursor>due", "amount_due")
            .replace("rn=1", "r<cursor>n=1"),
    )
    .await;
    assert!(field(&items, "rn")
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("(UInt64)"));
}

#[tokio::test]
async fn qualify_window_results_keep_types_through_ctes_and_hover() {
    for call in [
        "row_number()",
        "rank()",
        "dense_rank()",
        "count()",
        "denseRank()",
    ] {
        super::quoted_names::typed("clickhouse",&format!("WITH ranked AS(SELECT {call} OVER(ORDER BY total) AS rn FROM orders QUALIFY rn=1) SELECT r.r<cursor>n FROM ranked r"),"rn","UInt64").await;
    }
}

#[tokio::test]
async fn qualify_aliases_stay_in_their_own_query_and_set_branch() {
    for (marked, own, absent) in [
        ("SELECT row_number() OVER() AS outer_rank FROM orders QUALIFY EXISTS(SELECT row_number() OVER() AS inner_rank FROM users QUALIFY <cursor>)", "inner_rank", "outer_rank"),
        ("SELECT row_number() OVER() AS outer_rank FROM orders QUALIFY <cursor> > (SELECT row_number() OVER() AS inner_rank FROM users)", "outer_rank", "inner_rank"),
        ("SELECT row_number() OVER() AS first_rank FROM orders QUALIFY first_rank=1 UNION ALL SELECT row_number() OVER() AS second_rank FROM users QUALIFY <cursor>", "second_rank", "first_rank"),
        ("SELECT row_number() OVER() AS previous_rank FROM orders QUALIFY previous_rank=1; SELECT row_number() OVER() AS current_rank FROM users QUALIFY <cursor>", "current_rank", "previous_rank"),
    ] {
        let items = candidates("clickhouse", marked).await;
        field(&items,own);
        assert!(!items.iter().any(|i|i.kind==Some(CompletionItemKind::FIELD) && i.label==absent),"{marked}: {items:?}");
    }
}

#[tokio::test]
async fn qualify_never_invents_a_source_member_or_enables_other_profiles() {
    for dialect in [
        "clickhouse",
        "postgres",
        "mysql",
        "sqlserver",
        "sqlite",
        "hive",
    ] {
        let marked = if dialect == "clickhouse" {
            "SELECT row_number() OVER() AS private_rank FROM orders o QUALIFY o.private_<cursor>rank=1"
        } else {
            "SELECT row_number() OVER() AS private_rank FROM orders QUALIFY private_<cursor>rank=1"
        };
        let items = candidates(dialect, marked).await;
        assert!(
            !items
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::FIELD) && i.label == "private_rank"),
            "{dialect}: {items:?}"
        );
    }
}

#[tokio::test]
async fn qualify_unfinished_expressions_and_quoted_unicode_keep_aliases() {
    for marked in [
        "SELECT row_number() OVER() AS `排名` FROM orders QUALIFY `排<cursor>名`=1",
        "SELECT row_number() OVER() AS `order.rank` FROM orders QUALIFY `order.<cursor>rank`=1",
        "SELECT row_number() OVER() AS rn FROM orders QUALIFY <cursor>",
        "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1 AND (<cursor>",
    ] {
        let name = if marked.contains("排名") {
            "排名"
        } else if marked.contains("order.rank") {
            "order.rank"
        } else {
            "rn"
        };
        let items = candidates("clickhouse", marked).await;
        assert!(
            items
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::FIELD) && i.label == name),
            "{marked}: {items:?}"
        );
        field(&items, name);
    }
    for marked in [
        "SELECT row_number() OVER() AS rn FROM orders QUALIFY 'r<cursor>n'=1",
        "SELECT row_number() OVER() AS rn FROM orders QUALIFY /* r<cursor>n */ rn=1",
    ] {
        assert!(
            candidates("clickhouse", marked).await.is_empty(),
            "{marked}"
        );
    }
}

#[tokio::test]
async fn qualify_without_catalog_and_same_sql_schema_refresh() {
    let h = configured(
        "clickhouse",
        "SELECT row_number() OVER() AS rn QUALIFY r<cursor>n=1",
    );
    let server = h.service.inner();
    server.configured_file_schemas.remove(h.uri.as_str());
    field(&complete_harness(&h).await, "rn");
    let h=configured("clickhouse","SELECT total AS amount_due,row_number() OVER() AS rn FROM orders QUALIFY amount_<cursor>due>0 AND rn=1");
    let server = h.service.inner();
    let id = *server.configured_file_schemas.get(h.uri.as_str()).unwrap();
    for ty in ["decimal", "UInt32", "String"] {
        let mut schema = server.schema_manager.get(id).unwrap();
        schema
            .tables
            .iter_mut()
            .find(|t| t.name == "orders")
            .unwrap()
            .columns
            .iter_mut()
            .find(|c| c.name == "total")
            .unwrap()
            .data_type = ty.into();
        server.schema_manager.update(id, schema);
        assert!(field(&complete_harness(&h).await, "amount_due")
            .detail
            .as_deref()
            .unwrap_or("")
            .contains(&format!("({ty})")));
    }
}

#[tokio::test]
async fn qualify_diagnostics_require_only_the_current_query_window_and_profile() {
    for (profile, sql, code) in [
        ("clickhouse", "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1", None),
        ("clickhouse", "SELECT total FROM orders QUALIFY row_number() OVER(ORDER BY total)=1", None),
        ("clickhouse", "SELECT total AS amount FROM orders QUALIFY amount>0", Some("QUALIFY_REQUIRES_WINDOW")),
        ("clickhouse", "SELECT (SELECT row_number() OVER() FROM users) AS rn FROM orders QUALIFY rn=1", Some("QUALIFY_REQUIRES_WINDOW")),
        ("clickhouse", "SELECT row_number() OVER() AS rn FROM orders QUALIFY EXISTS(SELECT id FROM users QUALIFY id=1)", Some("QUALIFY_REQUIRES_WINDOW")),
        ("postgres", "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1", Some("UNSUPPORTED_DIALECT_SYNTAX")),
        ("mysql", "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1", Some("UNSUPPORTED_DIALECT_SYNTAX")),
        ("sqlserver", "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1", Some("UNSUPPORTED_DIALECT_SYNTAX")),
        ("sqlite", "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1", Some("UNSUPPORTED_DIALECT_SYNTAX")),
        ("clickhouse", "SELECT 'QUALIFY' FROM orders /* QUALIFY */", None),
    ] {
        let h=configured(profile,&format!("/* 😀 */\r\n{sql}<cursor>"));
        let dialect=h.service.inner().get_dialect_for_file(h.uri.as_str()).unwrap();
        let text=h.service.inner().document_manager.get(h.uri.as_str()).unwrap();
        let diagnostics=super::super::super::diagnostics::document_diagnostics(&*dialect,&text,None).await;
        let own: Vec<_> = diagnostics.iter().filter(|d|matches!(d.code.as_ref(),Some(NumberOrString::String(s)) if s=="QUALIFY_REQUIRES_WINDOW" || s=="UNSUPPORTED_DIALECT_SYNTAX")).collect();
        assert_eq!(own.len(),usize::from(code.is_some()),"{profile}: {sql}: {diagnostics:?}");
        if let Some(code)=code { assert_eq!(own[0].code,Some(NumberOrString::String(code.into()))); assert_eq!(own[0].range.start.line,1); }
        else { assert!(!diagnostics.iter().any(|d| d.severity==Some(DiagnosticSeverity::ERROR)),"{sql}: {diagnostics:?}"); }
    }
}

#[tokio::test]
async fn qualify_ambiguous_unknown_and_non_expression_positions_do_not_guess() {
    let collision = candidates(
        "clickhouse",
        "SELECT row_number() OVER() AS id FROM orders QUALIFY i<cursor>d=1",
    )
    .await;
    assert!(
        field(&collision, "id")
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("(unknown)"),
        "{collision:?}"
    );
    let items = candidates(
        "clickhouse",
        "SELECT row_number() OVER() AS rn, total AS rn FROM orders QUALIFY r<cursor>n=1",
    )
    .await;
    assert!(!items
        .iter()
        .any(|i| i.kind == Some(CompletionItemKind::FIELD) && i.label == "rn"));
    for call in [
        "app.row_number() OVER()",
        "row_number(42) OVER()",
        "unregistered_window() OVER()",
        "ROW_NUMBER() OVER()",
    ] {
        let items = candidates(
            "clickhouse",
            &format!("SELECT {call} AS rank_value FROM orders QUALIFY rank_<cursor>value=1"),
        )
        .await;
        assert!(field(&items, "rank_value")
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("(unknown)"));
    }
    for marked in [
        "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1 <cursor>",
        "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1 ORDER BY <cursor>",
        "SELECT row_number() OVER() AS rn FROM orders WHERE <cursor>",
    ] {
        let items = candidates("clickhouse", marked).await;
        assert!(
            !items.iter().any(|i| i
                .data
                .as_ref()
                .and_then(|d| d.pointer("/oxide/projection"))
                .and_then(serde_json::Value::as_bool)
                == Some(true)),
            "{marked}: {items:?}"
        );
    }
}

#[tokio::test]
async fn qualify_keyword_entry_and_finished_predicates_offer_legal_followups() {
    for marked in [
        "SELECT row_number() OVER() AS rn FROM orders QUALI<cursor>",
        "SELECT row_number() OVER() AS rn FROM orders WHERE id>0 QUALI<cursor>",
        "SELECT row_number() OVER() AS rn QUALI<cursor>",
    ] {
        let items = candidates("clickhouse", marked).await;
        assert!(
            items
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::KEYWORD) && i.label == "QUALIFY"),
            "{marked}: {items:?}"
        );
    }
    let items = candidates(
        "clickhouse",
        "SELECT row_number() OVER() AS rn FROM orders QUALIFY rn=1 <cursor>",
    )
    .await;
    for keyword in ["AND", "OR", "ORDER BY", "LIMIT"] {
        assert!(items.iter().any(|i| i.label == keyword), "{items:?}");
    }
    assert!(!items
        .iter()
        .any(|i| i.label == "QUALIFY" || i.label == "HAVING" || i.label == "GROUP BY"));
}

#[tokio::test]
async fn qualify_wide_outputs_retry_with_prefix_and_keep_late_exact_names() {
    let mut projections = (0..300)
        .map(|n| format!("row_number() OVER() AS alias_{n}"))
        .collect::<Vec<_>>();
    projections.push("row_number() OVER() AS alias".into());
    for (prefix, incomplete, count) in [
        ("", true, 256),
        ("alias", true, 256),
        ("alias_299", false, 1),
    ] {
        let h = configured(
            "clickhouse",
            &format!(
                "SELECT {} FROM orders QUALIFY {prefix}<cursor>=1",
                projections.join(",")
            ),
        );
        let response = h
            .service
            .inner()
            .completion(
                serde_json::from_value(
                    serde_json::json!({"textDocument":{"uri":h.uri},"position":h.position}),
                )
                .unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        let (items, actual_incomplete) = match response {
            CompletionResponse::List(list) => (list.items, list.is_incomplete),
            CompletionResponse::Array(items) => (items, false),
        };
        assert_eq!(actual_incomplete, incomplete);
        let outputs: Vec<_> = items
            .iter()
            .filter(|i| {
                i.data
                    .as_ref()
                    .and_then(|d| d.pointer("/oxide/projection"))
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
            })
            .collect();
        assert_eq!(outputs.len(), count);
        assert!(outputs
            .iter()
            .all(|i| i.detail.as_deref().is_some_and(|d| d.contains("(UInt64)"))));
        if !prefix.is_empty() {
            assert!(
                outputs.iter().any(|i| i.label == prefix),
                "late exact name must survive: {prefix}"
            );
        }
    }
}
