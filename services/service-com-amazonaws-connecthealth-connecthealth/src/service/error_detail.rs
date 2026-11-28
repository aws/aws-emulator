// SPDX-License-Identifier: CC0-1.0

//! The detail TEXT of a refused call of this service: the sentences its exceptions carry,
//! stated once each.
//!
//! Every exception this service's model declares carries exactly one member, `message`
//! targeting `String`, and NO `@jsonName`
//! (`smithy_query(query="shape",shape="ResourceNotFoundException,ValidationException,AccessDeniedException,InternalServerException,ServiceQuotaExceededException,ThrottlingException,ConflictException",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! and the KEY that text goes out under is decided by the PLANE that answers rather than
//! by the sentence or the shape — `Message` for the subscription plane and `message` for
//! the others, held once in [`crate::ops::types::exception_wire`]. Nothing here rewrites a
//! rendered body: the status, the discriminator (`__type` and the `x-amzn-errortype`
//! header) and every member are the renderer's, and this module only supplies the sentence
//! a handler puts in that member.
//!
//! ```json
//! HTTP/1.1 404 Not Found
//! content-type: application/json
//! x-amzn-errortype: ResourceNotFoundException
//!
//! {"__type":"ResourceNotFoundException","Message":"Subscription not found: dom-0123456789abcdef0123/sub-000000000000000000000"}
//! ```
//!
//! It also renders, once, the text AWS gives a constraint violation a handler enforces
//! itself ([`range_violation_message`]) — through the framework's own
//! `Violation`/`envelope` renderer, so a page size this service refuses rather than
//! clamps reads exactly like the bound the boundary validator refuses. The bounds on a
//! `TagMap`'s entries, which that walk cannot reach at all, are enforced in
//! [`crate::service::tag_bounds`] by driving the walk itself.
//!
//! Every sentence stated here names the value the request addressed, because that is what
//! tells one refusal from another — and names it through ONE bounded rendering,
//! [`echoed`], which every handler of this service that interpolates a request value into
//! a sentence of its own goes through as well. A value read off the request may not size
//! the response it is answered with, whether or not its member carries a modelled bound:
//! that is the rule [`crate::service::tag_bounds`] keeps for the entries a tag refusal
//! names and [`crate::state`] for the cursor it decodes and the credential-scope region it
//! files a record under, and a refusal built here is held to it in the same way.
//!
//! The two FIXED sentences of the patient-insights runtime endpoint
//! ([`DOMAIN_NOT_AUTHORIZED_MESSAGE`], [`REQUEST_VALIDATION_FAILED_MESSAGE`]) are
//! `const`s rather than constructors for the opposite reason: that endpoint states them
//! with nothing read off the request at all, so there is no value to name and none to
//! bound.

use std::borrow::Cow;

use aws_smithy_schema_server::{envelope, Violation, ViolationKind};

/// The text a constraint violation is reported with: the FRAMEWORK's own renderer.
///
/// `aws_smithy_schema_server` owns both halves of that sentence — [`Violation::message`]
/// renders the clause (`Value '<value>' at '<member path>' failed to satisfy constraint:
/// <requirement>`, with the requirement coming from the [`ViolationKind`]) and
/// [`envelope`] wraps it (`1 validation error detected: <clause>`) — and it is what the
/// boundary walk answers with when it refuses a request before a handler runs. A bound a
/// HANDLER has to enforce itself therefore renders through the same code rather than
/// restating the words, so the two cannot drift.
///
/// `violations` is never empty at a call site (a violation is what brought us here);
/// `envelope` is the one place that decides what no violation reads as, and an empty
/// slice could only mean the caller found nothing to refuse.
fn rendered_violation(violations: &[Violation]) -> String {
    envelope(violations).unwrap_or_default()
}

/// AWS's constraint-violation text for `value` outside the `@range` of the member at
/// `member_path`.
///
/// Rendered by the framework (see [`rendered_violation`]), so a bound a handler enforces
/// itself — a page size it refuses rather than clamps — reads exactly like the one the
/// boundary validator refuses. `@range`'s bounds are held as the text they display with,
/// which is why they are passed through as strings.
pub fn range_violation_message(member_path: &str, value: i32, min: i32, max: i32) -> String {
    let kind = if value < min {
        ViolationKind::MinValue(min.to_string())
    } else {
        ViolationKind::MaxValue(max.to_string())
    };
    rendered_violation(&[Violation {
        path: member_path.to_string(),
        kind,
        value: Some(value.to_string()),
        sensitive: false,
    }])
}

