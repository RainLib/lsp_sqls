//! Module and batch invariants supplement the shared multi-dialect grammar.
use super::types::SqlParser;
use crate::placeholder::SqlPlaceholderDialect;
use std::collections::HashSet;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use tree_sitter::Node;

pub(super) fn validate(
    parser: &SqlParser,
    node: Node,
    source: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if node.kind() == "program" && parser.placeholder_dialect == SqlPlaceholderDialect::SqlServer {
        validate_batches(parser, node, source, diagnostics);
        return;
    }
    if node.kind() == "tsql_batch_separator"
        && parser.placeholder_dialect == SqlPlaceholderDialect::SqlServer
    {
        return;
    }
    let module = matches!(
        node.kind(),
        "tsql_procedure_definition" | "tsql_function_definition"
    );
    let separator = node.kind() == "tsql_batch_separator";
    let foreign_function = node.kind() == "create_function";
    if !module
        && !separator
        && !foreign_function
        && !matches!(
            node.kind(),
            "tsql_procedure_parameter" | "tsql_function_parameter" | "tsql_set_statement"
        )
    {
        return;
    }
    let mut error = |code: &str, message: &str| {
        diagnostics.push(Diagnostic {
            range: parser.node_range(node),
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String(code.into())),
            source: Some("sql-dialect".into()),
            message: message.into(),
            ..Default::default()
        })
    };
    if parser.placeholder_dialect != SqlPlaceholderDialect::SqlServer {
        if module || separator {
            error(
                "UNSUPPORTED_DIALECT_SYNTAX",
                "T-SQL module definitions and GO require the SQL Server profile",
            );
        }
        return;
    }
    if foreign_function {
        error(
            "UNSUPPORTED_DIALECT_SYNTAX",
            "SQL Server function definitions require T-SQL parameters and a T-SQL body",
        );
        return;
    }
    if module {
        let mut previous = node.prev_named_sibling();
        while previous.is_some_and(|sibling| matches!(sibling.kind(), "comment" | "marginalia")) {
            previous = previous.and_then(|sibling| sibling.prev_named_sibling());
        }
        if previous.is_some_and(|sibling| sibling.kind() != "tsql_batch_separator") {
            error(
                "INVALID_TSQL_MODULE_BATCH",
                "A T-SQL module must begin its own batch",
            );
        }
        let mut names = HashSet::new();
        let mut parameters = 0;
        for parameter in node.named_children(&mut node.walk()).filter(|child| {
            matches!(
                child.kind(),
                "tsql_procedure_parameter" | "tsql_function_parameter"
            )
        }) {
            parameters += 1;
            if let Some(name) = parameter.child_by_field_name("name") {
                if !names.insert(&source[name.byte_range()]) {
                    error(
                        "DUPLICATE_TSQL_PARAMETER",
                        "A module parameter is declared more than once",
                    );
                }
            }
        }
        if parameters > 2100 {
            error(
                "TOO_MANY_TSQL_PARAMETERS",
                "SQL Server modules allow at most 2100 parameters",
            );
        }
        if let Some(options) = node
            .named_children(&mut node.walk())
            .find(|child| child.kind() == "tsql_module_options")
        {
            let invalid = options.named_children(&mut options.walk()).any(|child| {
                if node.kind() == "tsql_function_definition" {
                    child.kind() == "keyword_recompile"
                } else {
                    child.kind() == "keyword_schemabinding"
                }
            });
            if invalid {
                error(
                    "INVALID_TSQL_MODULE_OPTION",
                    "This option is not supported for this T-SQL module kind",
                );
            }
        }
    } else {
        if node.child_by_field_name("default").is_some_and(|value| {
            let raw = source[value.byte_range()].trim();
            crate::tsql_literal::end_at(raw, 0) != Some(raw.len())
        }) {
            error(
                "INVALID_TSQL_PARAMETER_DEFAULT",
                "T-SQL parameter defaults require a string, number, binary constant or NULL",
            );
        }
        if node.child_by_field_name("name").is_some_and(|name| {
            let name = &source[name.byte_range()];
            !name.starts_with('@') || name.starts_with("@@")
        }) {
            error(
                "INVALID_TSQL_VARIABLE",
                "Local parameter and assignment names require an @name",
            );
        }
        let mut readonly = false;
        let mut output = false;
        let mut array = false;
        for child in node.named_children(&mut node.walk()) {
            readonly |= child.kind() == "keyword_readonly";
            output |= matches!(child.kind(), "keyword_out" | "keyword_output");
            array |= child.kind() == "array_size_definition";
        }
        if readonly && output || array {
            error(
                "INVALID_TSQL_PARAMETER_TYPE",
                "T-SQL array parameters and READONLY output parameters are not supported",
            );
        }
    }
}

// One lexical mask and monotonically advancing line bounds for the program;
// repeated invalid GO tokens on a long line must not rescan it quadratically.
fn validate_batches(
    parser: &SqlParser,
    root: Node,
    source: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !root
        .named_children(&mut root.walk())
        .any(|node| node.kind() == "tsql_batch_separator")
    {
        return;
    }
    let noise = SqlParser::mask_sql_noise_for_dialect(source, "sqlserver");
    let (mut line_end, mut first, mut last) = (0, 0, 0);
    for node in root
        .named_children(&mut root.walk())
        .filter(|node| node.kind() == "tsql_batch_separator")
    {
        if node.start_byte() >= line_end {
            let line_start = noise[line_end..node.start_byte()]
                .rfind(['\r', '\n'])
                .map_or(line_end, |at| line_end + at + 1);
            line_end = noise[node.end_byte()..]
                .find(['\r', '\n'])
                .map_or(noise.len(), |at| node.end_byte() + at);
            let line = &noise[line_start..line_end];
            first = line_start + line.len() - line.trim_start().len();
            last = line_end - (line.len() - line.trim_end().len());
        }
        let standalone = node.start_byte() == first && node.end_byte() == last;
        let valid_count = node
            .named_children(&mut node.walk())
            .filter(|child| child.kind() == "literal")
            .all(|count| {
                source[count.byte_range()]
                    .parse::<u32>()
                    .is_ok_and(|n| n > 0)
            });
        if !standalone || !valid_count {
            diagnostics.push(Diagnostic {
                range: parser.node_range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String(
                    "INVALID_TSQL_BATCH_SEPARATOR".into(),
                )),
                source: Some("sql-dialect".into()),
                message: "GO must occupy its own line with an optional positive count".into(),
                ..Default::default()
            });
        }
    }
}
