//! Constructors preserve known element/field identities without implementing
//! Hive's full implicit conversion lattice or evaluating arbitrary UDFs.
use super::{infer, ExpressionType, ProjectionSources};
use tree_sitter::Node;

fn value(data_type: String) -> Option<ExpressionType> {
    (data_type.len() <= 16 * 1024).then_some(ExpressionType {
        data_type,
        nullable: false,
    })
}
fn common(types: Vec<ExpressionType>) -> String {
    let mut found: Option<String> = None;
    for ty in types {
        if ty.data_type == "null" {
            continue;
        }
        if found
            .as_ref()
            .is_some_and(|known| !known.eq_ignore_ascii_case(&ty.data_type))
        {
            return "unknown".into();
        }
        found = Some(ty.data_type);
    }
    found.unwrap_or_else(|| "void".into())
}
pub(super) fn array(
    node: Node<'_>,
    sql: &str,
    sources: &ProjectionSources<'_>,
    depth: usize,
) -> Option<ExpressionType> {
    let args: Vec<_> = node
        .named_children(&mut node.walk())
        .filter(|n| n.kind() != "keyword_array")
        .collect();
    if args.len() > 256 {
        return None;
    }
    let types = args
        .into_iter()
        .map(|arg| {
            infer(arg, sql, sources, Some("hive"), depth + 1)
                .unwrap_or_else(|| ExpressionType::new("unknown", true))
        })
        .collect();
    value(format!("array<{}>", common(types)))
}
pub(super) fn constructor(
    name: &str,
    args: &[Node<'_>],
    sql: &str,
    sources: &ProjectionSources<'_>,
    depth: usize,
) -> Option<ExpressionType> {
    if args.len() > 256 {
        return None;
    }
    let types = || {
        args.iter()
            .map(|arg| {
                infer(*arg, sql, sources, Some("hive"), depth + 1)
                    .unwrap_or_else(|| ExpressionType::new("unknown", true))
            })
            .collect::<Vec<_>>()
    };
    match name {
        "array" => value(format!("array<{}>", common(types()))),
        "map" if args.len().is_multiple_of(2) => {
            if args.is_empty() {
                return value("map<void,void>".into());
            }
            let types = types();
            let keys = types.iter().step_by(2).cloned().collect();
            let values = types.iter().skip(1).step_by(2).cloned().collect();
            value(format!("map<{},{}>", common(keys), common(values)))
        }
        "struct" => {
            let mut fields = Vec::new();
            for (i, ty) in types().into_iter().enumerate() {
                fields.push(format!("col{}:{}", i + 1, ty.data_type));
            }
            value(format!("struct<{}>", fields.join(",")))
        }
        "named_struct" if args.len().is_multiple_of(2) && !args.is_empty() => {
            let mut fields = Vec::new();
            for pair in args.chunks(2) {
                let literal = pair[0].utf8_text(sql.as_bytes()).ok()?.trim();
                let name = literal.strip_prefix('\'')?.strip_suffix('\'')?;
                if name.is_empty() || name.contains(['\\', '\'']) {
                    return None;
                }
                let ty = infer(pair[1], sql, sources, Some("hive"), depth + 1)
                    .unwrap_or_else(|| ExpressionType::new("unknown", true));
                fields.push(format!("`{}`:{}", name.replace('`', "``"), ty.data_type));
            }
            value(format!("struct<{}>", fields.join(",")))
        }
        _ => None,
    }
}