/// How much of a value read off the REQUEST one refusal sentence of this service echoes.
///
/// A refusal names the value the caller addressed, because that is what tells one refusal
/// from another — but not every such value carries a modelled bound: `resourceArn` is a
/// bare `String` and `clientToken` a `NonEmptyString` carrying a `@pattern` and no
/// `@length`
/// (`smithy_query(query="operation",operation="ListTagsForResource,StartPatientInsightsJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
/// so a sentence interpolating one of those verbatim would let the request decide how
/// large the response answering it is — the amplification every other value this service
/// reads off a request is already held away from ([`crate::service::tag_bounds`] for the
/// entries a tag refusal names, [`crate::state`] for the cursor it decodes and the
/// credential-scope region it files a record under).
///
/// The number is the ceiling the FRAMEWORK applies to a value it echoes into a
/// constraint clause (`MAX_ECHOED_VALUE` in
/// `vendor/smithy-schema-server/src/constraints.rs`), counted in bytes as that ceiling
/// counts, so one value is echoed at one size whether the walk refused the request or a
/// handler did.
pub const MAX_ECHOED_VALUE_LEN: usize = 256;

/// `value` as a refusal sentence of this service echoes it: itself when it is within
/// [`MAX_ECHOED_VALUE_LEN`], and otherwise a prefix of that many bytes — cut on a
/// character boundary, never mid-UTF-8 — followed by the length of the whole value, which
/// is the elision the framework renders for an over-long value it refuses, so a caller
/// meeting one value in two refusals meets one rendering of it.
///
/// RENDERING ONLY. Every caller keeps the value it was given for its own comparisons and
/// reaches this solely to interpolate: eliding before a comparison would test text the
/// caller never sent.
pub fn echoed(value: &str) -> Cow<'_, str> {
    if value.len() <= MAX_ECHOED_VALUE_LEN {
        return Cow::Borrowed(value);
    }
    let mut end = MAX_ECHOED_VALUE_LEN;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}... [{} bytes]", &value[..end], value.len()))
}

/// The detail text of the `ResourceNotFoundException` answered when a domain cannot be
/// resolved — because no domain of that id is held, or because the one held is already
/// on its way out.
///
/// Built from the REQUEST: the requested `domainId` is what tells one 404 from another,
/// echoed through the one bounded rendering every sentence of this module echoes a
/// request value through ([`echoed`]). ConnectHealth words it `<resource kind> not found:
/// <identifier>` — the same form [`subscription_not_found_message`] carries for a
/// subscription — so the text is stated ONCE here rather than in each of the handlers
/// that resolve a domain.
pub fn domain_not_found_message(domain_id: &str) -> String {
    format!("Domain not found: {}", echoed(domain_id))
}

/// The detail text of the `ResourceNotFoundException` answered when a Medical Scribe
/// listening session cannot be resolved under the (`domainId`, `subscriptionId`) scope
/// the request named.
///
/// Built from the REQUEST, and in the form ConnectHealth reports for a scoped
/// identifier it cannot resolve: the member that could not be resolved and the value
/// the request gave it.
pub fn scribe_session_not_found_message(session_id: &str) -> String {
    format!(
        "Resource not found: sessionId with ID {} does not exist",
        echoed(session_id)
    )
}

/// The detail text of the `ResourceNotFoundException` answered when a JOB of this service
/// cannot be resolved under a domain that does resolve.
///
/// RECORDED behaviour for the patient-insights pair, and the form every job of this service
/// is named by: the endpoint names the job by its ARN — `Job <jobArn> does not exist.` —
/// rather than by the bare `jobId`, so a caller reading the sentence has the whole resource
/// path the lookup failed on. The fixed text is `Job ` and ` does not exist.`; the ARN
/// between them is the one this service's own ARN rule builds for the resolved domain and
/// the requested `jobId` ([`crate::state::patient_insights_job_arn`],
/// [`crate::state::medical_coding_job_arn`]), never a value echoed from the request
/// unchanged — and it still goes out through this module's one bounded rendering
/// ([`echoed`]), as every sentence here does.
pub fn job_not_found_message(job_arn: &str) -> String {
    format!("Job {} does not exist.", echoed(job_arn))
}

/// The detail text of the `ResourceNotFoundException` answered when a dataset cannot be
/// resolved under a domain that does resolve — because the domain holds no dataset of that
/// id, or because the one it holds is in a status no reader of a dataset reports (the model
/// documents `GetDataset` as answering this for a dataset whose status is `DELETED`).
///
/// The same `<resource kind> not found: <identifier>` form [`domain_not_found_message`] and
/// [`subscription_not_found_message`] carry, with the identifier the request addressed the
/// dataset by, echoed through this module's one bounded rendering ([`echoed`]).
pub fn dataset_not_found_message(dataset_id: &str) -> String {
    format!("Dataset not found: {}", echoed(dataset_id))
}

