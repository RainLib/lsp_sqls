use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

#[derive(Debug)]
pub(super) struct HttpBody {
    pub(super) text: String,
    pub(super) start_line: u32,
}

pub(super) enum HttpRequestBodies {
    NotHttp,
    Bodies(Vec<HttpBody>),
    Diagnostics(Vec<Diagnostic>),
}

pub(super) fn extract_http_request_bodies(input: &str) -> HttpRequestBodies {
    let mut saw_request = false;
    let mut bodies = Vec::new();
    let mut current_body = Vec::<String>::new();
    let mut current_start_line = 0u32;

    for (line_index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        match parse_http_request_line(trimmed, line_index as u32) {
            Ok(Some(inline_body)) => {
                saw_request = true;
                push_http_body(&mut bodies, &mut current_body, current_start_line);
                if let Some(body) = inline_body {
                    current_start_line = line_index as u32;
                    current_body.push(body);
                }
            }
            Ok(None) if saw_request => {
                if current_body.is_empty() {
                    current_start_line = line_index as u32;
                }
                current_body.push(trimmed.to_string());
            }
            Ok(None) => return HttpRequestBodies::NotHttp,
            Err(diagnostic) => return HttpRequestBodies::Diagnostics(vec![*diagnostic]),
        }
    }

    if !saw_request {
        return HttpRequestBodies::NotHttp;
    }

    push_http_body(&mut bodies, &mut current_body, current_start_line);
    HttpRequestBodies::Bodies(bodies)
}

fn parse_http_request_line(
    line: &str,
    line_index: u32,
) -> Result<Option<Option<String>>, Box<Diagnostic>> {
    let Some((method, rest)) = line.split_once(char::is_whitespace) else {
        return Ok(None);
    };
    if !matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "PATCH"
    ) {
        return Ok(None);
    }

    let rest = rest.trim();
    if rest.is_empty() {
        return Err(Box::new(http_request_diagnostic(
            line_index,
            line,
            "Elasticsearch request path is required",
        )));
    }

    let mut split = rest.splitn(2, char::is_whitespace);
    let path = split.next().unwrap_or("").trim();
    if !path.starts_with('/') {
        return Err(Box::new(http_request_diagnostic(
            line_index,
            line,
            "Elasticsearch request path must start with '/'",
        )));
    }

    Ok(Some(
        split
            .next()
            .map(str::trim)
            .filter(|body| !body.is_empty())
            .map(ToString::to_string),
    ))
}

fn push_http_body(
    bodies: &mut Vec<HttpBody>,
    current_body: &mut Vec<String>,
    current_start_line: u32,
) {
    if current_body.is_empty() {
        return;
    }

    bodies.push(HttpBody {
        text: current_body.join("\n"),
        start_line: current_start_line,
    });
    current_body.clear();
}

pub(super) fn offset_diagnostics(diagnostics: &mut [Diagnostic], line_offset: u32) {
    for diagnostic in diagnostics {
        diagnostic.range.start.line += line_offset;
        diagnostic.range.end.line += line_offset;
    }
}

fn http_request_diagnostic(line: u32, text: &str, message: &str) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: Position { line, character: 0 },
            end: Position {
                line,
                character: text.encode_utf16().count() as u32,
            },
        },
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(
            "ELASTICSEARCH_HTTP_REQUEST".to_string(),
        )),
        code_description: None,
        source: Some("elasticsearch-dsl".to_string()),
        message: message.to_string(),
        related_information: None,
        tags: None,
        data: None,
    }
}
