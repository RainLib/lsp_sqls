//! Bounded module-header metadata over existing lexical masks. A module must
//! start its batch; strings/comments and incomplete defaults cannot declare names.
use super::{
    execute_context::keyword_at, routine_identity::read_parameter_name, tsql_variables::Variable,
};

pub(super) struct Scope {
    pub(super) body: usize,
    pub(super) parameters: Vec<Variable>,
}
fn next(content: &str, at: usize) -> usize {
    at + content[at..]
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(content.len() - at)
}
fn word(noise: &str, content: &str, at: &mut usize, word: &str) -> bool {
    *at = next(content, *at);
    if keyword_at(noise, *at, word) {
        *at += word.len();
        true
    } else {
        false
    }
}
fn reference(text: &str, content: &str, mut at: usize) -> Option<usize> {
    for part in 0..2 {
        let (name, length) = read_parameter_name(text.get(at..)?, "sqlserver")?;
        if name.starts_with('@') {
            return None;
        }
        at += length;
        let after = next(content, at);
        if content.as_bytes().get(after) != Some(&b'.') {
            return Some(at);
        }
        if part == 1 {
            return None;
        }
        at = next(content, after + 1);
    }
    None
}
fn data_type(text: &str, content: &str, at: &mut usize) -> Option<String> {
    let start = next(content, *at);
    let mut end = reference(text, content, start)?;
    let opening = next(content, end);
    if content.as_bytes().get(opening) == Some(&b'(') {
        // Type precision/MAX are bounded syntax, not arbitrary expressions.
        let closing = content.get(opening + 1..)?.find(')')? + opening + 1;
        if closing - start > 256 || content[opening + 1..closing].contains('(') {
            return None;
        }
        end = closing + 1;
    }
    *at = next(content, end);
    let ty = text.get(start..end)?.trim();
    Some(if ty.len() <= 256 {
        ty.into()
    } else {
        "unknown".into()
    })
}
fn options(text: &str, noise: &str, content: &str, at: &mut usize) -> Option<()> {
    if !word(noise, content, at, "WITH") {
        return Some(());
    }
    for _ in 0..16 {
        if !["RECOMPILE", "ENCRYPTION", "SCHEMABINDING"]
            .iter()
            .any(|name| word(noise, content, at, name))
        {
            if !word(noise, content, at, "EXECUTE") || !word(noise, content, at, "AS") {
                return None;
            }
            if !["CALLER", "SELF", "OWNER"]
                .iter()
                .any(|name| word(noise, content, at, name))
            {
                *at = crate::tsql_literal::end_at(text, next(content, *at))?;
            }
        }
        *at = next(content, *at);
        if content.as_bytes().get(*at) != Some(&b',') {
            return Some(());
        }
        *at += 1;
    }
    None
}
pub(super) fn scope(
    text: &str,
    noise: &str,
    content: &str,
    batch: usize,
    cursor: usize,
) -> Option<Scope> {
    let (text, noise, content) = (&text[..cursor], &noise[..cursor], &content[..cursor]);
    let (mut at, function) =
        super::tsql_module_scope::prefix(noise, content, next(content, batch))?;
    // Once a module prefix is recognized, a partial/invalid header must not
    // fall back to collecting arbitrary DECLARE tokens from that header.
    let parsed = (|| -> Option<Scope> {
        at = next(content, reference(text, content, next(content, at))?);
        if function {
            if content.as_bytes().get(at) != Some(&b'(') {
                return None;
            }
            at = next(content, at + 1);
        }
        let mut parameters = Vec::new();
        while text.get(at..)?.starts_with('@') {
            let name_start = at;
            if parameters.len() >= 2100 {
                return None;
            }
            let (name, length) = read_parameter_name(&text[at..], "sqlserver")?;
            if !crate::tsql_identifier::is_complete_name(&name) {
                return None;
            }
            at += length;
            word(noise, content, &mut at, "AS");
            let data_type = data_type(text, content, &mut at)?;
            word(noise, content, &mut at, "NULL");
            if content.as_bytes().get(at) == Some(&b'=') {
                at = next(
                    content,
                    crate::tsql_literal::end_at(text, next(content, at + 1))?,
                );
            }
            let scalar = !word(noise, content, &mut at, "READONLY");
            if !function {
                word(noise, content, &mut at, "OUTPUT");
                word(noise, content, &mut at, "OUT");
            }
            parameters.push(Variable {
                name,
                data_type,
                scalar,
                name_start,
                visible_from: cursor,
            });
            at = next(content, at);
            if content.as_bytes().get(at) != Some(&b',') {
                break;
            }
            at = next(content, at + 1);
            if !text.get(at..)?.starts_with('@') {
                return None;
            }
        }
        if function {
            if content.as_bytes().get(at) != Some(&b')') {
                return None;
            }
            at += 1;
            if !word(noise, content, &mut at, "RETURNS") {
                return None;
            }
            if !word(noise, content, &mut at, "TABLE") {
                data_type(text, content, &mut at)?;
            }
        }
        options(text, noise, content, &mut at)?;
        if function {
            word(noise, content, &mut at, "AS");
        } else if !word(noise, content, &mut at, "AS") {
            return None;
        }
        at = next(content, at);
        if function && !keyword_at(noise, at, "BEGIN") && !keyword_at(noise, at, "RETURN") {
            return None;
        }
        if function {
            if let Some(end) = super::tsql_module_scope::function_end(noise, at, cursor) {
                return Some(Scope {
                    body: end,
                    parameters: Vec::new(),
                });
            }
        }
        for parameter in &mut parameters {
            parameter.visible_from = at;
        }
        Some(Scope {
            body: at,
            parameters,
        })
    })();
    Some(parsed.unwrap_or(Scope {
        body: cursor,
        parameters: Vec::new(),
    }))
}
