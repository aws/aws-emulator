// SPDX-License-Identifier: CC0-1.0

//! The QUERY wire form an integration request names a call in, read ONCE for every service
//! of that protocol this module calls.
//!
//! A QUERY api names its action in an `Action` field and that action's inputs in the other
//! fields of an `application/x-www-form-urlencoded` body, which is why an integration to one
//! carries a request template that renders that form. An `AWS` integration puts those fields
//! in three places, and this module is the one reading of all three:
//!
//! * the `action/{Action}&{name}={value}` form of the `uri`, whose `&{name}={value}` tail
//!   fixes parameters in the configuration itself;
//! * the query string the integration MAPS onto the integration request
//!   (`integration.request.querystring.{name}`);
//! * the integration request's own BODY — the request template this call selects, RENDERED
//!   (`Integration$requestTemplates`) — read as the wire form it is written in: the fields of
//!   an `application/x-www-form-urlencoded` body by the maintained reader for that media
//!   type, and the members of a JSON object body, which is the form an AWS JSON protocol
//!   carries the same inputs in.
//!
//! A later source overrides an earlier one, so a value the integration request carries stands
//! over one the `uri` fixes.
//!
//! WHICH call those fields name is each service's own reading of them ([`super::sqs`],
//! [`super::sns`]), because the field that names the resource is that api's; the fields
//! themselves are one rule, stated here.

use std::collections::BTreeMap;

use crate::ops::types::GetIntegrationOutput;
use crate::service::mapping_template::TemplateInputs;

use super::super::{MethodRequest, UnservedIntegration, INTEGRATION_REQUEST_QUERY_PREFIX};
use super::{mapped_value, unanswered};

/// The most bytes of an integration request this module reads as the service's wire form.
///
/// The body is the rendered request template, which the renderer already bounds; this is the
/// second half of the same guard, so a body that arrived some other way (a passed-through
/// payload) is bounded too. A body past it is REFUSED rather than read as a prefix: half a
/// form is a different request from the one the caller sent.
const MAX_FORM_BYTES: usize = 1024 * 1024;

/// Every field the integration request carries, by name, over the three places an `AWS`
/// integration puts one — in the order a later source overrides an earlier.
///
/// `fixed` is the `&{name}={value}` tail of the `uri`'s own `action` form, empty for a `uri`
/// that fixes none; it is a query string, so it is read by the same maintained reader the
/// integration request's own fields are.
pub(super) fn carried_parameters(
    integration: &GetIntegrationOutput,
    call: &MethodRequest,
    fixed: &str,
) -> Result<BTreeMap<String, String>, UnservedIntegration> {
    let mut parameters = BTreeMap::new();
    for (field, value) in form_fields(fixed.as_bytes()) {
        parameters.insert(field, value);
    }
    for (field, value) in mapped_query_parameters(integration, &call.inputs) {
        parameters.insert(field, value);
    }
    for (field, value) in request_fields(&request_body(integration, call)?) {
        parameters.insert(field, value);
    }
    Ok(parameters)
}

/// The query-string fields the integration MAPS onto the integration request, each resolved
/// against this call off the same inputs a mapping template of it is rendered from.
///
/// A mapping whose source this request carries nothing for carries nothing, which is what an
/// absent method request parameter means for every other mapping of this data plane.
fn mapped_query_parameters(
    integration: &GetIntegrationOutput,
    inputs: &TemplateInputs,
) -> Vec<(String, String)> {
    integration
        .request_parameters
        .iter()
        .flatten()
        .filter_map(|(key, source)| {
            let name = key.strip_prefix(INTEGRATION_REQUEST_QUERY_PREFIX)?;
            Some((name.to_string(), mapped_value(source, inputs)?))
        })
        .collect()
}

/// The body the integration request carries, bounded again by what this module reads one AS:
/// the service's own wire form, which it parses to learn the call.
///
/// The rendering itself is the parent module's ([`super::integration_request_body`]), because
/// it is the rule `Integration$requestTemplates` and `Integration$passthroughBehavior` state
/// for every service this data plane calls.
fn request_body(
    integration: &GetIntegrationOutput,
    call: &MethodRequest,
) -> Result<Vec<u8>, UnservedIntegration> {
    let body = super::integration_request_body(integration, call)?;
    if body.len() > MAX_FORM_BYTES {
        return Err(unanswered(&format!(
            "the integration request of {bytes} bytes is larger than the {MAX_FORM_BYTES} \
             bytes this emulator reads one as the service's wire form, so the call it names \
             cannot be read",
            bytes = body.len()
        )));
    }
    Ok(body)
}

/// The fields an integration request body names, read as the service's own wire form
/// carries them: the fields of an `application/x-www-form-urlencoded` body, which is the
/// QUERY form, and the top-level members of a JSON object, which is the form an AWS JSON
/// protocol carries the same inputs in. A body that is neither names no field.
fn request_fields(body: &[u8]) -> Vec<(String, String)> {
    if let Ok(::serde_json::Value::Object(members)) =
        ::serde_json::from_slice::<::serde_json::Value>(body)
    {
        return members
            .into_iter()
            .filter_map(|(field, value)| match value {
                ::serde_json::Value::String(text) => Some((field, text)),
                ::serde_json::Value::Number(number) => Some((field, number.to_string())),
                ::serde_json::Value::Bool(flag) => Some((field, flag.to_string())),
                // A member that is itself a document names no single field of the query
                // form, so it is left to the field the form spells it as.
                _ => None,
            })
            .collect();
    }
    form_fields(body)
}

/// The fields of an `application/x-www-form-urlencoded` body, decoded by the maintained
/// reader for that media type rather than by a split written here — so a field whose value
/// carries a `&`, a `=` or a percent escape is read as the one value it is.
fn form_fields(body: &[u8]) -> Vec<(String, String)> {
    ::form_urlencoded::parse(body)
        .map(|(field, value)| (field.into_owned(), value.into_owned()))
        .collect()
}

/// The same fields written back OUT as that wire form carries them, which is the body an
/// integration request to a query api is made with: every field this call named, encoded by
/// the same maintained coder that read them.
pub(super) fn form_body(parameters: &BTreeMap<String, String>) -> Vec<u8> {
    let mut written = ::form_urlencoded::Serializer::new(String::new());
    for (field, value) in parameters {
        written.append_pair(field, value);
    }
    written.finish().into_bytes()
}
