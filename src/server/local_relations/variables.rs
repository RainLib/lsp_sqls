//! One inference-local declaration snapshot. Children retain explicit byte
//! provenance through the statement mask and borrowed query slices.
use super::expression_types::ExpressionType;
use crate::server::tsql_types::scalar_type;
use crate::{parser::SqlParser, placeholder::SqlPlaceholderDialect};
use std::{collections::HashMap, ops::Range};

struct Bindings {
    batch: Range<usize>,
    values: HashMap<String, Binding>,
}
struct Binding {
    raw_type: String,
    name_start: usize,
    visible_from: usize,
    data_type: std::cell::OnceCell<Option<String>>,
}
#[derive(Clone, Copy)]
pub(super) struct Origin<'a> {
    snapshot: &'a Snapshot<'a>,
    anchor: &'a str,
}
pub(super) struct Snapshot<'a> {
    source: &'a str,
    cursor: usize,
    value: std::cell::OnceCell<Option<Bindings>>,
    tree: Option<&'a tree_sitter::Tree>,
    parsed_tree: std::cell::OnceCell<Option<tree_sitter::Tree>>,
}
impl<'a> Snapshot<'a> {
    pub(super) fn new(
        source: &'a str,
        cursor: usize,
        dialect: Option<&str>,
        tree: Option<&'a tree_sitter::Tree>,
    ) -> Option<Self> {
        (SqlPlaceholderDialect::for_name(dialect) == SqlPlaceholderDialect::SqlServer
            && source.len() <= super::LOCAL_RELATION_SCAN_MAX_BYTES
            && source.as_bytes().contains(&b'@'))
        .then_some(Self {
            source,
            cursor,
            value: Default::default(),
            tree,
            parsed_tree: Default::default(),
        })
    }
    pub(super) fn origin<'b>(&'b self, anchor: &'b str) -> Option<Origin<'b>> {
        (anchor.len() == self.source.len()).then_some(Origin {
            snapshot: self,
            anchor,
        })
    }
    fn resolve_type(&self, name: &str, binding: &Binding) -> Option<String> {
        let ty = scalar_type(&binding.raw_type)?;
        let tree = self.tree.or_else(|| {
            self.parsed_tree
                .get_or_init(|| {
                    let mut parser =
                        SqlParser::new_with_placeholder_dialect(SqlPlaceholderDialect::SqlServer);
                    parser.parse_syntax_tree(self.source)
                })
                .as_ref()
        })?;
        let source = self.source;
        let mut node = tree
            .root_node()
            .named_descendant_for_byte_range(binding.name_start, binding.name_start)?;
        for _ in 0..16 {
            if matches!(
                node.kind(),
                "tsql_declaration" | "tsql_procedure_parameter" | "tsql_function_parameter"
            ) {
                break;
            }
            let Some(parent) = node.parent() else {
                break;
            };
            node = parent;
        }
        if !matches!(
            node.kind(),
            "tsql_declaration" | "tsql_procedure_parameter" | "tsql_function_parameter"
        ) || node.has_error()
            || node.child_by_field_name("name").is_none_or(|node| {
                node.start_byte() != binding.name_start || &source[node.byte_range()] != name
            })
            || node
                .named_children(&mut node.walk())
                .any(|child| child.kind() == "array_size_definition")
            || node
                .child_by_field_name("type")
                .and_then(|node| scalar_type(&source[node.byte_range()]))
                .as_ref()
                != Some(&ty)
            || (node.kind() == "tsql_declaration"
                && matches!(ty.as_str(), "text" | "ntext" | "image"))
            || node.child_by_field_name("default").is_some_and(|value| {
                let raw = source[value.byte_range()].trim();
                crate::tsql_literal::end_at(raw, 0) != Some(raw.len())
            })
        {
            return None;
        }
        Some(ty)
    }
}
impl Bindings {
    fn new(source: &str, cursor: usize) -> Option<Self> {
        let noise = SqlParser::mask_sql_noise_for_dialect(source, "sqlserver");
        let content = SqlParser::mask_sql_operand_context_for_dialect(source, "sqlserver");
        let variables = crate::server::tsql_variables::declared(source, &noise, &content, cursor);
        // Unknown declarations past a hard budget may duplicate an earlier
        // name. An incomplete snapshot cannot prove a unique type.
        if variables.is_incomplete || !variables.variables.iter().any(|v| v.scalar) {
            return None;
        }
        let values = variables
            .variables
            .into_iter()
            .filter(|v| v.scalar)
            .map(|v| {
                (
                    v.name,
                    Binding {
                        raw_type: v.data_type,
                        name_start: v.name_start,
                        visible_from: v.visible_from,
                        data_type: Default::default(),
                    },
                )
            })
            .collect();
        Some(Self {
            batch: crate::server::tsql_variables::batch_range(&noise, cursor),
            values,
        })
    }
}
impl Origin<'_> {
    pub(super) fn lookup(&self, query: &str, at: usize, name: &str) -> Option<ExpressionType> {
        if !crate::tsql_identifier::is_complete_name(name) {
            return None;
        }
        let start = query
            .as_ptr()
            .addr()
            .checked_sub(self.anchor.as_ptr().addr())?;
        let end = start.checked_add(query.len())?;
        if self.anchor.get(start..end)?.as_ptr() != query.as_ptr() {
            return None;
        }
        let at = start.checked_add(at)?;
        if self.snapshot.source.get(at..at.checked_add(name.len())?) != Some(name) {
            return None;
        }
        let bindings = self
            .snapshot
            .value
            .get_or_init(|| Bindings::new(self.snapshot.source, self.snapshot.cursor))
            .as_ref()?;
        if !bindings.batch.contains(&at) {
            return None;
        }
        let binding = bindings.values.get(name)?;
        if at < binding.visible_from {
            return None;
        }
        let data_type = binding
            .data_type
            .get_or_init(|| self.snapshot.resolve_type(name, binding))
            .as_ref()?;
        Some(ExpressionType {
            data_type: data_type.clone(),
            nullable: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variable_types_validate_scalar_modifiers_without_guessing_udts() {
        for ty in [
            "int",
            "nvarchar(MAX)",
            "decimal(/*x*/18,2)",
            "datetime2(7)",
            "uniqueidentifier",
        ] {
            assert!(scalar_type(ty).is_some(), "{ty}");
        }
        for ty in [
            "int(5)",
            "nvarchar(4001)",
            "decimal(2,4)",
            "float(54)",
            "char(max)",
            "app.Type",
            "[int]",
            "unknown",
            "cursor",
            "table",
        ] {
            assert!(scalar_type(ty).is_none(), "{ty}");
        }
    }
    #[test]
    fn variable_types_snapshot_is_lazy_and_checks_real_slice_batch_and_declaration_order() {
        let source = "SELECT @v; DECLARE @v bigint; SELECT @v;\nGO\nDECLARE @v int; SELECT @v;";
        let snapshot = Snapshot::new(source, source.len(), Some("sqlserver"), None).unwrap();
        assert!(snapshot.value.get().is_none());
        let origin = snapshot.origin(source).unwrap();
        assert!(origin
            .lookup(source, source.find("@v").unwrap(), "@v")
            .is_none());
        let last = source.rfind("@v").unwrap();
        assert_eq!(origin.lookup(source, last, "@v").unwrap().data_type, "int");
        let copied = source.to_owned();
        assert!(origin.lookup(&copied, last, "@v").is_none());
        assert!(snapshot.value.get().is_some());
        let source = "SELECT @v; DECLARE @v int; SELECT @v;";
        let snapshot = Snapshot::new(source, source.len(), Some("tsql"), None).unwrap();
        let origin = snapshot.origin(source).unwrap();
        assert!(origin
            .lookup(source, source.find("@v").unwrap(), "@v")
            .is_none());
        assert_eq!(
            origin
                .lookup(source, source.rfind("@v").unwrap(), "@v")
                .unwrap()
                .data_type,
            "int"
        );
    }
    #[test]
    fn variable_types_only_validate_referenced_declarations_in_shared_snapshot() {
        let mut source = (0..2100)
            .map(|i| format!("DECLARE @v{i:04} int; "))
            .collect::<String>();
        source.push_str("SELECT @v2099,@v0000,@v2099;");
        let snapshot = Snapshot::new(&source, source.len(), Some("sqlserver"), None).unwrap();
        let origin = snapshot.origin(&source).unwrap();
        assert!(snapshot.parsed_tree.get().is_none());
        assert_eq!(
            origin
                .lookup(&source, source.rfind("@v2099").unwrap(), "@v2099")
                .unwrap()
                .data_type,
            "int"
        );
        let initialized = || {
            snapshot
                .value
                .get()
                .unwrap()
                .as_ref()
                .unwrap()
                .values
                .values()
                .filter(|v| v.data_type.get().is_some())
                .count()
        };
        assert_eq!(initialized(), 1);
        let tree = snapshot.parsed_tree.get().unwrap().as_ref().unwrap() as *const _;
        assert_eq!(
            origin
                .lookup(&source, source.rfind("@v0000").unwrap(), "@v0000")
                .unwrap()
                .data_type,
            "int"
        );
        assert_eq!(initialized(), 2);
        assert_eq!(
            tree,
            snapshot.parsed_tree.get().unwrap().as_ref().unwrap() as *const _
        );
        assert_eq!(
            origin
                .lookup(&source, source.rfind("@v2099").unwrap(), "@v2099")
                .unwrap()
                .data_type,
            "int"
        );
        assert_eq!(initialized(), 2);
    }
    #[test]
    fn variable_types_incomplete_snapshot_never_claims_a_unique_type() {
        let mut source = (0..4097)
            .map(|i| format!("DECLARE @v{i:04} int; "))
            .collect::<String>();
        source.push_str("SELECT @v0000;");
        let snapshot = Snapshot::new(&source, source.len(), Some("mssql"), None).unwrap();
        assert!(snapshot
            .origin(&source)
            .unwrap()
            .lookup(&source, source.rfind("@v0000").unwrap(), "@v0000")
            .is_none());
    }
}