/// The detail text of the `AccessDeniedException` the patient-insights runtime endpoint
/// answers when the `domainId` a request names resolves to no domain this account holds
/// live in the Region.
///
/// RECORDED behaviour: that endpoint authorizes the domain the request addresses BEFORE
/// it confirms or denies anything about the resource, so the refusal is an
/// authorization failure (HTTP 401) rather than the modelled 404 — and its whole text is
/// FIXED. Nothing read off the request appears in it, deliberately: a sentence that named
/// the requested id would tell an unauthorized caller which ids exist, which is exactly
/// what authorizing the resource ARN before resolving it withholds. A `const` rather than
/// a constructor, because there is no value to build it from.
pub const DOMAIN_NOT_AUTHORIZED_MESSAGE: &str = "Domain is not valid.";

/// The detail text of the `ValidationException` the patient-insights runtime endpoint
/// answers when a value of the request itself fails the rules its shape declares.
///
/// RECORDED behaviour: that endpoint states such a refusal as this one fixed sentence —
/// it names neither the member nor the value, unlike the per-member clause the control
/// plane's boundary walk renders ([`range_violation_message`]) — so a caller reads one
/// sentence whichever member of the request was at fault.
pub const REQUEST_VALIDATION_FAILED_MESSAGE: &str = "Request validation failed";

/// The detail text of the `ResourceNotFoundException` answered when a subscription cannot
/// be resolved — because the subscription is absent, or because its parent domain is.
///
/// Built from the REQUEST, and from both identifiers: the two are what tell one 404 from
/// another, since the same status answers a missing domain and a missing subscription. The
/// `<domainId>/<subscriptionId>` form is the one ConnectHealth itself reports, the same for
/// every pair of identifiers; stated once here so the operations that resolve a
/// subscription — the reader and the two status transitions — cannot word it three ways.
pub fn subscription_not_found_message(domain_id: &str, subscription_id: &str) -> String {
    format!(
        "Subscription not found: {}/{}",
        echoed(domain_id),
        echoed(subscription_id)
    )
}

