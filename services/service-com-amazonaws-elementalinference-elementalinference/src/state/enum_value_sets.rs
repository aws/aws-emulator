// SPDX-License-Identifier: CC0-1.0

//! The ORDER this service states an enum's value set in when it refuses a value outside
//! it.
//!
//! A service states a value set in its own published order, which is not necessarily the
//! order the model declares the members in. The table here holds one row per enum whose
//! stated order is known to differ; a value set absent from it is stated as the model
//! declares it, because this service has shown no other order for it. One place, so every
//! member targeting one enum — of any operation, at any depth — states that enum's set
//! identically.

use super::*;

/// The order this service STATES the `DictionaryLanguage` value set in when it refuses
/// a value outside it.
///
/// A service states an enum's value set in its own published order, which is not the
/// order the model happens to declare that enum's members in: this is the order this
/// service's refusal of a language outside `DictionaryLanguage` lists them in, taken
/// from the refusal the service answers such a request with. It is FIXED text — no
/// member of the request and nothing in the store varies it — so it lives here beside
/// the service's other refusal wording, as the one copy both the boundary renderer
/// (`crate::service`) and a handler's own refusal
/// ([`invalid_language_message`]) state the set from.
///
/// It holds exactly the values `crate::ops::types::DICTIONARY_LANGUAGE_VALUES` holds,
/// which is what [`published_enum_value_set`] recognises an enum's value set by and
/// what the dictionary suite asserts. The last value is the one the model has since
/// ADDED to the enum: the refusal that showed this order was answered for the six values
/// before it, so their order is the recorded one and the added value takes the position
/// the recording says nothing about — the end.
pub const DICTIONARY_LANGUAGE_VALUE_SET: [&str; 7] =
    ["spa", "por", "deu", "fra", "ita", "eng", "ara"];

/// Every enum value set this service publishes an order for.
///
/// One row per enum whose stated order is known to differ from its declaration order;
/// a value set absent from this table is stated as the model declares it, because this
/// service has shown no other order for it.
pub(super) const PUBLISHED_ENUM_VALUE_SETS: &[&[&str]] = &[&DICTIONARY_LANGUAGE_VALUE_SET];

/// The order this service states `values` in, when `values` is the value set of an
/// enum whose order it publishes ([`PUBLISHED_ENUM_VALUE_SETS`]); `None` for any other
/// value set.
///
/// An enum is recognised by its value SET rather than by the member that carried the
/// rejected value, so every member targeting one enum — of any operation, at any depth
/// — states that enum's set in the one order, and a set this service publishes no
/// order for keeps the order it was presented in.
pub fn published_enum_value_set(values: &[String]) -> Option<&'static [&'static str]> {
    let presented: BTreeSet<&str> = values.iter().map(String::as_str).collect();
    PUBLISHED_ENUM_VALUE_SETS
        .iter()
        .copied()
        .find(|published| {
            published.len() == presented.len()
                && published.iter().all(|value| presented.contains(value))
        })
}

/// The refusal a value outside an enum's value set earns: the constraint sentence the
/// service words such a refusal with, stating the member at fault, the value it
/// carried and the value set in the order this service publishes for it
/// ([`published_enum_value_set`]).
///
/// Built through the framework's own violation rendering, so a refusal a HANDLER words
/// is byte-identical to the one the boundary walk words for the same member and value.
pub fn enum_value_refusal_message(member: &str, value: &str, values: &[&str]) -> String {
    let stated: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    let stated = published_enum_value_set(&stated).map_or(stated, |published| {
        published.iter().map(|value| (*value).to_string()).collect()
    });
    let violation = ::aws_smithy_schema_server::constraints::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(member),
        kind: ::aws_smithy_schema_server::constraints::ViolationKind::EnumValue(stated),
        value: Some(value.to_string()),
        sensitive: false,
    };
    ::aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(&violation))
        .unwrap_or_default()
}

/// The refusal a `language` outside the modelled `DictionaryLanguage` enum earns, or
/// `None` when the value is one the enum lists.
///
/// `language` is an enum member, so only the values the model lists are a dictionary
/// language. The boundary validator refuses the rest, and this is the same refusal for
/// a request that reaches a handler regardless — ONE copy, so `CreateDictionary` and
/// `UpdateDictionary` cannot drift in which values they accept or in what they say
/// about a value they do not, and the sentence is the one the boundary states
/// ([`enum_value_refusal_message`]) rather than a second wording of the same rule.
pub fn invalid_language_message(language: &str) -> Option<String> {
    if crate::ops::types::DICTIONARY_LANGUAGE_VALUES.contains(&language) {
        return None;
    }
    Some(enum_value_refusal_message(
        LANGUAGE_MEMBER,
        language,
        crate::ops::types::DICTIONARY_LANGUAGE_VALUES,
    ))
}
