//! A callable parameter view is shared by signatures, ranking and inlay hints.
use crate::{
    placeholder::SqlPlaceholderDialect,
    schema::{Function, FunctionParameter},
};

pub(super) struct RoutineParameters<'a> {
    inputs: Vec<&'a FunctionParameter>,
    variadic: Option<usize>,
}

pub(super) fn explicit_variadic(text: &str) -> bool {
    let text = text.trim_start();
    text.get(..8)
        .is_some_and(|word| word.eq_ignore_ascii_case("VARIADIC"))
        && text
            .get(8..)
            .and_then(|tail| tail.chars().next())
            .is_some_and(|ch| !crate::parser::SqlParser::is_identifier_char(ch))
}

impl<'a> RoutineParameters<'a> {
    pub(super) fn new(function: &'a Function, dialect: &str, procedure: bool) -> Self {
        let postgres = crate::sql_lexical::lexical_rules_for(dialect).placeholder
            == SqlPlaceholderDialect::Postgres;
        let function_output = postgres && !procedure && function.routine_kind() != "Procedure";
        let inputs = function
            .parameters
            .iter()
            .filter(|parameter| {
                parameter.mode_name() != Some("RETURN")
                    && !(function_output && matches!(parameter.mode_name(), Some("OUT" | "TABLE")))
            })
            .collect::<Vec<_>>();
        let variadic = inputs.len().checked_sub(1).filter(|last| {
            postgres
                && inputs[*last].mode_name() == Some("VARIADIC")
                && !Self::element_type(inputs[*last]).is_empty()
                && inputs[*last].data_type.trim().ends_with("[]")
        });
        Self { inputs, variadic }
    }
    pub(super) fn len(&self) -> usize {
        self.inputs.len()
    }
    pub(super) fn named_index(&self, name: &str) -> Option<usize> {
        let mut found = self
            .inputs
            .iter()
            .enumerate()
            .filter(|(_, parameter)| parameter.name == name);
        let (index, _) = found.next()?;
        found.next().is_none().then_some(index)
    }
    pub(super) fn declaration_index(&self, index: usize) -> Option<usize> {
        if index < self.inputs.len() {
            Some(index)
        } else {
            self.variadic
        }
    }
    pub(super) fn variadic_index(&self) -> Option<usize> {
        self.variadic
    }
    pub(super) fn iter(&self) -> impl Iterator<Item = &&'a FunctionParameter> {
        self.inputs.iter()
    }
    pub(super) fn parameter(&self, index: usize) -> Option<&'a FunctionParameter> {
        self.inputs.get(index).copied().or_else(|| {
            self.variadic
                .and_then(|last| self.inputs.get(last).copied())
        })
    }
    fn element_type(parameter: &FunctionParameter) -> &str {
        parameter.data_type.trim().trim_end_matches("[]").trim_end()
    }
    pub(super) fn data_type(&self, index: usize, explicit: bool) -> Option<&'a str> {
        let parameter = self.parameter(index)?;
        Some(
            if !explicit && self.variadic.is_some_and(|last| index >= last) {
                Self::element_type(parameter)
            } else {
                parameter.data_type.trim()
            },
        )
    }
    pub(super) fn accepts(&self, count: usize, explicit: bool) -> bool {
        if explicit {
            return self.variadic.is_some() && count == self.inputs.len();
        }
        let required = self
            .inputs
            .iter()
            .filter(|parameter| !parameter.optional)
            .count();
        required <= count && (self.variadic.is_some() || count <= self.inputs.len())
    }
    pub(super) fn has_slot(&self, index: usize, content: bool, explicit: bool) -> bool {
        if explicit {
            return self.variadic == Some(index);
        }
        if self.inputs.is_empty() {
            index == 0 && !content
        } else {
            self.parameter(index).is_some()
        }
    }
}

pub(super) fn routine_kind_matches(function: &Function, procedure: bool) -> bool {
    match function.routine_type.as_deref() {
        Some(kind) if kind.eq_ignore_ascii_case("procedure") => procedure,
        Some(kind) if kind.eq_ignore_ascii_case("function") => !procedure,
        _ => true,
    }
}