/// The detail text of the `ResourceNotFoundException` answered when the ARN a tagging
/// call names resolves to no resource this service holds.
///
/// The same `<resource kind> not found: <identifier>` form as the two constructors
/// above, with the identifier the request used — here a resource ARN, since that is
/// what the tagging operations address a resource by.
///
/// That identifier is the one this service echoes which its model bounds in no way at all
/// (`resourceArn` is a bare `String`), so it is the one this module's bounded rendering
/// ([`echoed`]) actually bites on: a caller-sized ARN is named as far as a caller can act
/// on it and no further.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource not found: {}", echoed(resource_arn))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value read off the request is echoed whole while it is within the ceiling and
    /// elided past it, on a character boundary, whatever its content.
    #[test]
    fn a_request_value_is_echoed_up_to_the_ceiling_and_elided_past_it() {
        // One row per size the rule distinguishes, around the ceiling and far past it,
        // in one-byte and multi-byte characters.
        let rows: Vec<(&str, String)> = vec![
            ("empty", String::new()),
            ("an ordinary ARN-sized value", "a".repeat(64)),
            ("one byte short of the ceiling", "a".repeat(MAX_ECHOED_VALUE_LEN - 1)),
            ("exactly the ceiling", "a".repeat(MAX_ECHOED_VALUE_LEN)),
            ("one byte past the ceiling", "a".repeat(MAX_ECHOED_VALUE_LEN + 1)),
            ("far past the ceiling", "a".repeat(64 * MAX_ECHOED_VALUE_LEN)),
            // Multi-byte characters: the ceiling counts bytes, so this is well past it,
            // and the cut may not land inside a character.
            ("multi-byte characters", "é".repeat(MAX_ECHOED_VALUE_LEN)),
            ("a value with no character boundary at the ceiling", format!("{}{}", "a".repeat(MAX_ECHOED_VALUE_LEN - 1), "日".repeat(8))),
        ];
        for (case, value) in &rows {
            let shown = echoed(value);
            if value.len() <= MAX_ECHOED_VALUE_LEN {
                assert_eq!(
                    shown, value.as_str(),
                    "case {case:?}: a value within the ceiling is echoed whole, got {shown:?}"
                );
                continue;
            }
            assert!(
                shown.len() <= MAX_ECHOED_VALUE_LEN + format!("... [{} bytes]", value.len()).len(),
                "case {case:?}: an over-long value must be elided, got {} bytes",
                shown.len()
            );
            assert!(
                shown.ends_with(&format!("... [{} bytes]", value.len())),
                "case {case:?}: the elision must state the whole length, got {shown:?}"
            );
            let prefix = shown
                .strip_suffix(&format!("... [{} bytes]", value.len()))
                .unwrap_or_else(|| panic!("case {case:?}: an elided rendering, got {shown:?}"));
            assert!(
                value.starts_with(prefix) && prefix.len() <= MAX_ECHOED_VALUE_LEN,
                "case {case:?}: the prefix must be the value's own leading characters, \
                 got {prefix:?}"
            );
        }
    }

    /// Every sentence this module builds from a request value echoes it through that one
    /// ceiling — not only the one whose member the model leaves unbounded.
    #[test]
    fn every_sentence_bounds_the_request_value_it_names() {
        let huge = "z".repeat(4 * MAX_ECHOED_VALUE_LEN);
        // One row per constructor, so a sentence added without the bounded echo fails
        // here rather than only for the member the issue named.
        let rows: Vec<(&str, String)> = vec![
            ("domain_not_found_message", domain_not_found_message(&huge)),
            (
                "scribe_session_not_found_message",
                scribe_session_not_found_message(&huge),
            ),
            (
                "job_not_found_message",
                job_not_found_message(&huge),
            ),
            (
                "dataset_not_found_message",
                dataset_not_found_message(&huge),
            ),
            (
                "subscription_not_found_message",
                subscription_not_found_message(&huge, &huge),
            ),
            (
                "resource_not_found_message",
                resource_not_found_message(&huge),
            ),
        ];
        for (case, sentence) in &rows {
            assert!(
                !sentence.contains(huge.as_str()),
                "case {case:?}: the whole request value must not be echoed, got {} bytes",
                sentence.len()
            );
            assert!(
                sentence.contains(&format!("... [{} bytes]", huge.len())),
                "case {case:?}: the sentence must name the value elided, got {sentence:?}"
            );
            // Two echoes of one value at most, so the sentence's size is fixed by the
            // service rather than by the request.
            assert!(
                sentence.len() <= 2 * MAX_ECHOED_VALUE_LEN + 256,
                "case {case:?}: the sentence must not be sized by the request, got {} bytes",
                sentence.len()
            );
        }
    }

    /// A value of ordinary size is still named in full: the ceiling bounds amplification,
    /// it does not change what a caller is told about a request it could plausibly make.
    #[test]
    fn an_ordinary_identifier_is_still_named_in_full() {
        let arn = "arn:aws:health-agent:us-west-2:000000000000:domain/dom-0123456789abcdef0123";
        let domain_id = "dom-0123456789abcdef0123";
        let subscription_id = "sub-000000000000000000000";
        let job_arn = format!("{arn}/patient-insights-job/job-0123456789abcdef");
        for (case, sentence, expected) in [
            (
                "resource_not_found_message",
                resource_not_found_message(arn),
                format!("Resource not found: {arn}"),
            ),
            (
                "domain_not_found_message",
                domain_not_found_message(domain_id),
                format!("Domain not found: {domain_id}"),
            ),
            (
                "subscription_not_found_message",
                subscription_not_found_message(domain_id, subscription_id),
                format!("Subscription not found: {domain_id}/{subscription_id}"),
            ),
            (
                "scribe_session_not_found_message",
                scribe_session_not_found_message(subscription_id),
                format!("Resource not found: sessionId with ID {subscription_id} does not exist"),
            ),
            (
                "job_not_found_message",
                job_not_found_message(&job_arn),
                format!("Job {job_arn} does not exist."),
            ),
            (
                "dataset_not_found_message",
                dataset_not_found_message("0123456789abcdef0123456789abcdef"),
                "Dataset not found: 0123456789abcdef0123456789abcdef".to_string(),
            ),
        ] {
            assert_eq!(
                sentence, expected,
                "case {case:?}: expected {expected:?}, got {sentence:?}"
            );
        }
    }

    /// The constraint sentence names the offending value, the member path and the bound
    /// that was violated — whichever end of the range it fell outside.
    #[test]
    fn the_range_violation_sentence_names_the_value_the_member_and_the_bound() {
        for (value, expected) in [
            (
                101,
                "1 validation error detected: Value '101' at 'maxResults' failed to satisfy \
                 constraint: Member must have value less than or equal to 100",
            ),
            (
                0,
                "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1",
            ),
            (
                -7,
                "1 validation error detected: Value '-7' at 'maxResults' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1",
            ),
        ] {
            let rendered = range_violation_message("maxResults", value, 1, 100);
            assert_eq!(
                rendered, expected,
                "case {value}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
