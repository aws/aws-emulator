// SPDX-License-Identifier: CC0-1.0

//! The CLASS of the fault every error body of this service names, in its `Type` member.
//!
//! # The rule
//!
//! Every error shape this service's model declares carries a `Type` member — "The
//! exception type"
//! (`smithy_query(query="shape",shape="InvalidParameterValueException,ResourceNotFoundException,ResourceConflictException,ServiceException,TooManyRequestsException",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`)
//! — and what it states is whose fault the failure is: the request's, or the service's.
//! The model already classifies that for every one of them: an `@error("client")` shape
//! carries a 4xx `@httpError` and an `@error("server")` shape a 5xx one, so the status
//! class of a failure IS its fault class. So an error body of this service reads
//!
//! ```json
//! HTTP/1.1 400 Bad Request
//! content-type: application/json
//!
//! {"Type":"User","message":"One or more of your tags had a problem, …"}
//! ```
//!
//! and a failure inside the service names [`SERVICE_FAULT`] there instead.
//!
//! # Why it is stated here
//!
//! The class is a property of the failure, not of the call that hit it, so no raise site
//! carries it: a handler builds the modelled error shape with its message and leaves the
//! member unset, and the JSON codec writes nothing at all for an unset member — so every
//! refusal this service answered went out with no `Type` member, whatever its status. The
//! error bodies the FRAMEWORK renders (the pre-handler refusal of a value outside a
//! declared bound, and the scaffold's 501) are built from synthetic shapes that do not
//! declare the member at all, so they could not carry it either.
//!
//! This module therefore names the class on the error bodies of this service, wherever
//! they were produced: one place that sees a whole rendered response, rather than the
//! hundreds of raise sites, so no later handler can forget it. The status, the error code,
//! the message, every other member, every header and the whole success path stay with the
//! renderer that owns them; a body that already names a class keeps the one it names, and
//! the body is re-serialized through `serde_json`, so its member names and values survive
//! while its byte-level layout is that serializer's.
//!
//! The same mechanism, for the same kind of reason, is stated in
//! `service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706`
//! (`src/service/error_message_member.rs`) and
//! `service-com-amazonaws-connecthealth-connecthealth` (`src/service/error_detail.rs`);
//! only the member and the value differ. The structural home for it is the error renderer
//! itself, which is in the agent-immutable `vendor/` tree, so this service states it at the
//! seams that can see a whole rendered response: the operation wrapper every generated
//! modelled operation is served behind, and the one hand-written entry served outside that
//! wrapper (see below).
//!
//! # What it does NOT reach
//!
//! That wrapper is this service's GENERATED modelled-operation surface, so two answers are
//! outside it and keep the envelope their own renderer wrote:
//!
//! * the DOWNLOAD plane (`crate::service::Downloads`), which serves a deployment package
//!   or a layer archive rather than a modelled operation — its refusals are object-fetch
//!   refusals, not Lambda API errors, so a Lambda error shape's member has no place in
//!   them;
//! * the routing-error and framework-error paths, which answer a request that named no
//!   modelled operation or failed at the transport, so no error shape of this model is
//!   what they render.
//!
//! `InvokeWithResponseStream` is a modelled operation the wrapper does not serve — its
//! output binds a `@streaming` union, so it is served by its own hand-written entry — and
//! its errors are error shapes of this model like any other, so that entry names the class
//! on them through [`with_fault_class`] itself
//! (`crate::ops::invoke_with_response_stream`).
//!
//! And two answers inside that surface are held back from it deliberately: the OPAQUE
//! refusal of a service-minted token (`crate::service::opaque_members`), whose recorded body
//! is its fixed sentence and no other member at all, and the refusals listed in
//! [`NAMES_NO_CLASS`], whose recorded bodies carry their message alone.

use aws_smithy_runtime_api::http::Response as RtResponse;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;

/// The class a failure the CALLER can fix names: a request this service refused.
pub const USER_FAULT: &str = "User";

/// The class a failure inside the service names.
pub const SERVICE_FAULT: &str = "Service";

