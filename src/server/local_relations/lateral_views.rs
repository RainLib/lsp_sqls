//! Hive UDTF rows are installed in source order, using the same joined input
//! bindings as completion and hover. No function result is evaluated here.
use super::{
    hive_types::{kind, Kind},
    install_local_table, local_relation_column, local_relation_table,
    projection::ProjectionSources,
    provenance::ParsedOrigin,
    query_output::Environment,
};
use crate::{
    parser::SqlParser,
    schema::{Schema, Table},
};

pub(super) fn tables(
    source: &str,
    schema: &Schema,
    uri: &str,
    cursor: usize,
    dialect: Option<&str>,
    environment: Environment<'_>,
) -> Vec<Table> {
    if dialect != Some("hive")
        || source.len() > super::LOCAL_RELATION_SCAN_MAX_BYTES
        || !source
            .as_bytes()
            .windows(7)
            .any(|s| s[0].eq_ignore_ascii_case(&b'l') && s.eq_ignore_ascii_case(b"LATERAL"))
    {
        return Vec::new();
    }
    let parsed = ParsedOrigin::for_completion(source, dialect, cursor);
    let Some(tree) = parsed.tree() else {
        return Vec::new();
    };
    let parser = SqlParser::new();
    let Some(from) = parser.query_from_range_at_position(
        tree,
        source,
        crate::position::byte_position_at_end(&source[..cursor]),
    ) else {
        return Vec::new();
    };
    let views: Vec<crate::parser::LateralView> = SqlParser::lateral_views(tree, source);
    if !views.iter().any(|view| view.from == from) {
        return Vec::new();
    }
    let upper = SqlParser::mask_sql_noise(source).to_ascii_uppercase();
    let content = SqlParser::mask_sql_operand_context_for_dialect(source, "hive");
    let mut work = schema.clone();
    let mut result = Vec::new();
    for view in views.into_iter().filter(|view| view.from == from) {
        if view.arguments.len() > 256 {
            continue;
        }
        let at = view.arguments.first().map_or(view.range.start, |r| r.start);
        let inputs = ProjectionSources::new(
            source,
            &upper,
            &work,
            at,
            dialect,
            &[],
            Environment {
                origin: parsed.view(),
                ..environment
            },
        );
        let types: Vec<_> = view
            .arguments
            .iter()
            .map(|range| {
                inputs
                    .argument_type(source, &content, range.clone(), dialect)
                    .map(|ty| ty.data_type)
            })
            .collect();
        let raw = shape(&view.function, &types, &view.arguments, source);
        let mut names: Vec<String> = if view.columns.is_empty() {
            raw.as_ref()
                .map(|r| r.iter().map(|(n, _)| n.clone()).collect())
                .unwrap_or_default()
        } else {
            view.columns
        };
        let mut seen = std::collections::HashSet::new();
        if names
            .iter()
            .any(|name| !seen.insert(name.to_ascii_lowercase()))
        {
            names.clear();
        }
        let valid = raw.as_ref().filter(|raw| raw.len() == names.len());
        let line = source[..view.range.start]
            .bytes()
            .filter(|c| *c == b'\n')
            .count() as u32
            + 1;
        let columns = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let ty = valid.map_or("unknown", |raw| raw[i].1.as_str());
                let mut column = local_relation_column(name, ty, uri, line);
                column.nullable = !(view.function.eq_ignore_ascii_case("posexplode")
                    && valid.is_some()
                    && i == 0
                    && !view.outer);
                column
            })
            .collect();
        let table = local_relation_table(view.alias, "TABLE FUNCTION", columns, uri, line);
        install_local_table(&mut work, table.clone());
        result.push(table);
    }
    result
}

fn shape(
    function: &str,
    types: &[Option<String>],
    arguments: &[std::ops::Range<usize>],
    source: &str,
) -> Option<Vec<(String, String)>> {
    if SqlParser::identifier_qualifier(function).is_some() || function.starts_with(['`', '"']) {
        return None;
    }
    let first = || kind(types.first()?.as_deref()?);
    let output = match function.to_ascii_lowercase().as_str() {
        "explode" if types.len() == 1 => match first()? {
            Kind::Array(ty) => vec![("col".into(), ty)],
            Kind::Map(key, value) => vec![("key".into(), key), ("value".into(), value)],
            _ => return None,
        },
        "posexplode" if types.len() == 1 && types[0].is_none() => vec![
            ("pos".into(), "int".into()),
            ("val".into(), "unknown".into()),
        ],
        "posexplode" if types.len() == 1 => match first()? {
            Kind::Array(ty) => vec![("pos".into(), "int".into()), ("val".into(), ty)],
            _ => return None,
        },
        "inline" if types.len() == 1 => match first()? {
            Kind::Array(ty) => match kind(&ty)? {
                Kind::Struct(fields) => fields,
                _ => return None,
            },
            _ => return None,
        },
        "json_tuple" | "parse_url_tuple" if types.len() > 1 => (0..types.len() - 1)
            .map(|i| (format!("c{i}"), "string".into()))
            .collect(),
        "stack" if types.len() > 1 => {
            let rows = source
                .get(arguments[0].clone())?
                .trim()
                .parse::<usize>()
                .ok()?;
            if rows == 0 || rows > 256 {
                return None;
            }
            let width = (types.len() - 1).div_ceil(rows);
            let mut columns = Vec::new();
            for i in 0..width {
                let mut ty = types[1 + i].clone().unwrap_or_else(|| "unknown".into());
                for row in 1..rows {
                    if let Some(other) = types.get(1 + row * width + i) {
                        if other.as_ref() != Some(&ty) {
                            ty = "unknown".into();
                        }
                    }
                }
                columns.push((format!("col{i}"), ty));
            }
            columns
        }
        _ => return None,
    };
    Some(output)
}
