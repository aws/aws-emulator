// SPDX-License-Identifier: CC0-1.0

//! Where this service reports the detail text of a failure: the `Message` member of the
//! error body.
//!
//! ConnectHealth answers a refused call with the status, the error-type discriminator and
//! the exception's detail text in a member spelled `Message`:
//!
//! ```json
//! HTTP/1.1 404 Not Found
//! content-type: application/json
//! x-amzn-errortype: ResourceNotFoundException
//!
//! {"Message":"Subscription not found: dom-0123456789abcdef0123/sub-000000000000000000000"}
//! ```
//!
//! Every exception this service's model declares carries exactly one member, `message`
//! targeting `String`, and NO `@jsonName`
//! (`smithy_query(query="shape",shape="ResourceNotFoundException,ValidationException,AccessDeniedException,InternalServerException,ServiceQuotaExceededException,ThrottlingException,ConflictException",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! so the framework's restJson1 renderers — which fall back to the Smithy member name when
//! a member declares no `@jsonName` — write that text under the key `message`, and the
//! `Message` member a ConnectHealth client reads is absent from the body entirely. The same
//! holds for the two error bodies the FRAMEWORK renders rather than a handler: the boundary
//! refusal raised for a value outside a member's declared bound, and the scaffold's 501,
//! both of which spell the member `message` in their own synthetic shapes.
//!
//! This module therefore renames that one member on the error bodies of this service, so
//! the detail text is readable wherever it was produced. Everything else about the response
//! is left exactly as the renderer wrote it: the status, the discriminator (`__type` and the
//! `x-amzn-errortype` header), every other member, every header, and the whole success
//! path. A failure answered before any operation is served — a request naming no modelled
//! operation, a transport-level failure, a pre-dispatch authentication refusal — is not
//! reached from here, because this layer is an operation wrapper.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The body member ConnectHealth reports an exception's detail text in.
pub const DETAIL_MEMBER: &str = "Message";

/// The body member the framework's renderers write that text in: the Smithy member name
/// of every exception shape in this model (`message`), which is also the name the
/// synthetic framework shapes use.
const RENDERED_DETAIL_MEMBER: &str = "message";

/// The error body with its detail member renamed to [`DETAIL_MEMBER`], or `None` when
/// there is nothing to rename: the body is not a JSON object, or it carries no
/// `message` member.
///
/// Idempotent, and non-destructive: a body that already carries a `Message` member keeps
/// that value, so a modelled member of the error shape always wins over the renamed one.
fn with_detail_member(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let detail = object.remove(RENDERED_DETAIL_MEMBER)?;
    object.entry(DETAIL_MEMBER).or_insert(detail);
    serde_json::to_vec(&document).ok()
}