/// The error-body member the class travels in, as every error shape of this model spells
/// it.
const FAULT_CLASS_MEMBER: &str = "Type";

/// The response header a renderer uses to advertise its body's length.
const CONTENT_LENGTH: &str = "content-length";

/// The member every error body of this service names its error shape in.
const SHAPE_MEMBER: &str = "__type";

/// One refusal whose recorded body names NO class of fault.
struct NamesNoClass {
    /// The operation that answers it, as the model names it.
    operation: &'static str,
    /// The error shape it is answered as, as the model names it.
    shape: &'static str,
}

/// Every refusal of this service that carries its message ALONE, one row each.
///
/// The member is optional on every error shape this model declares, and AWS populates it on
/// most of this service's refusals but not on all of them, so which refusals name a class is
/// recorded rather than derived: a row here is a refusal whose recorded body has no `Type`
/// member at all.
///
/// The rows are the REVISION PRECONDITION of the resource-policy operations — the refusal
/// for a `RevisionId` that is not the stored policy's, which both operations taking that
/// precondition answer identically (corpus `simparity-wire-aws-v1`, fixtures
/// `lambda-resource-policy-revision-mismatch`,
/// `lambda-resource-policy-conditional-update-and-stale-revision` and
/// `lambda-resource-policy-precondition-conflict`) — the refusal of the CODE-SIGNING
/// CONFIGURATION a request addresses by its ARN, and the PARAMETER refusal of
/// `ListVersionsByFunction`. The same shape raised by another operation of this service —
/// the `RevisionId` precondition of `AddPermission` and `RemovePermission`, recorded WITH the
/// member, and the parameter refusal of the sibling listings, recorded with it too — is not
/// matched by these rows, which is why a row names the operation as well as the shape.
const NAMES_NO_CLASS: &[NamesNoClass] = &[
    NamesNoClass {
        operation: "PutResourcePolicy",
        shape: "PreconditionFailedException",
    },
    NamesNoClass {
        operation: "DeleteResourcePolicy",
        shape: "PreconditionFailedException",
    },
    // The refusal of the CODE-SIGNING CONFIGURATION the request addresses: the four
    // operations that take a configuration as their own resource bind `CodeSigningConfigArn`
    // as their `@httpLabel`
    // (`smithy_query(query="operation",operation="GetCodeSigningConfig,UpdateCodeSigningConfig,DeleteCodeSigningConfig,ListFunctionsByCodeSigningConfig",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`),
    // and a refusal about the configuration so addressed — it is not there
    // (`ResourceNotFoundException`), or another operation holds it (the
    // `ResourceConflictException` only the deletion models) — is recorded with its message
    // alone (corpus `simparity-wire-aws-v1`, fixtures
    // `lambda-code-signing-config-enforcement`, `lambda-codesigning-config-crud-and-list`,
    // `lambda-function-codesigning-attach-detach-lifecycle` and
    // `lambda-function-codesigning-attach-error-paths`). The FUNCTION-scoped code-signing
    // operations — which address a function and name a configuration in the body, if at all
    // — are recorded WITH the member, which is why these rows name those four operations
    // and no other.
    NamesNoClass {
        operation: "GetCodeSigningConfig",
        shape: "ResourceNotFoundException",
    },
    NamesNoClass {
        operation: "UpdateCodeSigningConfig",
        shape: "ResourceNotFoundException",
    },
    NamesNoClass {
        operation: "DeleteCodeSigningConfig",
        shape: "ResourceNotFoundException",
    },
    NamesNoClass {
        operation: "DeleteCodeSigningConfig",
        shape: "ResourceConflictException",
    },
    NamesNoClass {
        operation: "ListFunctionsByCodeSigningConfig",
        shape: "ResourceNotFoundException",
    },
    // The PARAMETER refusal of `ListVersionsByFunction`, recorded with its message alone
    // (corpus `simparity-wire-aws-v1`, fixture
    // `lambda-listing-qualified-identifier-and-missing-function-errors`, step `task#8`,
    // where a `FunctionName` carrying a version number is answered
    // `400 InvalidParameterValueException` with no `Type` member at all). The same shape
    // raised by the sibling listings — `ListAliases`, `ListFunctionEventInvokeConfigs`,
    // `ListFunctionUrlConfigs` — IS recorded with the member, which is why a row names the
    // operation as well as the shape.
    NamesNoClass {
        operation: "ListVersionsByFunction",
        shape: "InvalidParameterValueException",
    },
];

