//! Prefer proven builtin type identities without guessing implicit casts/UDTs.
use super::routine_parameters::RoutineParameters;
use super::signature_help::{live_overload_accepts_call, RoutineCallContext};
use crate::schema::Function;
use std::cmp::Reverse;

pub(super) fn canonical_type(ty: &str, dialect: &str) -> Option<String> {
    if matches!(dialect, "postgres" | "postgresql" | "pgsql" | "psql") {
        let ty = crate::postgres_types::builtin_type(ty)?;
        return Some(format!("{}{}", ty.name, if ty.array { "[]" } else { "" }));
    }
    if crate::placeholder::SqlPlaceholderDialect::for_name(Some(dialect))
        == crate::placeholder::SqlPlaceholderDialect::SqlServer
    {
        if let Some(ty) = super::tsql_types::scalar_type(ty) {
            return Some({
                if ty == "integer" {
                    "int".into()
                } else {
                    ty
                }
            });
        }
    }
    if ty.len() > 256 || ty.contains(['.', '"', '`', '\'']) {
        return None;
    }
    let name = ty
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let (base, modifier) = if let Some(at) = name.find('(') {
        let modifier = &name[at..];
        if !modifier.ends_with(')')
            || !modifier[1..modifier.len() - 1]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b',' | b' '))
        {
            return None;
        }
        (name[..at].trim(), modifier)
    } else {
        (name.as_str(), "")
    };
    let canonical = match base {
        "int" | "integer" => "int",
        "signed" if matches!(dialect, "mysql" | "mariadb") => "bigint",
        "tinyint" | "smallint" | "mediumint" | "bigint" | "boolean" | "bit" | "real" | "float"
        | "double" | "decimal" | "numeric" | "text" | "varchar" | "nvarchar" | "char" | "nchar"
        | "date" | "datetime" | "timestamp" | "json" | "bigint unsigned" | "int64" | "int32"
        | "int16" | "int8" | "uint64" | "uint32" | "float64" | "float32" => base,
        _ => return None,
    };
    Some(format!("{canonical}{modifier}"))
}

pub(super) fn rank_live_overloads(
    functions: &mut [&Function],
    call: &RoutineCallContext,
    arguments: &[Option<String>],
    complete_arity: Option<usize>,
    explicit: bool,
    dialect: &str,
) {
    let dialect = dialect.to_ascii_lowercase();
    let known = arguments
        .iter()
        .map(|ty| ty.as_deref().and_then(|ty| canonical_type(ty, &dialect)))
        .collect::<Vec<_>>();
    functions.sort_by_cached_key(|function| {
        let parameters = RoutineParameters::new(function, &dialect, call.procedure_call);
        let binding = call
            .supplied
            .has_named()
            .then(|| call.supplied.bind(&parameters, &dialect));
        let exact = known
            .iter()
            .enumerate()
            .filter(|(index, known)| {
                known.as_ref().is_some_and(|known| {
                    parameters
                        .data_type(
                            binding
                                .as_ref()
                                .and_then(|bound| bound.slots.get(*index).copied().flatten())
                                .unwrap_or(*index),
                            explicit,
                        )
                        .and_then(|ty| canonical_type(ty, &dialect))
                        .as_ref()
                        == Some(known)
                })
            })
            .count();
        let fits = binding.as_ref().map(|bound| bound.fits).unwrap_or_else(|| {
            complete_arity.map_or_else(
                || live_overload_accepts_call(function, call, &dialect),
                |count| parameters.accepts(count, explicit),
            )
        });
        (!fits, Reverse(exact))
    });
}
