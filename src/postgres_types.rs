//! Bounded builtin type identities. Quoted names preserve case and only the
//! actual pg_catalog namespace may qualify a builtin; domains stay unknown.
#[derive(Debug, PartialEq)]
pub(crate) struct BuiltinType<'a> {
    pub name: &'static str,
    pub modifiers: Option<&'a str>,
    pub array: bool,
}

pub(crate) fn builtin_type(input: &str) -> Option<BuiltinType<'_>> {
    if input.len() > 1024 {
        return None;
    }
    let (first, quoted, mut tail) = identifier(input.trim())?;
    let (mut name, quoted, qualified) = if let Some(after) = tail.strip_prefix('.') {
        if first != "pg_catalog" {
            return None;
        }
        let (name, quoted, rest) = identifier(after.trim_start())?;
        tail = rest;
        (name, quoted, true)
    } else {
        (first, quoted, false)
    };
    let sql_alias = !quoted && !qualified;
    if sql_alias {
        if name == "double" {
            tail = keyword(tail, "precision")?;
            name = "float8".into();
        }
        if matches!(name.as_str(), "character" | "char" | "bit") {
            if let Some(rest) = keyword(tail, "varying") {
                tail = rest;
                name = if name == "bit" { "varbit" } else { "varchar" }.into();
            }
        }
    }
    let internal_array = name.starts_with('_');
    let identity = if internal_array {
        name.strip_prefix('_')?
    } else {
        &name
    };
    let mut canonical = internal_name(identity).or_else(|| {
        (sql_alias && !internal_array)
            .then_some(match identity {
                "integer" | "int" => Some("integer"),
                "smallint" => Some("smallint"),
                "bigint" => Some("bigint"),
                "boolean" => Some("boolean"),
                "real" => Some("real"),
                "decimal" => Some("numeric"),
                "character" | "char" => Some("character"),
                "float" => Some("double precision"),
                _ => None,
            })
            .flatten()
    })?;
    // Unquoted SQL CHAR is an alias for bpchar, unlike pg_catalog."char".
    if sql_alias && name == "char" {
        canonical = "character";
    }
    let mut modifiers = None;
    if tail.starts_with('(') {
        let end = tail.find(')')?;
        let content = &tail[1..end];
        let fields = content.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.is_empty()
            || fields.len() > 2
            || fields.iter().any(|value| value.parse::<i32>().is_err())
        {
            return None;
        }
        let width = fields[0].parse::<i32>().ok()?;
        let max_fields = if canonical == "numeric" { 2 } else { 1 };
        if fields.len() > max_fields
            || !(matches!(
                canonical,
                "numeric"
                    | "character"
                    | "varchar"
                    | "bit"
                    | "varbit"
                    | "time"
                    | "timetz"
                    | "timestamp"
                    | "timestamptz"
            ) || sql_alias && name == "float")
        {
            return None;
        }
        if matches!(canonical, "time" | "timetz" | "timestamp" | "timestamptz") {
            if !(0..=6).contains(&width) {
                return None;
            }
        } else if width <= 0 {
            return None;
        }
        if sql_alias && name == "float" {
            if width > 53 {
                return None;
            }
            canonical = if width <= 24 {
                "real"
            } else {
                "double precision"
            };
        }
        modifiers = Some(&tail[..=end]);
        tail = tail[end + 1..].trim_start();
    }
    if sql_alias && matches!(name.as_str(), "time" | "timestamp") {
        for (qualifier, zone) in [("with", true), ("without", false)] {
            if let Some(rest) = keyword(tail, qualifier) {
                tail = keyword(keyword(rest, "time")?, "zone")?;
                canonical = match (name.as_str(), zone) {
                    ("time", true) => "timetz",
                    ("timestamp", true) => "timestamptz",
                    ("time", false) => "time",
                    _ => "timestamp",
                };
                break;
            }
        }
    }
    if tail.is_empty() {
        return Some(BuiltinType {
            name: canonical,
            modifiers,
            array: internal_array,
        });
    }
    let keyword_array = keyword(tail, "array");
    let mut array = internal_array || keyword_array.is_some();
    if let Some(rest) = keyword_array {
        tail = rest;
    }
    while let Some(rest) = tail.strip_prefix('[') {
        let end = rest.find(']')?;
        if !rest[..end].trim().bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        array = true;
        tail = rest[end + 1..].trim_start();
    }
    tail.is_empty().then_some(BuiltinType {
        name: canonical,
        modifiers,
        array,
    })
}