/// The error shape `body` names, as the model names it: the shape member every error body of
/// this service carries, with the namespace a renderer may qualify it with dropped.
fn named_shape(body: &Value) -> Option<&str> {
    let named = body.get(SHAPE_MEMBER)?.as_str()?;
    Some(match named.rsplit_once('#') {
        Some((_, shape)) => shape,
        None => named,
    })
}

/// Whether the answer `operation` gave, whose body is `body`, is one of the refusals that
/// names no class of fault ([`NAMES_NO_CLASS`]).
fn names_no_class(operation: &str, body: &Value) -> bool {
    let Some(shape) = named_shape(body) else {
        return false;
    };
    NAMES_NO_CLASS
        .iter()
        .any(|refusal| refusal.operation == operation && refusal.shape == shape)
}

/// The class of fault a response at `status` reports, or `None` for a status that reports
/// no failure at all.
///
/// The model's own classification: a client error is answered at a 4xx status and a server
/// error at a 5xx one, so the status class is what says which of the two a body names.
#[must_use]
pub fn fault_class(status: u16) -> Option<&'static str> {
    match status {
        400..=499 => Some(USER_FAULT),
        500..=599 => Some(SERVICE_FAULT),
        _ => None,
    }
}

/// The error body with `class` named in its [`FAULT_CLASS_MEMBER`], or `None` when there
/// is nothing to name it on: the body is not a JSON object, it already names one, or it is
/// one of the refusals `operation` answers with its message alone ([`NAMES_NO_CLASS`]).
///
/// Idempotent, and non-destructive: a body that already carries the member keeps the value
/// it carries, so a handler that states the class itself always wins.
fn naming_the_fault_class(operation: &str, body: &[u8], class: &str) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    if names_no_class(operation, &document) {
        return None;
    }
    let object = document.as_object_mut()?;
    if object.contains_key(FAULT_CLASS_MEMBER) {
        return None;
    }
    object.insert(
        FAULT_CLASS_MEMBER.to_string(),
        Value::String(class.to_string()),
    );
    serde_json::to_vec(&document).ok()
}

