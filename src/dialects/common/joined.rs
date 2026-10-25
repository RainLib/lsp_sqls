//! Bind source-owned join layouts to catalog/local rows once per request.
//! Physical rows remain borrowed; grouped and null-extended rows own their
//! resulting columns. Unknown arity never becomes a partial star shape.
use super::references::row_reference;
use crate::{
    parser::{JoinExpr, JoinKind, RelationAlias},
    schema::{Column, Schema, Table},
};
use std::{borrow::Cow, collections::HashSet};

pub(crate) struct JoinedRow<'a> {
    pub reference: String,
    pub alias: Option<RelationAlias>,
    pub table: Option<Cow<'a, Table>>,
}
pub(crate) struct JoinedSources<'a> {
    pub rows: Vec<JoinedRow<'a>>,
    pub output: Option<Table>,
}
const MAX_JOIN_COLUMNS: usize = 4096;
#[cfg(test)]
#[path = "joined_tests.rs"]
mod tests;

pub(crate) fn validate_join_profile(
    mut result: crate::parser::ParseResult,
    profile: &str,
) -> crate::parser::ParseResult {
    if !matches!(profile, "sqlserver" | "hive" | "clickhouse") {
        return result;
    }
    if !result
        .source
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|word| word.eq_ignore_ascii_case("NATURAL") || word.eq_ignore_ascii_case("USING"))
    {
        return result;
    }
    if let Some(tree) = &result.tree {
        let positions = crate::position::SourcePositions::new(&result.source);
        let position = |at: usize| {
            positions.as_ref().map_or_else(
                || crate::position::lsp_position_at_end(&result.source[..at]),
                |p| p.offset(&result.source, at),
            )
        };
        let mut cursor = tree.walk();
        'walk: loop {
            let n = cursor.node();
            let natural = n.kind() == "keyword_natural";
            let using = n.kind() == "keyword_using" && matches!(profile, "sqlserver" | "hive");
            if (natural || using) && n.parent().is_some_and(|p| p.kind() == "join") {
                result.diagnostics.push(tower_lsp::lsp_types::Diagnostic {
                    range: tower_lsp::lsp_types::Range::new(
                        position(n.start_byte()),
                        position(n.end_byte()),
                    ),
                    severity: Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR),
                    code: Some(tower_lsp::lsp_types::NumberOrString::String(
                        "UNSUPPORTED_DIALECT_SYNTAX".into(),
                    )),
                    source: Some("sql-dialect".into()),
                    message: format!(
                        "{} is not supported by the {profile} profile",
                        if natural {
                            "NATURAL JOIN"
                        } else {
                            "JOIN ... USING"
                        }
                    ),
                    ..Default::default()
                });
            }
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
    }
    result
}
fn joined_output(columns: Vec<Column>) -> Table {
    Table {
        name: String::new(),
        object_type: Some("JOIN OUTPUT".into()),
        columns,
        ..Default::default()
    }
}
fn joined_key(name: &str, fold: bool) -> String {
    if fold {
        name.to_ascii_lowercase()
    } else {
        name.into()
    }
}
fn joined_type(left: &str, right: &str, dialect: &str) -> String {
    if left == right || left.eq_ignore_ascii_case(right) {
        return left.into();
    }
    if dialect == "postgres" {
        let rank = |s: &str| match s.to_ascii_lowercase().as_str() {
            "smallint" | "int2" => Some(0),
            "integer" | "int" | "int4" => Some(1),
            "bigint" | "int8" => Some(2),
            "numeric" | "decimal" => Some(3),
            "real" | "float4" => Some(4),
            "double precision" | "float8" => Some(5),
            _ => None,
        };
        if let (Some(l), Some(r)) = (rank(left), rank(right)) {
            return [
                "smallint",
                "integer",
                "bigint",
                "numeric",
                "real",
                "double precision",
            ][l.max(r)]
            .into();
        }
    }
    "unknown".into()
}
fn joined_null_extend(rows: &mut [JoinedRow<'_>]) {
    for row in rows {
        if let Some(table) = &mut row.table {
            for column in &mut table.to_mut().columns {
                column.nullable = true;
            }
        }
    }
}
fn joined_combine<'a>(
    mut left: JoinedSources<'a>,
    mut right: JoinedSources<'a>,
    kind: JoinKind,
    keys: &[String],
    natural: bool,
    dialect: &str,
) -> JoinedSources<'a> {
    let output = match (left.output.take(), right.output.take()) {
        (Some(l), Some(r)) if l.columns.len() + r.columns.len() <= MAX_JOIN_COLUMNS => {
            let fold = dialect != "postgres";
            let mut requested = keys.to_vec();
            if natural {
                requested = l
                    .columns
                    .iter()
                    .filter(|c| {
                        !c.hidden
                            && r.columns.iter().any(|v| {
                                !v.hidden && joined_key(&v.name, fold) == joined_key(&c.name, fold)
                            })
                    })
                    .map(|c| c.name.clone())
                    .collect();
            }
            let mut merged = HashSet::new();
            let mut columns = Vec::new();
            let mut valid = true;
            for name in requested {
                let key = joined_key(&name, fold);
                if merged.contains(&key) {
                    valid = false;
                    break;
                }
                let lc: Vec<_> = l
                    .columns
                    .iter()
                    .filter(|c| joined_key(&c.name, fold) == key)
                    .collect();
                let rc: Vec<_> = r
                    .columns
                    .iter()
                    .filter(|c| joined_key(&c.name, fold) == key)
                    .collect();
                if lc.len() != 1 || rc.len() != 1 {
                    valid = false;
                    break;
                }
                let mut c = if kind == JoinKind::Right {
                    rc[0].clone()
                } else {
                    lc[0].clone()
                };
                c.data_type = joined_type(&lc[0].data_type, &rc[0].data_type, dialect);
                c.nullable = match kind {
                    JoinKind::Inner => false,
                    JoinKind::Left => lc[0].nullable,
                    JoinKind::Right => rc[0].nullable,
                    JoinKind::Full => lc[0].nullable || rc[0].nullable,
                    JoinKind::Cross => true,
                };
                c.primary_key = false;
                c.unique = false;
                c.indexed = false;
                c.auto_increment = false;
                c.default_value = None;
                c.source_location = None;
                columns.push(c);
                merged.insert(key);
            }
            for mut c in l.columns {
                if !merged.contains(&joined_key(&c.name, fold)) {
                    c.nullable |= matches!(kind, JoinKind::Right | JoinKind::Full);
                    columns.push(c);
                }
            }
            for mut c in r.columns {
                if !merged.contains(&joined_key(&c.name, fold)) {
                    c.nullable |= matches!(kind, JoinKind::Left | JoinKind::Full);
                    columns.push(c);
                }
            }
            valid.then(|| joined_output(columns))
        }
        _ => None,
    };
    if matches!(kind, JoinKind::Right | JoinKind::Full) {
        joined_null_extend(&mut left.rows);
    }
    if matches!(kind, JoinKind::Left | JoinKind::Full) {
        joined_null_extend(&mut right.rows);
    }
    left.rows.extend(right.rows);
    JoinedSources {
        rows: left.rows,
        output,
    }
}
#[cfg(test)]
pub(crate) fn resolve_joined_sources<'a>(
    expr: &JoinExpr,
    schema: &'a Schema,
    dialect: &str,
) -> JoinedSources<'a> {
    resolve_joined_sources_with_metadata(expr, schema, dialect, None)
}

