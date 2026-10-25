//! SQL routine identity retains quoted components and its metadata namespace.
use crate::parser::SqlParser;
use crate::schema::{Function, Schema, SchemaManager};
use crate::sql_lexical::lexical_rules_for;
use std::borrow::Cow;

#[derive(Clone, Debug)]
struct NamePart {
    value: String,
    quoted: bool,
}

fn postgres(dialect: &str) -> bool {
    ["postgres", "postgresql", "pgsql", "psql"]
        .iter()
        .any(|name| dialect.eq_ignore_ascii_case(name))
}

impl NamePart {
    fn matches(&self, metadata: &str, dialect: &str) -> bool {
        if postgres(dialect) || dialect.eq_ignore_ascii_case("clickhouse") {
            self.value == metadata
        } else {
            self.value.eq_ignore_ascii_case(metadata)
        }
    }
}

fn read_part(text: &str, start: usize, dialect: &str) -> Option<(NamePart, usize)> {
    let first = text.get(start..)?.chars().next()?;
    let rules = lexical_rules_for(dialect);
    let close = match first {
        '"' if !rules.double_quoted_strings => Some('"'),
        '`' if !postgres(dialect) => Some('`'),
        '[' if rules.bracket_identifiers => Some(']'),
        '\'' | '"' | '`' | '[' => return None,
        _ => None,
    };
    if let Some(close) = close {
        let mut value = String::new();
        let mut chars = text[start + 1..].char_indices().peekable();
        while let Some((offset, ch)) = chars.next() {
            if ch == close {
                if chars.peek().is_some_and(|(_, next)| *next == close) {
                    chars.next();
                    value.push(close);
                } else {
                    return (!value.is_empty()).then_some((
                        NamePart {
                            value,
                            quoted: true,
                        },
                        start + 1 + offset + ch.len_utf8(),
                    ));
                }
            } else if ch == '\\' && rules.backslash_identifiers {
                value.push(chars.next()?.1);
            } else {
                value.push(ch);
            }
        }
        return None;
    }
    if !first.is_alphabetic() && !matches!(first, '_' | '$' | '@' | '#') {
        return None;
    }
    let end = text[start..]
        .char_indices()
        .take_while(|(_, ch)| SqlParser::is_identifier_char(*ch))
        .map(|(offset, ch)| start + offset + ch.len_utf8())
        .last()?;
    let raw = &text[start..end];
    let value = if postgres(dialect) {
        raw.to_ascii_lowercase()
    } else {
        raw.to_string()
    };
    Some((
        NamePart {
            value,
            quoted: false,
        },
        end,
    ))
}

pub(super) fn read_parameter_name(text: &str, dialect: &str) -> Option<(String, usize)> {
    if lexical_rules_for(dialect).placeholder
        == crate::placeholder::SqlPlaceholderDialect::SqlServer
        && text.starts_with('@')
    {
        let end = crate::tsql_identifier::prefix_end(text, 0)?;
        return Some((text[..end].to_owned(), end));
    }
    let (part, end) = read_part(text, 0, dialect)?;
    Some((part.value, end))
}

/// `content` is the existing same-byte-length operand mask. A quoted name has
/// one marker at its opening delimiter, allowing bounded backward navigation.
pub(super) fn routine_reference_before(
    text: &str,
    content: &str,
    mut end: usize,
    dialect: &str,
) -> Option<(String, bool)> {
    let mut parts = Vec::new();
    loop {
        let (last, _) = content
            .get(..end)?
            .char_indices()
            .rev()
            .find(|(_, ch)| !ch.is_whitespace())?;
        let first = text.get(last..)?.chars().next()?;
        let start = if matches!(first, '"' | '`' | '[' | '\'') {
            last
        } else {
            text[..last + first.len_utf8()]
                .char_indices()
                .rev()
                .take_while(|(_, ch)| SqlParser::is_identifier_char(*ch))
                .last()?
                .0
        };
        let (_, name_end) = read_part(text, start, dialect)?;
        if name_end > end {
            return None;
        }
        parts.push(text.get(start..name_end)?);
        let previous = content
            .get(..start)?
            .char_indices()
            .rev()
            .find(|(_, ch)| !ch.is_whitespace());
        let Some((dot, '.')) = previous else {
            parts.reverse();
            let prefix = content.get(..start)?.trim_end();
            let preceding = prefix
                .rsplit(|ch: char| !SqlParser::is_identifier_char(ch))
                .next()
                .unwrap_or("");
            return Some((parts.join("."), preceding.eq_ignore_ascii_case("CALL")));
        };
        if parts.len() >= 3 {
            return None;
        }
        end = dot;
    }
}

pub(super) struct RoutineIdentity {
    parts: Vec<NamePart>,
}

impl RoutineIdentity {
    pub(super) fn parse(reference: &str, dialect: &str) -> Option<Self> {
        let mut parts = Vec::new();
        let mut start = 0;
        loop {
            let (part, end) = read_part(reference, start, dialect)?;
            parts.push(part);
            if end == reference.len() {
                return Some(Self { parts });
            }
            if parts.len() >= 3 || reference.as_bytes().get(end) != Some(&b'.') {
                return None;
            }
            start = end + 1;
        }
    }

    pub(super) fn name(&self) -> &str {
        &self.parts.last().expect("nonempty routine identity").value
    }

    pub(super) fn namespace_matches(&self, schema: &Schema, dialect: &str) -> bool {
        match self.parts.as_slice() {
            [_] => true,
            [namespace, _] => namespace.matches(&schema.database, dialect),
            [catalog, namespace, _] => {
                namespace.matches(&schema.database, dialect)
                    && schema
                        .catalog
                        .as_deref()
                        .is_some_and(|value| catalog.matches(value, dialect))
            }
            _ => false,
        }
    }

    pub(super) fn schema<'a>(
        &self,
        current: Option<&'a Schema>,
        manager: &SchemaManager,
        dialect: &str,
    ) -> Option<Cow<'a, Schema>> {
        let current = current?;
        if self.namespace_matches(current, dialect) {
            return Some(Cow::Borrowed(current));
        }
        // Metadata has no connection ID. Only a known current catalog permits
        // finding another namespace; never scan unrelated catalogs or guess.
        let catalog = current.catalog.as_deref()?;
        manager
            .find_unique(|candidate| {
                candidate.catalog.as_deref() == Some(catalog)
                    && self.namespace_matches(candidate, dialect)
            })
            .map(Cow::Owned)
    }

    pub(super) fn matches(&self, function: &Function, dialect: &str) -> bool {
        if self.syntax_construct(dialect) && self.parts.len() == 1 && !self.parts[0].quoted {
            return false;
        }
        self.parts
            .last()
            .is_some_and(|part| part.matches(&function.name, dialect))
    }

    fn syntax_construct(&self, dialect: &str) -> bool {
        postgres(dialect) && matches!(self.name(), "coalesce" | "greatest" | "least" | "nullif")
    }

    pub(super) fn permits_builtin(&self, dialect: &str) -> bool {
        match self.parts.as_slice() {
            [_] => true,
            [namespace, _] if postgres(dialect) => namespace.matches("pg_catalog", dialect),
            _ => false,
        }
    }

    pub(super) fn matches_builtin(&self, name: &str, dialect: &str) -> bool {
        let part = self.parts.last().expect("nonempty routine identity");
        if postgres(dialect) {
            // These SQL constructs are not ordinary pg_catalog functions.
            part.value == name.to_ascii_lowercase()
                && (!self.syntax_construct(dialect) || self.parts.len() == 1 && !part.quoted)
        } else {
            !part.quoted
        }
    }
}