/// The response as this service answers it: an error body naming the class of its fault.
///
/// `operation` is the operation that answered, because which refusals name a class is
/// recorded per refusal ([`NAMES_NO_CLASS`]).
///
/// A response that reports no failure, or whose body is not a JSON object (a streamed
/// body, an empty one, a download the service served), is returned untouched — status,
/// headers and every byte of the body exactly as the renderer wrote them.
#[must_use]
pub fn with_fault_class(operation: &str, mut resp: RtResponse) -> RtResponse {
    let Some(class) = fault_class(resp.status().as_u16()) else {
        return resp;
    };
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match naming_the_fault_class(operation, bytes, class) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get(CONTENT_LENGTH).is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if
        // it ever did, the header is REMOVED rather than left describing the body before
        // the member was added, so the two can never disagree.
        if resp
            .headers_mut()
            .try_insert(CONTENT_LENGTH, length)
            .is_err()
        {
            resp.headers_mut().remove(CONTENT_LENGTH);
        }
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response at `status` carrying `body`.
    fn response(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("the rewritten body is JSON")
    }

    /// The class named at each status this service's error shapes are answered at — every
    /// `@httpError` the model pins, one row each, plus the statuses no error of this
    /// service carries, so the rule is a property of the status class rather than of the
    /// one status the issue recorded.
    #[test]
    fn every_failure_status_names_its_own_fault_class() {
        let cases: &[(u16, Option<&str>)] = &[
            // The modelled client errors: 400 InvalidParameterValueException,
            // 404 ResourceNotFoundException, 409 ResourceConflictException,
            // 412 PreconditionFailedException, 429 TooManyRequestsException.
            (400, Some(USER_FAULT)),
            (403, Some(USER_FAULT)),
            (404, Some(USER_FAULT)),
            (409, Some(USER_FAULT)),
            (412, Some(USER_FAULT)),
            (429, Some(USER_FAULT)),
            // The modelled server errors: 500 ServiceException, 502 the KMS and mount
            // failures, and the scaffold's own 501.
            (500, Some(SERVICE_FAULT)),
            (501, Some(SERVICE_FAULT)),
            (502, Some(SERVICE_FAULT)),
            // Not failures at all: every success this service answers.
            (200, None),
            (201, None),
            (202, None),
            (204, None),
            (399, None),
        ];
        for (status, expected) in cases {
            let actual = fault_class(*status);
            assert_eq!(
                actual, *expected,
                "case {status}: expected the fault class {expected:?}, got {actual:?}"
            );
        }
    }

    /// One row per class of error body this service can answer with — a modelled
    /// exception at each status, and the two bodies the framework renders itself — so the
    /// member is a property of the error envelope rather than of one refusal. Every row is
    /// answered by an operation that is not among [`NAMES_NO_CLASS`].
    #[test]
    fn every_error_body_names_the_class_of_its_fault() {
        let cases: &[(u16, &str, &str, &str)] = &[
            (
                400,
                "InvalidParameterValueException",
                "The qualifier '1' in 'functionName' names a version or alias",
                USER_FAULT,
            ),
            (
                404,
                "ResourceNotFoundException",
                "Function not found: arn:aws:lambda:us-east-1:000000000000:function:absent",
                USER_FAULT,
            ),
            (
                409,
                "ResourceConflictException",
                "Function already exist: probe",
                USER_FAULT,
            ),
            (
                429,
                "TooManyRequestsException",
                "Rate exceeded",
                USER_FAULT,
            ),
            (
                500,
                "ServiceException",
                "Lambda state is not reachable on this engine",
                SERVICE_FAULT,
            ),
            // The framework's own signals, rendered by the boundary validator and the
            // scaffold channel rather than by a handler.
            (
                400,
                "ValidationException",
                "1 validation error detected: Value 'bad name!' at 'functionName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern",
                USER_FAULT,
            ),
            (
                501,
                "NotImplemented",
                "operation Invoke is scaffolded but not implemented in this emulator",
                SERVICE_FAULT,
            ),
        ];
        for (status, code, text, expected) in cases {
            let body = format!(r#"{{"__type":"{code}","message":"{text}"}}"#);
            let answered = with_fault_class("AddPermission", response(*status, &body));
            let document = decoded(answered.body().bytes().expect("an in-memory body"));
            assert_eq!(
                document.get(FAULT_CLASS_MEMBER).and_then(Value::as_str),
                Some(*expected),
                "case {code:?} at {status}: expected the class {expected:?}, got {document}"
            );
            assert_eq!(
                document.get("message").and_then(Value::as_str),
                Some(*text),
                "case {code:?} at {status}: the message must survive, got {document}"
            );
            assert_eq!(
                document.get("__type").and_then(Value::as_str),
                Some(*code),
                "case {code:?} at {status}: the discriminator must survive, got {document}"
            );
        }
    }

    /// The refusals recorded WITHOUT the member keep their message alone, and the SAME
    /// error shape raised by any other operation still names its class — one row per
    /// listed refusal, plus the operations that share its shape and its status.
    #[test]
    fn a_refusal_recorded_without_the_member_keeps_its_message_alone() {
        /// A shape no row of [`NAMES_NO_CLASS`] names, for the rows that check a refusal
        /// the table does not match.
        const OTHER_SHAPE: &str = "TooManyRequestsException";

        let mut cases: Vec<(String, &str, &str, Option<&str>)> = Vec::new();
        for refusal in NAMES_NO_CLASS {
            cases.push((
                format!("{} answering {}", refusal.operation, refusal.shape),
                refusal.operation,
                refusal.shape,
                None,
            ));
            // The same operation answering another shape, and another operation answering
            // the same shape, both still name the class: a row is the pair, not either
            // half of it.
            cases.push((
                format!("{} answering another shape", refusal.operation),
                refusal.operation,
                OTHER_SHAPE,
                Some(USER_FAULT),
            ));
        }
        // One operation per listed shape that is NOT listed with it: the permission
        // operations take the same revision precondition, and the function-scoped
        // code-signing operations raise the same not-found and conflict shapes about a
        // configuration they merely name.
        for (operation, shape) in [
            ("AddPermission", "PreconditionFailedException"),
            ("RemovePermission", "PreconditionFailedException"),
            ("PutFunctionCodeSigningConfig", "ResourceNotFoundException"),
            ("PutFunctionCodeSigningConfig", "ResourceConflictException"),
            ("GetFunctionCodeSigningConfig", "ResourceNotFoundException"),
            (
                "DeleteFunctionCodeSigningConfig",
                "ResourceConflictException",
            ),
            ("CreateCodeSigningConfig", "InvalidParameterValueException"),
            ("ListCodeSigningConfigs", "InvalidParameterValueException"),
        ] {
            cases.push((
                format!("{operation} answering {shape}"),
                operation,
                shape,
                Some(USER_FAULT),
            ));
        }
        for (case, operation, shape, expected) in cases {
            let text = "The value of RevisionId provided doesn't match the policy's latest \
                        revision ID.";
            let body = format!(r#"{{"__type":"{shape}","message":"{text}"}}"#);
            let answered = with_fault_class(operation, response(412, &body));
            let document = decoded(answered.body().bytes().expect("an in-memory body"));
            assert_eq!(
                document.get(FAULT_CLASS_MEMBER).and_then(Value::as_str),
                expected,
                "case {case:?}: expected the class {expected:?}, got {document}"
            );
            assert_eq!(
                document.get("message").and_then(Value::as_str),
                Some(text),
                "case {case:?}: the message must survive, got {document}"
            );
        }
    }

    /// Nothing to name a class on: a body that already names one keeps it, a body that is
    /// not an object and one that is not JSON at all are left alone, and so is every
    /// success.
    #[test]
    fn a_body_that_names_no_class_of_its_own_is_left_alone() {
        let already = r#"{"__type":"ResourceInUseException","Type":"Service","Message":"busy"}"#;
        let answered = with_fault_class("GetFunction", response(400, already));
        assert_eq!(
            answered.body().bytes().expect("an in-memory body"),
            already.as_bytes(),
            "a body that names its own class must be byte-identical"
        );

        for (case, status, body) in [
            ("not an object", 400, r#"["InvalidParameterValueException"]"#),
            ("not JSON", 404, "<Error/>"),
            ("an empty body", 400, ""),
            (
                "a success",
                200,
                r#"{"ReservedConcurrentExecutions":null}"#,
            ),
            ("an empty success", 204, ""),
        ] {
            let answered = with_fault_class("GetFunction", response(status, body));
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_bytes(),
                "case {case:?}: expected the body untouched"
            );
        }
    }

    /// A length a renderer advertised is corrected to the body it now describes.
    #[test]
    fn an_advertised_length_describes_the_body_that_names_the_class() {
        let body = r#"{"__type":"InvalidParameterValueException","message":"Invalid parameter."}"#;
        let mut resp = response(400, body);
        resp.headers_mut()
            .insert(CONTENT_LENGTH, body.len().to_string());
        let answered = with_fault_class("GetFunction", resp);
        let bytes = answered.body().bytes().expect("an in-memory body").to_vec();
        assert_eq!(
            answered.headers().get(CONTENT_LENGTH),
            Some(bytes.len().to_string().as_str()),
            "expected the advertised length to describe the rewritten body"
        );
    }
}