fn identifier(input: &str) -> Option<(String, bool, &str)> {
    if let Some(rest) = input.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some((rest[..end].to_string(), true, rest[end + 1..].trim_start()));
    }
    let length = input
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
        .count();
    let first = *input.as_bytes().first()?;
    if length == 0 || !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    Some((
        input[..length].to_ascii_lowercase(),
        false,
        input[length..].trim_start(),
    ))
}

fn keyword<'a>(input: &'a str, word: &str) -> Option<&'a str> {
    let head = input.get(..word.len())?;
    if !head.eq_ignore_ascii_case(word)
        || input
            .as_bytes()
            .get(word.len())
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
    {
        return None;
    }
    Some(input[word.len()..].trim_start())
}

fn internal_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "bool" => "boolean",
        "int2" => "smallint",
        "int4" => "integer",
        "int8" => "bigint",
        "float4" => "real",
        "float8" => "double precision",
        "bpchar" => "character",
        "char" => "\"char\"",
        "numeric" => "numeric",
        "money" => "money",
        "text" => "text",
        "varchar" => "varchar",
        "name" => "name",
        "bytea" => "bytea",
        "bit" => "bit",
        "varbit" => "varbit",
        "date" => "date",
        "time" => "time",
        "timetz" => "timetz",
        "timestamp" => "timestamp",
        "timestamptz" => "timestamptz",
        "interval" => "interval",
        "uuid" => "uuid",
        "json" => "json",
        "jsonb" => "jsonb",
        "xml" => "xml",
        "inet" => "inet",
        "cidr" => "cidr",
        "macaddr" => "macaddr",
        "macaddr8" => "macaddr8",
        "tsvector" => "tsvector",
        "tsquery" => "tsquery",
        "oid" => "oid",
        "xid" => "xid",
        "xid8" => "xid8",
        "cid" => "cid",
        "pg_lsn" => "pg_lsn",
        "regclass" => "regclass",
        "regtype" => "regtype",
        "regproc" => "regproc",
        "regprocedure" => "regprocedure",
        "regoper" => "regoper",
        "regoperator" => "regoperator",
        "regnamespace" => "regnamespace",
        "regrole" => "regrole",
        "regcollation" => "regcollation",
        "regconfig" => "regconfig",
        "regdictionary" => "regdictionary",
        "point" => "point",
        "line" => "line",
        "lseg" => "lseg",
        "box" => "box",
        "path" => "path",
        "polygon" => "polygon",
        "circle" => "circle",
        "int4range" => "int4range",
        "int8range" => "int8range",
        "numrange" => "numrange",
        "tsrange" => "tsrange",
        "tstzrange" => "tstzrange",
        "daterange" => "daterange",
        "int4multirange" => "int4multirange",
        "int8multirange" => "int8multirange",
        "nummultirange" => "nummultirange",
        "tsmultirange" => "tsmultirange",
        "tstzmultirange" => "tstzmultirange",
        "datemultirange" => "datemultirange",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_builtin_identity_preserves_catalog_and_quoted_case() {
        for (input, name, array) in [
            ("PG_CATALOG . int4", "integer", false),
            ("\"pg_catalog\".\"bool\"", "boolean", false),
            ("\"int4\"", "integer", false),
            ("char", "character", false),
            ("pg_catalog.char", "\"char\"", false),
            ("character varying(8)", "varchar", false),
            ("timestamp(3) with time zone", "timestamptz", false),
            ("float(24)", "real", false),
            ("pg_catalog._bool", "boolean", true),
            ("pg_catalog.bool [2][]", "boolean", true),
            ("integer ARRAY[3]", "integer", true),
        ] {
            let result = builtin_type(input).unwrap();
            assert_eq!((result.name, result.array), (name, array), "{input}");
        }
        for input in [
            "public.int4",
            "\"PG_CATALOG\".int4",
            "pg_catalog.\"INT4\"",
            "\"pg_catalog.int4\"",
            "pg_catalog.integer",
            "\"boolean\"",
            "pg_catalog.int4(2)",
            "bool trailing",
            "numeric(1,2,3)",
            "timestamp(7)",
            "float(54)",
            "pg_catalog._integer",
            "int4[bad]",
            "int4[2] trailing",
            "\"in\"\"t4\"",
            "pg_catalog.int4.other",
        ] {
            assert_eq!(builtin_type(input), None, "{input}");
        }
        assert_eq!(builtin_type(&"int4".repeat(257)), None);
    }
}
