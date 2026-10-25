//! Request-owned reference lookup. Explicit paths never borrow an unknown
//! catalog or server; schema-only paths retain the active catalog context.
use super::{local, table_matches};
use crate::{
    parser::SqlParser,
    schema::{Schema, SchemaManager, Table},
};
use std::{borrow::Cow, collections::HashMap};

fn namespace_matches(candidate: &Schema, active: Option<&Schema>, parts: &[String]) -> bool {
    let eq = |a: Option<&str>, b: Option<&str>| match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    };
    match parts {
        [_, _] => active.is_some_and(|a| {
            eq(candidate.server.as_deref(), a.server.as_deref())
                && eq(candidate.catalog.as_deref(), a.catalog.as_deref())
        }),
        [catalog, _, _] => {
            eq(
                candidate.server.as_deref(),
                active.and_then(|a| a.server.as_deref()),
            ) && eq(candidate.catalog.as_deref(), Some(catalog))
        }
        [server, catalog, _, _] => {
            eq(candidate.server.as_deref(), Some(server))
                && eq(candidate.catalog.as_deref(), Some(catalog))
        }
        [_] => true,
        _ => false,
    }
}

pub(crate) fn lookup_table<'a>(
    active: Option<&'a Schema>,
    manager: Option<&SchemaManager>,
    reference: &str,
) -> Option<Cow<'a, Table>> {
    let parts = SqlParser::identifier_parts(reference);
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    if let Some(schema) = active {
        if parts.len() == 1 {
            if let Some(table) = schema
                .tables
                .iter()
                .find(|t| local(t) && table_matches(schema, t, reference))
            {
                return Some(Cow::Borrowed(table));
            }
            if schema.namespace.is_some() && manager.is_some() {
                let mut path = Vec::new();
                if let Some(server) = &schema.server {
                    path.push(server.clone());
                }
                path.extend([schema.catalog.clone()?, String::new(), parts[0].clone()]);
                let resolved = manager?
                    .resolve_omitted_schema(&SqlParser::identifier_path_from_parts(&path))?;
                return resolved
                    .tables
                    .iter()
                    .find(|t| !local(t) && t.name.eq_ignore_ascii_case(&parts[0]))
                    .cloned()
                    .map(Cow::Owned);
            }
        }
        if (!parts.iter().any(String::is_empty) || manager.is_none())
            && namespace_matches(schema, active, &parts)
        {
            if let Some(table) = schema
                .tables
                .iter()
                .find(|t| table_matches(schema, t, reference))
            {
                return Some(Cow::Borrowed(table));
            }
        }
    }
    let manager = manager?;
    if parts.iter().any(String::is_empty) {
        let external = manager.resolve_omitted_schema(reference)?;
        return external
            .tables
            .iter()
            .find(|t| !local(t) && table_matches(&external, t, reference))
            .cloned()
            .map(Cow::Owned);
    }
    if parts.len() < 2 || parts.len() == 2 && active.and_then(|s| s.catalog.as_deref()).is_none() {
        return None;
    }
    manager
        .find_unique_table(
            |s| namespace_matches(s, active, &parts),
            |s, t| !local(t) && table_matches(s, t, reference),
        )
        .map(Cow::Owned)
}

/// Includes hits and misses; no persistent cache can outlive schema refresh.
pub(crate) struct RelationLookup<'a> {
    active: Option<&'a Schema>,
    manager: Option<&'a SchemaManager>,
    cache: HashMap<String, Option<Cow<'a, Table>>>,
}
impl<'a> RelationLookup<'a> {
    pub(crate) fn new(active: Option<&'a Schema>, manager: Option<&'a SchemaManager>) -> Self {
        Self {
            active,
            manager,
            cache: HashMap::new(),
        }
    }
    pub(crate) fn table(&mut self, reference: &str) -> Option<&Table> {
        let key = SqlParser::normalize_identifier(reference);
        if !self.cache.contains_key(&key) {
            if self.cache.len() >= 64 {
                return None;
            }
            let value = lookup_table(self.active, self.manager, reference);
            self.cache.insert(key.clone(), value);
        }
        self.cache.get(&key)?.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn schema(catalog: &str) -> Schema {
        serde_json::from_value(serde_json::json!({"id":crate::schema::SchemaId::new(),"catalog":catalog,"database":"app","tables":[{"name":"ledger","columns":[{"name":"value","data_type":"int","nullable":true}]}],"functions":[]})).unwrap()
    }
    #[test]
    fn hit_and_miss_memoization_is_owned_by_the_request() {
        let active = schema("db");
        let manager = SchemaManager::new();
        let external = schema("other");
        let id = manager.register(external);
        let mut lookup = RelationLookup::new(Some(&active), Some(&manager));
        assert_eq!(
            lookup.table("other.app.ledger").unwrap().columns[0].name,
            "value"
        );
        assert!(lookup.table("new.app.ledger").is_none());
        let mut updated = manager.get(id).unwrap();
        updated.tables[0].columns[0].name = "updated".into();
        manager.update(id, updated);
        manager.register(schema("new"));
        assert_eq!(
            lookup.table("other.app.ledger").unwrap().columns[0].name,
            "value"
        );
        assert!(lookup.table("new.app.ledger").is_none());
        let mut fresh = RelationLookup::new(Some(&active), Some(&manager));
        assert_eq!(
            fresh.table("other.app.ledger").unwrap().columns[0].name,
            "updated"
        );
        assert!(fresh.table("new.app.ledger").is_some());
    }
    #[test]
    fn misses_count_toward_budget_and_unidentified_catalogs_cannot_bind() {
        let mut active = schema("db");
        active.catalog = None;
        let manager = SchemaManager::new();
        manager.register(schema("other"));
        assert!(lookup_table(Some(&active), Some(&manager), "missing.app.ledger").is_none());
        let mut lookup = RelationLookup::new(Some(&active), Some(&manager));
        for i in 0..64 {
            assert!(lookup.table(&format!("missing{i}.app.ledger")).is_none());
        }
        assert_eq!(lookup.cache.len(), 64);
        assert!(lookup.table("ledger").is_none());
        assert_eq!(lookup.cache.len(), 64);
    }
}
