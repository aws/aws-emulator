// SPDX-License-Identifier: CC0-1.0

//! How this service words a request that OMITS a parameter the model requires.
//!
//! The framework finds the absence before any handler runs and describes it structurally
//! rather than as a sentence (a [`Refusal::Invalid`] carrying a
//! [`ViolationKind::MissingRequiredMember`]), because services do not agree on either the
//! code or the prose. ElastiCache states it as a bad parameter value: HTTP 400,
//! `InvalidParameterValue`, and the sentence `The parameter <Name> must be provided.`,
//! naming the parameter by the public name the request would have carried it under.
//!
//! That is one choice for the whole service rather than one per operation, so a client
//! reads the same code and the same shape of sentence whichever required parameter it left
//! out — and the same code it gets for a value the service cannot take.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// The awsQuery error code ElastiCache names a bad parameter value with, which is also
/// what the `InvalidParameterValueException` its handlers answer renders as.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// How this service words `refusal`, or `None` to keep the framework's own wording.
///
/// Only an ABSENT parameter is reworded: every other violation is about a value the
/// framework already states the failed constraint of, and this sentence states nothing
/// about a value. A refusal carrying a violation of any other kind is therefore left alone,
/// so no value-level diagnosis is replaced by a sentence that does not mention it.
pub fn absent_parameter_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if !violations
        .iter()
        .all(|violation| violation.kind == ViolationKind::MissingRequiredMember)
    {
        return None;
    }
    // The service names ONE parameter: the first absence found, which is the parameter a
    // caller fixing the request supplies next.
    let absent = violations.first()?;
    Some(Refused {
        code: INVALID_PARAMETER_VALUE.to_string(),
        message: format!(
            "The parameter {} must be provided.",
            parameter_name(absent)
        ),
    })
}

/// How this service words a parameter a HANDLER found missing: `The parameter
/// <Name> must be provided and must not be null.`
///
/// A member whose requirement is CONDITIONAL — one the model leaves optional
/// because whether the service needs it depends on the rest of the request — is
/// absent only as far as the handler that reads the rest of the request can
/// tell, so the boundary walk above never sees it and the sentence is worded
/// here. ElastiCache states such a requirement with the null the member arrived
/// as beside the absence, and the only variable part is the parameter's own
/// public name; the handler that raises it answers the same
/// `InvalidParameterValue`, HTTP 400, as every other bad parameter value.
pub fn conditionally_required_message(parameter: &str) -> String {
    format!("The parameter {parameter} must be provided and must not be null.")
}

/// The public name of the parameter `violation` is about.
///
/// A violation names its member by the dotted, lowerCamel path the framework spells
/// positions in (`subnetIds`, and a step into a collection as `<member>.<n>.member`). A
/// request to this service carries that parameter under the model's own member name, which
/// is the same name with its first character in upper case — so the leading segment is
/// re-cased and the rest, which describes a position INSIDE the parameter, is dropped.
fn parameter_name(violation: &Violation) -> String {
    let member = violation
        .path
        .split('.')
        .next()
        .unwrap_or(violation.path.as_str());
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}
