//! Column-level keywords only: names, CHECK/default expressions and opaque SQL
//! must not become uniqueness/nullability evidence. Masks retain byte offsets.
use super::syntax::{read_local_identifier_after, skip_sql_trivia};
#[derive(Default)]
pub(super) struct Flags {
    pub not_null: bool,
    pub primary_key: bool,
    pub unique: bool,
}
pub(super) fn flags(source: &str, masked: &str, start: usize) -> Flags {
    let mut result = Flags::default();
    let (mut at, mut depth, mut cases) = (start, 0usize, 0usize);
    let mut previous: Option<(&str, usize)> = None;
    while at < masked.len() {
        let ch = masked[at..].chars().next().unwrap();
        match ch {
            '(' => {
                depth += 1;
                previous = None;
                at += 1;
                continue;
            }
            ')' => {
                depth = depth.saturating_sub(1);
                previous = None;
                at += 1;
                continue;
            }
            _ if ch.is_whitespace() => {
                at += ch.len_utf8();
                continue;
            }
            _ => {}
        }
        let Some((_, word_start, end)) = read_local_identifier_after(masked, at) else {
            previous = None;
            at += ch.len_utf8();
            continue;
        };
        let word = &masked[word_start..end];
        at = end;
        if depth != 0 {
            continue;
        }
        if word.eq_ignore_ascii_case("CASE") {
            cases += 1;
            previous = None;
            continue;
        }
        if cases != 0 {
            if word.eq_ignore_ascii_case("END") {
                cases -= 1;
            }
            continue;
        }
        if word.eq_ignore_ascii_case("CONSTRAINT") {
            let name = skip_sql_trivia(source, end);
            if let Some((_, _, name_end)) = read_local_identifier_after(source, name) {
                at = name_end;
            }
            previous = None;
            continue;
        }
        if let Some((prior, _)) =
            previous.filter(|(_, end)| skip_sql_trivia(source, *end) == word_start)
        {
            result.not_null |=
                prior.eq_ignore_ascii_case("NOT") && word.eq_ignore_ascii_case("NULL");
            result.primary_key |=
                prior.eq_ignore_ascii_case("PRIMARY") && word.eq_ignore_ascii_case("KEY");
        }
        result.unique |= word.eq_ignore_ascii_case("UNIQUE");
        previous = Some((word, end));
    }
    result
}
