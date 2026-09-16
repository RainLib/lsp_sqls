use crate::parser::SqlParser;
use tower_lsp::lsp_types::{Location, Position, Range, Url};

pub(crate) fn format_sql_pretty(source: &str) -> String {
    format_sql_pretty_with_keyword_case(source, Some(true))
}

pub(crate) fn format_sql_pretty_with_keyword_case(source: &str, uppercase: Option<bool>) -> String {
    use sqlformat::{FormatOptions, Indent, QueryParams};
    let options = FormatOptions {
        indent: Indent::Spaces(2),
        uppercase,
        lines_between_queries: 1,
        ..FormatOptions::default()
    };
    sqlformat::format(source, &QueryParams::None, &options)
}

fn collect_invocation_name_ranges(node: tree_sitter::Node<'_>, ranges: &mut Vec<(usize, usize)>) {
    if node.kind() == "invocation" {
        let mut cursor = node.walk();
        if let Some(reference) = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "object_reference")
        {
            if let Some(name) = reference.child_by_field_name("name") {
                ranges.push((name.start_byte(), name.end_byte()));
            }
        };
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_invocation_name_ranges(child, ranges);
    }
}

/// Apply function casing only to Tree-sitter invocation-name nodes. This keeps
/// relation names, CTE column lists, parameter placeholders, data types,
/// comments, strings, and quoted identifiers byte-for-byte intact.
pub(crate) fn apply_formatter_function_case(source: &str, uppercase: Option<bool>) -> String {
    let Some(uppercase) = uppercase else {
        return source.to_string();
    };
    let mut parser = SqlParser::new();
    let parsed = parser.parse(source);
    let Some(tree) = parsed.tree.as_ref() else {
        return source.to_string();
    };
    let mut ranges = Vec::new();
    collect_invocation_name_ranges(tree.root_node(), &mut ranges);
    ranges.sort_unstable();
    ranges.dedup();

    let mut formatted = source.to_string();
    for (start, end) in ranges.into_iter().rev() {
        let Some(name) = source.get(start..end) else {
            continue;
        };
        if name.starts_with(['"', '`', '[']) {
            continue;
        }
        let replacement = if uppercase {
            name.to_uppercase()
        } else {
            name.to_lowercase()
        };
        formatted.replace_range(start..end, &replacement);
    }
    formatted
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LogicalOperatorLayout {
    #[default]
    Before,
    After,
    SameLine,
}

/// Apply editor-only layout preferences after `sqlformat` has produced a
/// syntactically safe baseline. The parser mask retains byte offsets and line
/// breaks while hiding comments, strings and quoted identifiers, so layout
/// keywords inside user content can never be rewritten.
pub(crate) fn apply_formatter_layout(
    source: &str,
    logical_operator_layout: LogicalOperatorLayout,
    from_source_same_line: bool,
) -> String {
    let mut lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    let masked_source = SqlParser::mask_sql_noise(source);
    let mut masked_lines = masked_source
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let had_trailing_newline = source.ends_with('\n');

    if logical_operator_layout != LogicalOperatorLayout::Before {
        let mut rewritten_lines = Vec::with_capacity(lines.len());
        let mut rewritten_masked_lines = Vec::with_capacity(masked_lines.len());
        for (current, masked_line) in lines.into_iter().zip(masked_lines.into_iter()) {
            let leading = masked_line.len() - masked_line.trim_start().len();
            let trimmed_upper = masked_line[leading..].to_ascii_uppercase();
            let operator_len = ["AND", "OR", "XOR"]
                .into_iter()
                .find(|operator| {
                    trimmed_upper == *operator
                        || trimmed_upper
                            .strip_prefix(operator)
                            .and_then(|rest| rest.chars().next())
                            .is_some_and(char::is_whitespace)
                })
                .map(str::len);
            let can_rewrite = operator_len.is_some()
                && rewritten_masked_lines
                    .last()
                    .is_some_and(|previous: &String| !previous.trim().is_empty());
            if !can_rewrite {
                rewritten_lines.push(current);
                rewritten_masked_lines.push(masked_line);
                continue;
            }

            let operator_len = operator_len.expect("checked above");
            let operator = &current[leading..leading + operator_len];
            let remainder = current[leading + operator_len..].trim_start();
            let masked_remainder = masked_line[leading + operator_len..].trim_start();
            match logical_operator_layout {
                LogicalOperatorLayout::SameLine => {
                    let previous = rewritten_lines.pop().expect("checked above");
                    let masked_previous = rewritten_masked_lines.pop().expect("checked above");
                    rewritten_lines.push(if remainder.is_empty() {
                        format!("{previous} {operator}")
                    } else {
                        format!("{} {operator} {remainder}", previous.trim_end())
                    });
                    rewritten_masked_lines.push(if masked_remainder.is_empty() {
                        format!("{masked_previous} {operator}")
                    } else {
                        format!(
                            "{} {operator} {masked_remainder}",
                            masked_previous.trim_end()
                        )
                    });
                }
                LogicalOperatorLayout::After => {
                    let previous = rewritten_lines.last_mut().expect("checked above");
                    *previous = format!("{} {operator}", previous.trim_end());
                    let masked_previous = rewritten_masked_lines.last_mut().expect("checked above");
                    *masked_previous = format!("{} {operator}", masked_previous.trim_end());
                    rewritten_lines.push(if remainder.is_empty() {
                        " ".repeat(leading)
                    } else {
                        format!("{}{remainder}", " ".repeat(leading))
                    });
                    rewritten_masked_lines.push(if masked_remainder.is_empty() {
                        " ".repeat(leading)
                    } else {
                        format!("{}{masked_remainder}", " ".repeat(leading))
                    });
                }
                LogicalOperatorLayout::Before => {}
            }
        }
        lines = rewritten_lines;
        masked_lines = rewritten_masked_lines;
    }

    if from_source_same_line {
        let mut rewritten_lines = Vec::with_capacity(lines.len());
        let mut index = 0usize;
        while index + 1 < lines.len() && index + 1 < masked_lines.len() {
            if !masked_lines[index].trim().eq_ignore_ascii_case("FROM") {
                rewritten_lines.push(lines[index].clone());
                index += 1;
                continue;
            }
            let next_masked = masked_lines[index + 1].trim_start();
            let next_source = lines[index + 1].trim_start();
            if next_masked.is_empty()
                || next_source.starts_with('(')
                || next_source.starts_with("/*")
                || next_source.starts_with("--")
            {
                rewritten_lines.push(lines[index].clone());
                index += 1;
                continue;
            }
            rewritten_lines.push(format!("{} {}", lines[index].trim_end(), next_source));
            index += 2;
        }
        rewritten_lines.extend(lines[index..].iter().cloned());
        lines = rewritten_lines;
    }

    let mut formatted = lines.join("\n");
    if had_trailing_newline {
        formatted.push('\n');
    }
    formatted
}

pub(crate) fn metadata_location(
    source_location: Option<&(String, u32)>,
    schema_uri: Option<&String>,
    fallback_uri: &str,
) -> Option<Location> {
    let (raw_uri, line) = if let Some((uri, line)) = source_location {
        (uri.as_str(), line.saturating_sub(1))
    } else if let Some(uri) = schema_uri {
        (uri.as_str(), 0)
    } else {
        (fallback_uri, 0)
    };
    Some(Location {
        uri: Url::parse(raw_uri).ok()?,
        range: Range {
            start: Position { line, character: 0 },
            end: Position { line, character: 0 },
        },
    })
}

fn dollar_quote_tag_at(source: &str, index: usize) -> Option<&str> {
    let rest = source.get(index..)?;
    let bytes = rest.as_bytes();
    if bytes.first() != Some(&b'$') {
        return None;
    }

    let mut end = 1;
    while end < bytes.len() {
        let byte = bytes[end];
        if byte == b'$' {
            return Some(&rest[..=end]);
        }
        if end == 1 && !(byte == b'_' || byte.is_ascii_alphabetic()) {
            return None;
        }
        if end > 1 && !(byte == b'_' || byte.is_ascii_alphanumeric()) {
            return None;
        }
        end += 1;
    }

    None
}

fn flush_compact_space(output: &mut String, pending_space: &mut bool) {
    if *pending_space
        && !output.is_empty()
        && output.chars().last().is_some_and(|ch| !ch.is_whitespace())
    {
        output.push(' ');
    }
    *pending_space = false;
}

/// Compact whitespace without changing quoted values, identifiers, comments,
/// or PostgreSQL dollar-quoted bodies. This is intentionally conservative: it
/// is a safe fallback formatter, not a SQL rewriter.
pub(crate) fn compact_sql_whitespace(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut pending_space = false;
    let mut index = 0usize;

    while index < source.len() {
        if source[index..].starts_with("--") {
            flush_compact_space(&mut output, &mut pending_space);
            let end = source[index..]
                .find('\n')
                .map(|relative| index + relative + 1)
                .unwrap_or(source.len());
            output.push_str(&source[index..end]);
            index = end;
            continue;
        }

        if source[index..].starts_with("/*") {
            flush_compact_space(&mut output, &mut pending_space);
            let end = source[index + 2..]
                .find("*/")
                .map(|relative| index + relative + 4)
                .unwrap_or(source.len());
            output.push_str(&source[index..end]);
            index = end;
            pending_space = true;
            continue;
        }

        if let Some(tag) = dollar_quote_tag_at(source, index) {
            flush_compact_space(&mut output, &mut pending_space);
            let body_start = index + tag.len();
            let end = source[body_start..]
                .find(tag)
                .map(|relative| body_start + relative + tag.len())
                .unwrap_or(source.len());
            output.push_str(&source[index..end]);
            index = end;
            continue;
        }

        let Some(ch) = source[index..].chars().next() else {
            break;
        };
        if ch.is_whitespace() {
            pending_space = true;
            index += ch.len_utf8();
            continue;
        }

        if matches!(ch, '\'' | '"' | '`' | '[') {
            flush_compact_space(&mut output, &mut pending_space);
            let start = index;
            let closing = if ch == '[' { ']' } else { ch };
            index += ch.len_utf8();
            while index < source.len() {
                let Some(quoted) = source[index..].chars().next() else {
                    break;
                };
                index += quoted.len_utf8();

                if quoted == '\\' && ch != '[' && index < source.len() {
                    if let Some(escaped) = source[index..].chars().next() {
                        index += escaped.len_utf8();
                    }
                    continue;
                }

                if quoted == closing {
                    if source[index..].starts_with(closing) {
                        index += closing.len_utf8();
                        continue;
                    }
                    break;
                }
            }
            output.push_str(&source[start..index]);
            continue;
        }

        flush_compact_space(&mut output, &mut pending_space);
        output.push(ch);
        index += ch.len_utf8();
    }

    output.trim().to_string()
}
