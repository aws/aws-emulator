// SPDX-License-Identifier: CC0-1.0

//! The format a blueprint's configuration document is rendered in.
//!
//! A blueprint record holds its Data Prepper configuration ONCE, as the YAML document
//! `PipelineBlueprint$PipelineConfigurationBody` describes ("The YAML configuration of the
//! blueprint"). `GetPipelineBlueprint` lets a caller ask for that one document in either of
//! the two formats `com.amazonaws.osis#BlueprintFormat` admits (its
//! `@pattern("^(YAML|JSON)$")`), so the JSON form is COMPUTED from the stored YAML rather
//! than kept as a second copy that could drift from it.
//!
//! The document read here is the service's OWN published data — the catalog constant in
//! [`crate::state`] — never a caller's: no operation writes a blueprint, so nothing in a
//! request decides how large or how deeply nested the text handed to the parser is.

/// `BlueprintFormat`'s YAML value, and the value the response echoes when a request names
/// no format: the stored document is YAML, so it is what the service renders by default.
pub const BLUEPRINT_FORMAT_YAML: &str = "YAML";

/// `BlueprintFormat`'s JSON value.
pub const BLUEPRINT_FORMAT_JSON: &str = "JSON";

/// Every value `BlueprintFormat` admits, in the order its pattern alternates them
/// (smithy_query: shape BlueprintFormat, `@pattern("^(YAML|JSON)$")`).
pub const BLUEPRINT_FORMATS: &[&str] = &[BLUEPRINT_FORMAT_YAML, BLUEPRINT_FORMAT_JSON];

/// The pattern a refusal of a `Format` value PUBLISHES for `BlueprintFormat`: the model's
/// `@pattern("^(YAML|JSON)$")` in the unanchored find-form this service quotes its patterns
/// in (`tests/all/pattern_messages.rs` states that rule for every patterned shape the
/// service declares). Stating it once is what keeps the handler's refusal worded exactly as
/// the boundary validator's is, so one rejected value has one message whichever layer
/// answers it.
pub const BLUEPRINT_FORMAT_PATTERN: &str = "(YAML|JSON).*";

/// One of the two formats a blueprint's configuration document can be rendered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlueprintFormat {
    /// The document as the record holds it.
    Yaml,
    /// The same document, rendered as JSON.
    Json,
}

impl BlueprintFormat {
    /// The format a request that names none asks for.
    pub const DEFAULT: Self = Self::Yaml;

    /// The format `requested` names, or `None` when it names neither.
    ///
    /// An absent format is [`Self::DEFAULT`]. The member's `@pattern` already refuses an
    /// unadmitted value at the boundary, so `None` is the answer for a value that reached a
    /// handler anyway — a refusal, never a silent fallback to the default, which would
    /// answer a document in a format nobody asked for.
    pub fn parse(requested: Option<&str>) -> Option<Self> {
        match requested {
            None => Some(Self::DEFAULT),
            Some(BLUEPRINT_FORMAT_YAML) => Some(Self::Yaml),
            Some(BLUEPRINT_FORMAT_JSON) => Some(Self::Json),
            Some(_) => None,
        }
    }

    /// The `BlueprintFormat` value naming this format, as the response's `Format` member
    /// reports it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yaml => BLUEPRINT_FORMAT_YAML,
            Self::Json => BLUEPRINT_FORMAT_JSON,
        }
    }

    /// Render `stored_yaml` — a blueprint's configuration document as its record holds it —
    /// in this format.
    ///
    /// YAML is the stored form, so it is handed back unchanged, down to its layout. JSON is
    /// the same document read once and written out as JSON, so whichever format a caller
    /// asks for it is given one configuration.
    pub fn render(self, stored_yaml: &str) -> Result<String, UnrenderableBlueprint> {
        match self {
            Self::Yaml => Ok(stored_yaml.to_string()),
            Self::Json => {
                let document = sole_document(stored_yaml)?;
                ::serde_json::to_string_pretty(&as_json(&document)?)
                    .map_err(UnrenderableBlueprint::new)
            }
        }
    }
}

