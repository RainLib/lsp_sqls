//! Explicit omitted-schema lookup, scoped to an exact server and catalog.
//! Missing snapshots never prove absence and therefore never trigger dbo.
use super::{Schema, SchemaManager};
use crate::parser::SqlParser;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceContext {
    pub default_schema: String,
    #[serde(default)]
    pub tables_complete: bool,
    /// Ephemeral proof attached only after an ordered manager lookup. Clients
    /// cannot supply this through JSON or preserve it in a catalog snapshot.
    #[serde(skip)]
    pub(crate) resolved_references: Vec<String>,
}

fn omitted(reference: &str) -> Option<Vec<String>> {
    let parts = SqlParser::identifier_parts(reference);
    if !(3..=4).contains(&parts.len())
        || !parts[parts.len() - 2].is_empty()
        || parts
            .iter()
            .enumerate()
            .any(|(i, p)| i != parts.len() - 2 && p.is_empty())
    {
        return None;
    }
    Some(parts)
}

fn same_namespace(schema: &Schema, parts: &[String]) -> bool {
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let server = if parts.len() == 4 {
        schema.server.as_deref().is_some_and(|s| eq(s, &parts[0]))
    } else {
        schema.server.is_none()
    };
    server
        && schema
            .catalog
            .as_deref()
            .is_some_and(|c| eq(c, &parts[parts.len() - 3]))
}

impl Schema {
    pub(crate) fn omitted_table_matches(&self, reference: &str, table: &str) -> bool {
        let Some(parts) = omitted(reference) else {
            return false;
        };
        let Some(context) = &self.namespace else {
            return false;
        };
        same_namespace(self, &parts)
            && parts.last().is_some_and(|n| n.eq_ignore_ascii_case(table))
            && context.resolved_references.iter().any(|r| {
                SqlParser::normalize_identifier(r)
                    .eq_ignore_ascii_case(&SqlParser::normalize_identifier(reference))
            })
    }
}

impl SchemaManager {
    pub(crate) fn resolve_omitted_schema(&self, reference: &str) -> Option<Schema> {
        let parts = omitted(reference)?;
        let mut effective: Option<String> = None;
        for candidate in self.schemas.iter() {
            if !same_namespace(&candidate, &parts) {
                continue;
            }
            let Some(context) = &candidate.namespace else {
                continue;
            };
            let name = &context.default_schema;
            if name.trim().is_empty() {
                return None;
            }
            if effective
                .as_ref()
                .is_some_and(|previous| !previous.eq_ignore_ascii_case(name))
            {
                return None;
            }
            effective = Some(name.clone());
        }
        let default = effective?;
        let primary = self.find_unique(|s| {
            same_namespace(s, &parts) && s.database.eq_ignore_ascii_case(&default)
        })?;
        let name = parts.last()?;
        let has = |s: &Schema| {
            s.tables
                .iter()
                .any(|t| !crate::relation_bindings::local(t) && t.name.eq_ignore_ascii_case(name))
        };
        let mut target = if has(&primary) {
            primary
        } else {
            if default.eq_ignore_ascii_case("dbo") || !primary.namespace.as_ref()?.tables_complete {
                return None;
            }
            self.find_unique(|s| {
                same_namespace(s, &parts) && s.database.eq_ignore_ascii_case("dbo") && has(s)
            })?
        };
        target.namespace = Some(NamespaceContext {
            default_schema: default,
            tables_complete: false,
            resolved_references: vec![reference.to_string()],
        });
        Some(target)
    }
}