pub(crate) fn resolve_joined_sources_with_metadata<'a>(
    expr: &JoinExpr,
    schema: &'a Schema,
    dialect: &str,
    manager: Option<&crate::schema::SchemaManager>,
) -> JoinedSources<'a> {
    match expr {
        JoinExpr::Empty => JoinedSources {
            rows: Vec::new(),
            output: Some(joined_output(Vec::new())),
        },
        JoinExpr::Unknown => JoinedSources {
            rows: vec![JoinedRow {
                reference: String::new(),
                alias: None,
                table: None,
            }],
            output: None,
        },
        JoinExpr::Relation {
            reference,
            alias,
            local,
            ..
        } => {
            let aliases: Vec<_> = alias.iter().cloned().collect();
            let target = alias.as_ref().map_or(reference.as_str(), |a| {
                row_reference(schema, &aliases, &a.sql)
            });
            let table = (!reference.is_empty())
                .then(|| crate::relation_bindings::projection_table(schema, manager, target))
                .flatten()
                .filter(|t| !local || crate::relation_bindings::query_local(t));
            let output = table
                .as_ref()
                .filter(|t| t.columns.len() <= MAX_JOIN_COLUMNS)
                .map(|t| joined_output(t.columns.clone()));
            JoinedSources {
                rows: vec![JoinedRow {
                    reference: reference.clone(),
                    alias: alias.clone(),
                    table,
                }],
                output,
            }
        }
        JoinExpr::Join {
            left,
            right,
            kind,
            keys,
            natural,
            ..
        } => joined_combine(
            resolve_joined_sources_with_metadata(left, schema, dialect, manager),
            resolve_joined_sources_with_metadata(right, schema, dialect, manager),
            *kind,
            keys,
            *natural,
            dialect,
        ),
        JoinExpr::Group {
            input,
            alias,
            columns,
            ..
        } => {
            let mut result = resolve_joined_sources_with_metadata(input, schema, dialect, manager);
            let Some(alias) = alias else {
                return result;
            };
            let table = result.output.take().and_then(|mut t| {
                if columns.len() > t.columns.len() {
                    return None;
                }
                for (column, name) in t.columns.iter_mut().zip(columns) {
                    column.name = name.clone();
                    column.source_location = None;
                }
                for c in &mut t.columns {
                    c.primary_key = false;
                    c.unique = false;
                    c.indexed = false;
                    c.auto_increment = false;
                    c.default_value = None;
                }
                t.name = alias.name.clone();
                t.object_type = Some("JOINED TABLE".into());
                Some(t)
            });
            let output = table.as_ref().map(|t| joined_output(t.columns.clone()));
            let row = JoinedRow {
                reference: alias.relation.clone(),
                alias: Some(alias.clone()),
                table: table.map(Cow::Owned),
            };
            let mut rows = if dialect == "sqlite" {
                result.rows
            } else {
                Vec::new()
            };
            rows.push(row);
            JoinedSources { rows, output }
        }
    }
}