/// The one YAML document `text` holds.
///
/// A configuration document describes a single Data Prepper pipeline set, so a text holding
/// none, or holding several, is not one.
fn sole_document(text: &str) -> Result<::yaml_rust2::Yaml, UnrenderableBlueprint> {
    let mut documents =
        ::yaml_rust2::YamlLoader::load_from_str(text).map_err(UnrenderableBlueprint::new)?;
    match documents.len() {
        1 => documents
            .pop()
            .ok_or_else(|| UnrenderableBlueprint::new("the document is empty")),
        held => Err(UnrenderableBlueprint::new(format!(
            "expected one YAML document, found {held}"
        ))),
    }
}

/// One YAML node as the JSON value that states the same thing.
///
/// YAML 1.2's data model is JSON's with a wider surface syntax, so the mapping is direct: a
/// mapping becomes an object keyed by the text of its keys, a sequence an array, the scalars
/// their JSON counterparts, and an explicitly null node JSON's null.
///
/// A node that states no value JSON can hold is REFUSED rather than rendered as null: the
/// two formats have to state the SAME configuration, so a document the mapping cannot carry
/// over whole is one the service cannot render in JSON at all. That covers a value the
/// loader could not resolve, a reference it could not resolve to the node it stands for, and
/// a mapping whose keys cannot all be told apart as JSON member names. (A resolvable
/// reference is not one of those: the loader resolves it to the node it names, which this
/// mapping then renders like any other.)
///
/// The recursion follows the document's own nesting, which is fixed: the text is the
/// catalog constant this crate ships, so no request decides how deep it goes.
fn as_json(node: &::yaml_rust2::Yaml) -> Result<::serde_json::Value, UnrenderableBlueprint> {
    use ::yaml_rust2::Yaml;
    Ok(match node {
        Yaml::String(text) => ::serde_json::Value::String(text.clone()),
        Yaml::Integer(number) => ::serde_json::Value::from(*number),
        // A YAML float is held as the text it was written as; JSON has no value for one
        // that is not finite, so such a scalar keeps its own text.
        Yaml::Real(text) => text
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map_or_else(
                || ::serde_json::Value::String(text.clone()),
                ::serde_json::Value::Number,
            ),
        Yaml::Boolean(held) => ::serde_json::Value::Bool(*held),
        Yaml::Array(items) => ::serde_json::Value::Array(
            items
                .iter()
                .map(as_json)
                .collect::<Result<Vec<_>, UnrenderableBlueprint>>()?,
        ),
        Yaml::Hash(entries) => {
            let mut members = ::serde_json::Map::new();
            for (key, value) in entries {
                let name = member_name(key)?;
                if members.insert(name.clone(), as_json(value)?).is_some() {
                    return Err(UnrenderableBlueprint::new(format!(
                        "two mapping keys state the member name {name:?}"
                    )));
                }
            }
            ::serde_json::Value::Object(members)
        }
        Yaml::Null => ::serde_json::Value::Null,
        Yaml::Alias(_) => {
            return Err(UnrenderableBlueprint::new(
                "the document holds a reference that could not be resolved",
            ))
        }
        Yaml::BadValue => {
            return Err(UnrenderableBlueprint::new(
                "the document holds a value that could not be resolved",
            ))
        }
    })
}

/// The member name a YAML mapping key states, as a JSON object names one.
///
/// A scalar key names the member its own text does. JSON has no key that is itself a
/// collection, so such a key is named by the JSON it is written as — which keeps the member
/// in the document rather than dropping it.
fn member_name(key: &::yaml_rust2::Yaml) -> Result<String, UnrenderableBlueprint> {
    use ::yaml_rust2::Yaml;
    match key {
        Yaml::String(text) | Yaml::Real(text) => Ok(text.clone()),
        Yaml::Integer(number) => Ok(number.to_string()),
        Yaml::Boolean(held) => Ok(held.to_string()),
        other => ::serde_json::to_string(&as_json(other)?).map_err(UnrenderableBlueprint::new),
    }
}

/// A stored blueprint whose configuration document could not be rendered in the requested
/// format.
///
/// The catalog is the service's own data, so this is a server-side fault rather than
/// anything a caller can fix: a handler answers it as `InternalException`.
#[derive(Debug, Clone)]
pub struct UnrenderableBlueprint {
    reason: String,
}

