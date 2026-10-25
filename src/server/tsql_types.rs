//! Validated SQL Server scalar spellings; no UDT or conversion guessing.
use crate::parser::SqlParser;
pub(super) fn scalar_type(raw: &str) -> Option<String> {
    if raw.len() > 256 {
        return None;
    }
    let noise = SqlParser::mask_sql_noise_for_dialect(raw, "sqlserver");
    let normalized = noise
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let (base, modifier) =
        normalized
            .split_once('(')
            .map_or((normalized.as_str().trim(), None), |(base, tail)| {
                (
                    base.trim(),
                    Some(tail.strip_suffix(')').unwrap_or("?").trim()),
                )
            });
    let known = matches!(
        base,
        "tinyint"
            | "smallint"
            | "int"
            | "integer"
            | "bigint"
            | "bit"
            | "decimal"
            | "numeric"
            | "money"
            | "smallmoney"
            | "real"
            | "float"
            | "char"
            | "nchar"
            | "varchar"
            | "nvarchar"
            | "binary"
            | "varbinary"
            | "date"
            | "time"
            | "datetime"
            | "datetime2"
            | "datetimeoffset"
            | "smalldatetime"
            | "uniqueidentifier"
            | "xml"
            | "sql_variant"
            | "text"
            | "ntext"
            | "image"
    );
    if !known {
        return None;
    }
    if let Some(modifier) = modifier {
        if modifier == "max" {
            if !matches!(base, "varchar" | "nvarchar" | "varbinary") {
                return None;
            }
        } else {
            let sizes = modifier
                .split(',')
                .map(|part| part.trim().parse::<u32>())
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            let valid = match (base, sizes.as_slice()) {
                ("decimal" | "numeric", [p]) => (1..=38).contains(p),
                ("decimal" | "numeric", [p, s]) => (1..=38).contains(p) && s <= p,
                ("time" | "datetime2" | "datetimeoffset", [p]) => *p <= 7,
                ("float", [p]) => (1..=53).contains(p),
                ("char" | "varchar" | "binary" | "varbinary", [n]) => (1..=8000).contains(n),
                ("nchar" | "nvarchar", [n]) => (1..=4000).contains(n),
                _ => false,
            };
            if !valid {
                return None;
            }
        }
    }
    Some(if let Some(modifier) = modifier {
        let modifier = if modifier == "max" {
            "max".into()
        } else {
            modifier
                .split(',')
                .map(|p| p.trim().parse::<u32>().unwrap().to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        format!("{base}({modifier})")
    } else {
        base.into()
    })
}
