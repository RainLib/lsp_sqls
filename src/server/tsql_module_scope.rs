//! Linear lexical outlines over existing same-byte masks. Function bodies end
//! independently of their batch; procedure bodies continue until GO/EOF.
use super::execute_context::keyword_at;

fn next(content: &str, at: usize) -> usize {
    at + content[at..]
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(content.len() - at)
}
pub(super) fn prefix(noise: &str, content: &str, mut at: usize) -> Option<(usize, bool)> {
    let word = |at: &mut usize, word: &str| {
        *at = next(content, *at);
        if keyword_at(noise, *at, word) {
            *at += word.len();
            true
        } else {
            false
        }
    };
    if word(&mut at, "CREATE") {
        if word(&mut at, "OR") && !word(&mut at, "ALTER") {
            return None;
        }
    } else if !word(&mut at, "ALTER") {
        return None;
    }
    let function = if word(&mut at, "FUNCTION") {
        true
    } else if word(&mut at, "PROCEDURE") || word(&mut at, "PROC") {
        false
    } else {
        return None;
    };
    Some((at, function))
}
pub(super) fn latest(noise: &str, content: &str, batch: usize, cursor: usize) -> Option<usize> {
    let (noise, content) = (&noise[..cursor], &content[..cursor]);
    let (mut at, mut found) = (batch, None);
    while at < cursor {
        let Some(relative) = noise.as_bytes()[at..]
            .iter()
            .position(|b| matches!(b, b'C' | b'c' | b'A' | b'a'))
        else {
            break;
        };
        let start = at + relative;
        at = start + 1;
        if let Some((end, _)) = prefix(noise, content, start) {
            found = Some(start);
            at = end;
        }
    }
    found
}
pub(super) fn function_end(noise: &str, body: usize, cursor: usize) -> Option<usize> {
    if keyword_at(noise, body, "BEGIN") {
        let mut depth = 0u32;
        for (relative, ch) in noise[body..cursor].char_indices() {
            let at = body + relative;
            if (matches!(ch, 'B' | 'b') && keyword_at(noise, at, "BEGIN"))
                || (matches!(ch, 'C' | 'c') && keyword_at(noise, at, "CASE"))
            {
                depth += 1;
            } else if matches!(ch, 'E' | 'e') && keyword_at(noise, at, "END") {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at + 3);
                }
            }
        }
        return None;
    }
    if !keyword_at(noise, body, "RETURN") {
        return None;
    }
    let query = next(noise, body + 6);
    let parenthesized = noise.as_bytes().get(query) == Some(&b'(');
    let mut depth = 0u32;
    for (relative, ch) in noise[query..cursor].char_indices() {
        let at = query + relative;
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if parenthesized && depth == 0 {
                    return Some(at + 1);
                }
            }
            ';' if depth == 0 => return Some(at + 1),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqlParser;
    #[test]
    fn variable_scopes_outline_protects_nested_case_comments_identifiers_and_unicode() {
        let source="/*😀*/ BEGIN BEGIN SELECT CASE WHEN 1=1 THEN N'END' ELSE [END] END; END; RETURN 1; END; SELECT 2;";
        let noise = SqlParser::mask_sql_noise_for_dialect(source, "sqlserver");
        let body = source.find("BEGIN").unwrap();
        assert_eq!(
            function_end(&noise, body, source.len()),
            Some(source.rfind("END;").unwrap() + 3)
        );
        assert_eq!(
            function_end(&noise, body, source.rfind("END;").unwrap()),
            None
        );
    }
    #[test]
    fn variable_scopes_inline_outline_balances_queries_and_hidden_delimiters() {
        for body in [
            "RETURN(WITH p AS(SELECT N');' AS id) SELECT id FROM p); SELECT 2;",
            "RETURN SELECT COALESCE(1,2) AS id; SELECT 2;",
        ] {
            let noise = SqlParser::mask_sql_noise_for_dialect(body, "sqlserver");
            let expected =
                body.rfind("; SELECT").unwrap() + usize::from(!body.starts_with("RETURN("));
            assert_eq!(function_end(&noise, 0, body.len()), Some(expected));
        }
    }
    #[test]
    fn variable_scopes_latest_module_skips_parameter_words_and_opaque_text() {
        let source="DECLARE @CREATE int; SELECT N'CREATE PROC app.fake'; /*ALTER FUNCTION fake*/ CREATE OR ALTER FUNCTION app.f(@x int) RETURNS int AS BEGIN RETURN @x; END;";
        let noise = SqlParser::mask_sql_noise_for_dialect(source, "sqlserver");
        let content = SqlParser::mask_sql_operand_context_for_dialect(source, "sqlserver");
        assert_eq!(
            latest(&noise, &content, 0, source.len()),
            Some(source.find("CREATE OR ALTER").unwrap())
        );
    }
}