impl UnrenderableBlueprint {
    fn new(reason: impl ::std::fmt::Display) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }

    /// Why the document could not be rendered, for a handler's own message.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl ::std::fmt::Display for UnrenderableBlueprint {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl ::std::error::Error for UnrenderableBlueprint {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_format_is_yaml_and_both_admitted_values_parse() {
        assert_eq!(BlueprintFormat::parse(None), Some(BlueprintFormat::Yaml));
        for format in BLUEPRINT_FORMATS {
            let parsed = BlueprintFormat::parse(Some(format));
            assert_eq!(
                parsed.map(BlueprintFormat::as_str),
                Some(*format),
                "case {format:?}: an admitted format must round-trip, got {parsed:?}"
            );
        }
        for rejected in ["yaml", "Json", "XML", ""] {
            assert_eq!(
                BlueprintFormat::parse(Some(rejected)),
                None,
                "case {rejected:?}: only the admitted values name a format"
            );
        }
    }

    /// The rows differ along the YAML construct the document is built from, so the mapping
    /// is asserted over the data model rather than over one configuration.
    #[test]
    fn the_json_rendering_states_what_the_yaml_document_does() {
        let rows: &[(&str, &str, ::serde_json::Value)] = &[
            (
                "a nested mapping and a sequence",
                "version: \"2\"\nlog-pipeline:\n  sink:\n    - opensearch:\n        index: \"logs\"\n",
                ::serde_json::json!({
                    "version": "2",
                    "log-pipeline": {"sink": [{"opensearch": {"index": "logs"}}]},
                }),
            ),
            (
                "an integer, a float and a boolean scalar",
                "workers: 2\nrate: 1.5\nacknowledgments: true\n",
                ::serde_json::json!({"workers": 2, "rate": 1.5, "acknowledgments": true}),
            ),
            (
                "an empty mapping and an empty value",
                "codec:\n  newline: {}\ncompression:\n",
                ::serde_json::json!({"codec": {"newline": {}}, "compression": null}),
            ),
            (
                "a flow collection, which is JSON's own syntax",
                "{\"hosts\": [\"https://example.invalid\"]}",
                ::serde_json::json!({"hosts": ["https://example.invalid"]}),
            ),
            (
                "a non-string key",
                "2: two\ntrue: yes-please\n",
                ::serde_json::json!({"2": "two", "true": "yes-please"}),
            ),
            (
                // An anchored node and the reference to it: the reader resolves the
                // reference, so both members state the node it names.
                "a reference to an anchored node",
                "workers: &units 2\nreaders: *units\n",
                ::serde_json::json!({"workers": 2, "readers": 2}),
            ),
        ];

        for (case, document, expected) in rows {
            let rendered = BlueprintFormat::Json
                .render(document)
                .unwrap_or_else(|error| panic!("case {case:?}: expected a rendering, got {error}"));
            let parsed: ::serde_json::Value = ::serde_json::from_str(&rendered)
                .unwrap_or_else(|error| panic!("case {case:?}: expected JSON, got {error}"));
            assert_eq!(
                &parsed, expected,
                "case {case:?}: expected {expected:?}, got {parsed:?}"
            );

            // The YAML rendering is the stored document itself, unchanged.
            let yaml = BlueprintFormat::Yaml
                .render(document)
                .unwrap_or_else(|error| panic!("case {case:?}: expected a rendering, got {error}"));
            assert_eq!(
                &yaml, document,
                "case {case:?}: YAML is the stored document, got {yaml:?}"
            );
        }
    }

    /// A text that is not one document cannot be rendered as JSON, and says so rather than
    /// answering half a document.
    #[test]
    fn a_text_that_is_not_one_document_is_refused() {
        for (case, text) in [
            ("no document at all", ""),
            ("two documents", "a: 1\n---\nb: 2\n"),
            ("text that does not parse", "a: [1, 2\n"),
            // Two keys JSON cannot tell apart as member names.
            ("keys that collide as member names", "2: two\n\"2\": also-two\n"),
        ] {
            let rendered = BlueprintFormat::Json.render(text);
            assert!(
                rendered.is_err(),
                "case {case:?}: expected a refusal, got {rendered:?}"
            );
        }
    }
}
