//! Query-local overlays coexist with catalog objects. A bare name can be
//! shadowed; catalog-qualified names never bind to CTEs or derived aliases.
use crate::{
    parser::SqlParser,
    schema::{Schema, Table},
};
mod lookup;
pub(crate) use lookup::{lookup_table, RelationLookup};

pub(crate) fn query_local(table: &Table) -> bool {
    matches!(
        table.object_type.as_deref(),
        Some(
            "COMMON TABLE EXPRESSION"
                | "DERIVED TABLE"
                | "CORRELATED TABLE"
                | "TABLE FUNCTION"
                | "JOINED TABLE"
        )
    )
}

pub(crate) fn local(table: &Table) -> bool {
    query_local(table) || table.object_type.as_deref() == Some("TEMPORARY TABLE")
}

pub(crate) fn catalog_table_matches(schema: &Schema, table: &Table, reference: &str) -> bool {
    schema.omitted_table_matches(reference, &table.name)
        || SqlParser::table_name_matches_with_namespace(
            reference,
            schema.server.as_deref(),
            schema.catalog.as_deref(),
            &schema.database,
            &table.name,
        )
}

pub(crate) fn table_matches(schema: &Schema, table: &Table, reference: &str) -> bool {
    if !catalog_table_matches(schema, table, reference) {
        return false;
    }
    let qualified = SqlParser::identifier_qualifier(reference).is_some();
    if query_local(table) {
        return !qualified;
    }
    qualified
        || !schema.tables.iter().any(|candidate| {
            query_local(candidate) && candidate.name.eq_ignore_ascii_case(&table.name)
        })
}

/// Resolve explicitly qualified inputs independently of the active document
/// schema. External rows are owned because manager snapshots are temporary.
pub(crate) fn projection_table<'a>(
    schema: &'a Schema,
    manager: Option<&crate::schema::SchemaManager>,
    reference: &str,
) -> Option<std::borrow::Cow<'a, Table>> {
    lookup_table(Some(schema), manager, reference)
}

pub(crate) fn install(schema: &mut Schema, table: Table) {
    // Preserve historical temporary-table replacement until its actual engine
    // namespace is modeled. Ordinary query overlays keep physical metadata.
    let temporary = table.object_type.as_deref() == Some("TEMPORARY TABLE");
    schema.tables.retain(|candidate| {
        !candidate.name.eq_ignore_ascii_case(&table.name) || (!local(candidate) && !temporary)
    });
    schema.tables.insert(0, table);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Column, SchemaId};
    fn schema() -> Schema {
        Schema {
            id: SchemaId::new(),
            server: None,
            namespace: None,
            catalog: Some("catalog".into()),
            database: "app".into(),
            tables: vec![table("BASE TABLE", "physical")],
            functions: vec![],
            source_uri: None,
            server_version: None,
        }
    }
    fn table(kind: &str, column: &str) -> Table {
        Table {
            name: "users".into(),
            object_type: Some(kind.into()),
            columns: vec![Column {
                name: column.into(),
                data_type: "text".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    #[test]
    fn query_only_names_shadow_bare_references_and_preserve_catalog_objects() {
        let mut schema = schema();
        install(&mut schema, table("COMMON TABLE EXPRESSION", "local"));
        assert_eq!(schema.tables.len(), 2);
        let local = &schema.tables[0];
        let physical = &schema.tables[1];
        assert!(table_matches(&schema, local, "users"));
        assert!(!table_matches(&schema, physical, "users"));
        for reference in ["app.users", "catalog.app.users", "[catalog].[app].[users]"] {
            assert!(!table_matches(&schema, local, reference));
            assert!(table_matches(&schema, physical, reference));
        }
        assert!(!table_matches(&schema, physical, "other.app.users"));
        assert!(!table_matches(&schema, physical, "catalog.other.users"));
    }
    #[test]
    fn replacing_an_inner_query_shape_keeps_physical_metadata_intact() {
        let original = schema();
        let mut view = original.clone();
        install(&mut view, table("COMMON TABLE EXPRESSION", "outer"));
        install(&mut view, table("DERIVED TABLE", "inner"));
        assert_eq!(view.tables.len(), 2);
        assert_eq!(view.tables[0].columns[0].name, "inner");
        assert_eq!(view.tables[1].columns[0].name, "physical");
        assert_eq!(original.tables.len(), 1);
        assert_eq!(original.tables[0].columns[0].name, "physical");
    }
    #[test]
    fn invocation_and_correlation_aliases_are_not_schema_objects() {
        for kind in ["DERIVED TABLE", "CORRELATED TABLE", "TABLE FUNCTION"] {
            let mut schema = schema();
            install(&mut schema, table(kind, "local"));
            assert!(table_matches(&schema, &schema.tables[0], "users"));
            assert!(!table_matches(&schema, &schema.tables[0], "app.users"));
        }
    }
    #[test]
    fn temporary_table_replacement_keeps_its_existing_behavior() {
        let mut schema = schema();
        install(&mut schema, table("TEMPORARY TABLE", "temporary"));
        assert_eq!(schema.tables.len(), 1);
        assert!(table_matches(&schema, &schema.tables[0], "users"));
        assert!(table_matches(&schema, &schema.tables[0], "app.users"));
        install(&mut schema, table("COMMON TABLE EXPRESSION", "cte"));
        assert_eq!(schema.tables.len(), 1);
        assert_eq!(schema.tables[0].columns[0].name, "cte");
    }
}
