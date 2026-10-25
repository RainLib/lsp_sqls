//! Prepare a complete configuration update before touching shared analysis state.
//! Missing fields preserve settings; explicitly malformed fields reject the patch.
use super::preferences::{CompletionPreferences, FormattingPreferences};
use crate::{
    dialects::DialectRegistry,
    schema::{Schema, SchemaId},
};
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, HashSet};

fn provided<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
fn token<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    super::analysis_provenance::parse_analysis_token(Some(&value))
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("Invalid analysis token"))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigurationPatch {
    #[serde(default, deserialize_with = "token")]
    pub oxide_analysis_token: Option<String>,
    #[serde(default, deserialize_with = "provided")]
    pub default_dialect: Option<String>,
    #[serde(default, deserialize_with = "provided")]
    pub completion_preferences: Option<CompletionPreferences>,
    #[serde(default, deserialize_with = "provided")]
    pub formatting_preferences: Option<FormattingPreferences>,
    #[serde(default, deserialize_with = "provided")]
    pub schemas: Option<Vec<Schema>>,
    #[serde(default, deserialize_with = "provided")]
    pub file_schemas: Option<BTreeMap<String, SchemaId>>,
    #[serde(default, deserialize_with = "provided")]
    pub file_dialects: Option<BTreeMap<String, String>>,
}
impl ConfigurationPatch {
    pub fn parse(
        settings: serde_json::Value,
        registry: &DialectRegistry,
    ) -> Result<Self, &'static str> {
        if !settings.is_object() {
            return Err("Configuration must be an object");
        }
        // Consume the incoming Value, including large catalogs, rather than
        // cloning and deserializing the schema array again during commit.
        let mut patch: Self =
            serde_json::from_value(settings).map_err(|_| "Invalid LSP configuration fields")?;
        let known = |name: &str| {
            !name.is_empty() && name.len() <= 64 && registry.get_by_name(name).is_some()
        };
        if let Some(name) = patch.default_dialect.as_mut() {
            if !known(name) {
                return Err("Unknown configuration dialect");
            }
            name.make_ascii_lowercase();
        }
        if let Some(routes) = patch.file_dialects.as_mut() {
            for name in routes.values_mut() {
                if !known(name) {
                    return Err("Unknown file dialect");
                }
                name.make_ascii_lowercase();
            }
        }
        if let Some(schemas) = &patch.schemas {
            let mut ids = HashSet::with_capacity(schemas.len());
            if schemas.iter().any(|schema| !ids.insert(schema.id)) {
                return Err("Duplicate schema identity in configuration");
            }
        }
        Ok(patch)
    }
}