/// The response as ConnectHealth answers it: an error body whose detail text is in its
/// `Message` member.
///
/// A response that is not a failure, or whose body is not an error body carrying a
/// rendered detail member (see [`with_detail_member`]), is returned untouched — status,
/// headers and every byte of the body exactly as the renderer wrote them.
pub fn with_service_detail_member(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < 400 {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match with_detail_member(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        let _ = resp.headers_mut().try_insert("content-length", length);
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

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

/// The detail text of the `ResourceNotFoundException` answered when a patient insights
/// job cannot be resolved under a domain that does resolve.
///
/// RECORDED behaviour: the patient-insights endpoint names the job by its ARN — `Job
/// <jobArn> does not exist.` — rather than by the bare `jobId`, so a caller reading the
/// sentence has the whole resource path the lookup failed on. The fixed text is `Job `
/// and ` does not exist.`; the ARN between them is the one this service's own ARN rule
/// builds for the resolved domain and the requested `jobId`
/// ([`crate::state::patient_insights_job_arn`]), never a value echoed from the request
/// unchanged — and it still goes out through this module's one bounded rendering
/// ([`echoed`]), as every sentence here does.
pub fn patient_insights_job_not_found_message(job_arn: &str) -> String {
    format!("Job {} does not exist.", echoed(job_arn))
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

/// The operation the generated routing table names for this request.
fn modeled_operation<'s>(
    svc: &'s dyn Service,
    req: &'s RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`ErrorDetailOperation`].
pub fn routes<Eng: Send + Sync + 'static>(
    entry: &crate::ServiceDefinition<Eng>,
    req: &RtRequest,
) -> bool {
    entry
        .protocol()
        .routing_key(req)
        .ok()
        .and_then(|key| entry.routing_table().get(key))
        .is_some()
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then reports the detail text of whatever it answered in this service's `Message` member.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk and every status,
/// code and text it answers with; this wrapper owns only which member that text is in.
pub struct ErrorDetailOperation;

impl ErasedOperation for ErrorDetailOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match modeled_operation(svc, req) {
                Some(op) => with_service_detail_member(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is
        // the operation this wrapper delegates to, so its model-declared schemes decide
        // and this wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("the rewritten body is JSON")
    }

    /// One row per error body this service can answer with — every exception the model
    /// declares, at the status it declares, plus the two bodies the framework renders
    /// itself — so the rename is not keyed to the one the issue recorded.
    #[test]
    fn every_error_body_reports_its_detail_text_in_the_message_member() {
        for (code, detail) in [
            ("ResourceNotFoundException", "Subscription sub-x was not found in domain dom-y"),
            ("ValidationException", "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"),
            ("AccessDeniedException", "no credentials"),
            ("InternalServerException", "could not reach this service's store"),
            ("ServiceQuotaExceededException", "too many domains"),
            ("ThrottlingException", "slow down"),
            ("ConflictException", "a job with that token is running"),
            // The framework's own signals, rendered by the scaffold channel and the
            // boundary validator rather than by a handler.
            ("NotImplemented", "operation X is scaffolded but not implemented"),
        ] {
            let body = format!(r#"{{"__type":"{code}","message":"{detail}"}}"#);
            let rewritten = with_detail_member(body.as_bytes())
                .unwrap_or_else(|| panic!("case {code:?}: expected a rename, got no rewrite"));
            let document = decoded(&rewritten);
            assert_eq!(
                document.get(DETAIL_MEMBER).and_then(Value::as_str),
                Some(detail),
                "case {code:?}: expected the detail text in {DETAIL_MEMBER}, got {document}"
            );
            assert!(
                document.get(RENDERED_DETAIL_MEMBER).is_none(),
                "case {code:?}: the body must carry the text under ONE key, got {document}"
            );
            assert_eq!(
                document.get("__type").and_then(Value::as_str),
                Some(code),
                "case {code:?}: the discriminator must survive, got {document}"
            );
        }
    }

    /// Nothing to rename: a body that carries no rendered detail member, one that already
    /// spells it the way this service does, one that is not an object, and one that is not
    /// JSON at all, are all left alone.
    #[test]
    fn a_body_with_nothing_to_rename_is_left_alone() {
        for (label, body) in [
            ("no detail member", r#"{"__type":"ThrottlingException"}"#),
            ("already renamed", r#"{"__type":"ThrottlingException","Message":"slow down"}"#),
            ("not an object", r#"["ResourceNotFoundException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                with_detail_member(body.as_bytes()).is_none(),
                "case {label:?}: expected no rewrite"
            );
        }
    }

    /// An EMPTY detail member is renamed as the empty value it is, never dropped and
    /// never filled in: the front door's authorizer failure, which `DeleteDomain` answers
    /// for an id with no allocation history, carries `message` as an explicit JSON null,
    /// and a client branching on whether a failure carries a diagnosis has to read that
    /// null back rather than an absent member or an empty string.
    #[test]
    fn an_empty_detail_member_is_renamed_as_the_null_it_is() {
        let body = r#"{"__type":"AuthorizerConfigurationException","message":null}"#;
        let document = decoded(&with_detail_member(body.as_bytes()).expect("a rename"));
        assert_eq!(
            document.get(DETAIL_MEMBER),
            Some(&Value::Null),
            "the empty detail member must survive the rename as null, got {document}"
        );
        assert!(
            document.get(RENDERED_DETAIL_MEMBER).is_none(),
            "the body must carry the member under ONE key, got {document}"
        );
    }

    /// A body carrying BOTH spellings keeps the one the error shape modelled.
    #[test]
    fn a_modelled_message_member_wins_over_the_renamed_one() {
        let body = r#"{"__type":"ValidationException","Message":"modelled","message":"rendered"}"#;
        let document = decoded(&with_detail_member(body.as_bytes()).expect("a rewrite"));
        assert_eq!(
            document.get(DETAIL_MEMBER).and_then(Value::as_str),
            Some("modelled"),
            "the modelled member must survive, got {document}"
        );
        assert!(
            document.get(RENDERED_DETAIL_MEMBER).is_none(),
            "the rendered member must not remain beside it, got {document}"
        );
    }

    /// A successful response is not an error body: the rename never reaches it.
    #[test]
    fn a_success_is_returned_untouched() {
        let body = r#"{"subscription":{"status":"ACTIVE","message":"not an error"}}"#;
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_service_detail_member(resp);
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
        );
    }

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
                "patient_insights_job_not_found_message",
                patient_insights_job_not_found_message(&huge),
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
                "patient_insights_job_not_found_message",
                patient_insights_job_not_found_message(&job_arn),
                format!("Job {job_arn} does not exist."),
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
