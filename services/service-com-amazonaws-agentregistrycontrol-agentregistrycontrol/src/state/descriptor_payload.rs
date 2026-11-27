// SPDX-License-Identifier: CC0-1.0

//! Whether a registry record's descriptor payloads are the content this service requires,
//! and how it words a refusal of one that is not.
//!
//! Every descriptor of the typed set carries its content in a `data` member of
//! `DescriptorData`, documented as the descriptor content "serialized as descriptor payload
//! data" and validated by the service against the protocol schema of the descriptor's own
//! type. So a write of a descriptor is refused, before the record is written rather than
//! stored and served back as if it described something, on two counts:
//!
//! * content that is not even JSON cannot be a payload of any of those schemas, so it is
//!   refused by the parse below, which states where it stopped parsing; and
//! * content that IS JSON but that no supported version of the descriptor's own protocol
//!   schema accepts is refused by the schema that applies to it — for the MCP server
//!   descriptor, [`super::mcp_server_schema`].
//!
//! The parse comes first, for every payload of the set: a payload that is no JSON document
//! matches no version of any schema either, and what a caller has to be told about it is
//! where it stopped parsing rather than that no version accepted it.
//!
//! The READINGS the refusal is built from — the unquoted word a parse stopped on where a
//! value belongs, and otherwise the character or the end of input it stopped on together
//! with what was expected there — are the tree's shared ones
//! ([`::service_common::json_parse_fault`]), taken from the parse that actually ran, so this
//! service and every other one handed the same payload cannot disagree about where it went
//! wrong. A fault class worded wrongly is corrected THERE, for every service that quotes it.
//!
//! What is this service's own is the frame: the sentence names the member the payload is
//! carried in and nothing else. It carries no location clause — neither the position the
//! parse ended at nor the opening mark of a container left open — so beyond the token the
//! parser quotes, nothing of a `@sensitive` payload is written into the refusal.

use ::service_common::json_parse_fault as fault;

use crate::ops::types::Descriptors;

/// The member every descriptor of the typed set carries its payload in, as the model names
/// it — the one word the refusal below names, since the refusal is about that member's value
/// and not about which descriptor of the set carries it.
const PAYLOAD_MEMBER: &str = "data";

/// How this service refuses a descriptor payload that is not JSON: the member the payload is
/// carried in, the fixed text, and the reading of where the parse stopped. The ONE place
/// this sentence is written, so a create and an edit word the same error class the same way.
fn invalid_payload_message(reading: &str) -> String {
    format!("{PAYLOAD_MEMBER} is not valid JSON: {reading}")
}

/// The refusal message for the FIRST descriptor payload of `descriptors` this service does
/// not admit, or `None` when every payload the set carries is content it does.
///
/// Called with the descriptors a request would leave on the record — the whole set, so the
/// rule holds for every descriptor type and for the payloads nested in a descriptor's
/// `additionalData` as much as for the top-level one. Every write path that accepts a
/// descriptor goes through this one function, so a create and an edit admit the same
/// payloads.
pub fn refusal(descriptors: Option<&Descriptors>) -> Option<String> {
    unparsed(descriptors).or_else(|| super::mcp_server_schema::refusal(descriptors))
}

/// The refusal for the first payload of the set that is not a JSON document at all, or
/// `None` when every payload the set carries parses.
fn unparsed(descriptors: Option<&Descriptors>) -> Option<String> {
    payloads(descriptors?).into_iter().find_map(|payload| {
        let error = ::serde_json::from_str::<::serde_json::Value>(payload).err()?;
        Some(invalid_payload_message(&reading(payload, &error)))
    })
}

/// Where `payload` stopped parsing, in the words this service's refusal quotes.
///
/// `error` is the parse that refused `payload`: call this where that parse failed, and
/// nowhere else.
fn reading(payload: &str, error: &::serde_json::Error) -> String {
    // A payload that stops on an unquoted word where a VALUE belongs is reported by naming
    // that word, which is what lets a caller whose descriptor was built by templating or
    // concatenation find the fault; every other fault names the character the parse stopped
    // on, or the end of the input, and what was expected there.
    match fault::unrecognized_token(payload, error) {
        Some((word, _line, _column)) => format!(
            "Unrecognized token '{word}': was expecting ({})",
            fault::VALUE_CLASSES
        ),
        None => fault::unexpected_input(payload, error),
    }
}

/// Every `DescriptorData` payload the set carries, in the order `Descriptors` declares its
/// members, each descriptor's own payload before the ones nested under its `additionalData`.
/// `HttpDescriptor` and `AgUiDescriptor` carry none: they are source-only, so their content
/// is synchronized from the source rather than supplied.
///
/// EVERY descriptor of the set whose model declares a `DescriptorData` member is walked
/// here: a descriptor this list skipped would be one whose payload is admitted unchecked
/// while every other descriptor's is refused.
fn payloads(descriptors: &Descriptors) -> Vec<&str> {
    let mut found: Vec<&str> = Vec::new();
    if let Some(descriptor) = &descriptors.mcp_server {
        found.extend(descriptor.data.as_deref());
        if let Some(tools) = descriptor
            .additional_data
            .as_ref()
            .and_then(|additional| additional.tools.as_ref())
        {
            found.extend(tools.data.as_deref());
        }
    }
    if let Some(descriptor) = &descriptors.a2a_agent_card {
        found.extend(descriptor.data.as_deref());
    }
    if let Some(descriptor) = &descriptors.agent_skills_definition {
        found.extend(descriptor.data.as_deref());
        if let Some(skill_md) = descriptor
            .additional_data
            .as_ref()
            .and_then(|additional| additional.skill_md.as_ref())
        {
            found.extend(skill_md.data.as_deref());
        }
    }
    if let Some(descriptor) = &descriptors.custom {
        found.extend(descriptor.data.as_deref());
    }
    if let Some(descriptor) = &descriptors.signature_agent_card {
        found.extend(descriptor.data.as_deref());
    }
    found
}
