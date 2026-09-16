use super::*;

#[test]
fn signature_call_context_ignores_nested_sql_noise() {
    let sql = "SELECT calc(inner(1, 2), 'a,''b)', $$,)$$, /* , ) */ ";
    assert_eq!(
        routine_call_at_position(sql, lsp_position_at_end(sql)),
        Some(RoutineCallContext {
            name: "calc".to_string(),
            active_group: 0,
            active_parameter: 3,
            current_argument_has_content: false,
        })
    );

    let literal = "SELECT calc('value'";
    assert!(
        routine_call_at_position(literal, lsp_position_at_end(literal))
            .is_some_and(|call| call.current_argument_has_content)
    );
}

#[test]
fn signature_call_context_selects_nested_and_quoted_routines() {
    let nested = "SELECT outer(inner('),', ";
    assert_eq!(
        routine_call_at_position(nested, lsp_position_at_end(nested)),
        Some(RoutineCallContext {
            name: "inner".to_string(),
            active_group: 0,
            active_parameter: 1,
            current_argument_has_content: false,
        })
    );

    let quoted = "SELECT [dbo].[计算](";
    assert_eq!(
        routine_call_at_position(quoted, lsp_position_at_end(quoted)).map(|call| call.name),
        Some("计算".to_string())
    );
}

#[test]
fn signature_call_context_supports_parametric_function_groups() {
    let sql = "SELECT quantilesTDigest(0.5, 0.9)(value, ";
    assert_eq!(
        routine_call_at_position(sql, lsp_position_at_end(sql)),
        Some(RoutineCallContext {
            name: "quantilesTDigest".to_string(),
            active_group: 1,
            active_parameter: 1,
            current_argument_has_content: false,
        })
    );
}

#[test]
fn signature_overload_fit_prioritizes_a_available_parameter_slot() {
    let call = RoutineCallContext {
        name: "calculate".to_string(),
        active_group: 0,
        active_parameter: 1,
        current_argument_has_content: false,
    };
    let one_parameter = Function {
        name: "calculate".to_string(),
        routine_type: Some("function".to_string()),
        parameters: vec![FunctionParameter {
            name: "value".to_string(),
            data_type: "numeric".to_string(),
            optional: false,
        }],
        return_type: "numeric".to_string(),
        description: None,
    };
    let mut two_parameters = one_parameter.clone();
    two_parameters.parameters.push(FunctionParameter {
        name: "precision".to_string(),
        data_type: "integer".to_string(),
        optional: true,
    });

    assert!(!live_overload_accepts_call(&one_parameter, &call));
    assert!(live_overload_accepts_call(&two_parameters, &call));
}

#[test]
fn automatic_code_actions_skip_full_document_source_work() {
    let automatic = CodeActionContext {
        diagnostics: Vec::new(),
        only: None,
        trigger_kind: None,
    };
    assert!(code_action_kind_available(
        &automatic,
        &CodeActionKind::REFACTOR_REWRITE
    ));
    assert!(!code_action_kind_explicitly_requested(
        &automatic,
        &CodeActionKind::SOURCE
    ));

    let source_only = CodeActionContext {
        diagnostics: Vec::new(),
        only: Some(vec![CodeActionKind::SOURCE]),
        trigger_kind: None,
    };
    assert!(code_action_kind_explicitly_requested(
        &source_only,
        &CodeActionKind::SOURCE
    ));
    assert!(!code_action_kind_available(
        &source_only,
        &CodeActionKind::REFACTOR_REWRITE
    ));
}
