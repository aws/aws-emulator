// SPDX-License-Identifier: CC0-1.0

//! How this API answers the refusals whose error SHAPE its wire names nowhere.
//!
//! # What the wire shows
//!
//! Most of what this API refuses names its error shape — a network read of a deleted network
//! is a typed `ForbiddenError`, a refused member a typed `ValidationError` — but a few
//! refusals carry the status and a `message` and nothing else: no `x-amzn-errortype` header
//! (which is where `restJson1` resolves an error shape from) and no `__type` / `code` / `Code`
//! member in the body.
//!
//! ```json
//! HTTP/1.1 404 Not Found
//! content-type: application/json; charset=utf-8
//!
//! {"message":"Bot not found"}
//! ```
//!
//! A client therefore classifies such a refusal by STATUS: finding no shape in either place,
//! an SDK reports the error code as the bare status string and none of its modelled-exception
//! classes match. The framework's `restJson1` renderer writes the discriminator in both places
//! for every modelled error (`vendor/smithy-schema-server/src/rest_json.rs`'s
//! `serialize_error`), and `vendor/` is read-only, so this correction lives at the one seam
//! this service owns over the answers its operations write.
//!
//! # What it reaches, and what it deliberately does not
//!
//! One operation's one refusal at a time: [`UntypedRefusalOperation`] is built for the STATUS
//! that operation's untyped refusal carries, and is put in front of that operation alone
//! ([`crate::service`]). Every other answer of that operation — a success, a refusal of
//! another class — and every answer of every other operation keeps the shape its renderer
//! named it with.
//!
//! Nothing here decides WHICH refusals a service names: each is a fact of this API's wire,
//! stated once beside the status it carries
//! ([`crate::service::untyped_bot_refusal`], [`crate::service::untyped_security_group_refusal`]).
//!
//! The status, the sentence, every other member and every other header stay exactly as the
//! framework rendered them, and the whole success path is untouched. The service's response
//! envelope is applied LAST, after the discriminator is gone, because its entity tag is a
//! function of the bytes the client receives.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The response header a `restJson1` client resolves a failure's error shape from, named by
/// the tree's one home for it.
const ERROR_TYPE_HEADER: &str = service_common::error_type_locator::ERROR_TYPE_HEADER;

/// The body members a JSON failure can name its error shape in, in the order a client
/// consults them: `__type` is what this emulator's `restJson1` renderer writes, and
/// `code` / `Code` are the other spellings the same parsers accept.
const DISCRIMINATOR_MEMBERS: [&str; 3] = [
    service_common::error_type_locator::SHAPE_DISCRIMINATOR,
    "code",
    "Code",
];

/// The header a renderer announces a body's length in, which has to be corrected when the
/// body is rewritten under it.
const CONTENT_LENGTH: &str = "content-length";

/// `body` with every error-shape discriminator removed, or `None` when there is nothing to
/// remove: the body is not a JSON object, or it names no shape at all.
///
/// Every other member is written back as it was, so only the discriminator goes.
fn without_discriminator(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    if !DISCRIMINATOR_MEMBERS
        .iter()
        .any(|member| object.contains_key(*member))
    {
        return None;
    }
    for member in DISCRIMINATOR_MEMBERS {
        object.remove(member);
    }
    serde_json::to_vec(&document).ok()
}

/// `resp` as this API answers the refusal `untyped_status` identifies: no `x-amzn-errortype`
/// header, and a body naming no error shape either.
///
/// Any other response — a success, a refusal of another class — is handed back exactly as its
/// renderer wrote it: status, headers and every body byte.
#[must_use]
pub fn untyped(mut resp: RtResponse, untyped_status: u16) -> RtResponse {
    if resp.status().as_u16() != untyped_status {
        return resp;
    }
    // The header and the body are two independent places a client reads the shape from, so
    // each is dropped on its own: a body that cannot be rewritten still loses the header.
    resp.headers_mut().remove(ERROR_TYPE_HEADER);
    let rewritten = {
        // An error body is built in memory by its renderer, so these bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match without_discriminator(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that announced a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get(CONTENT_LENGTH).is_some() {
        let length = rewritten.len().to_string();
        let _ = resp.headers_mut().try_insert(CONTENT_LENGTH, length);
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// Serves the modelled operation this service's routing table names for the request exactly
/// as the framework would, reports the one refusal of it that names no error shape untyped,
/// and answers the result in this service's response envelope.
///
/// It owns no operation and no routing of its own: the one it delegates to is the one the
/// generated routing table names for the request, resolved from the `svc` the framework
/// passes into [`ErasedOperation::run`], so the whole walk — deserialize, validate, handle,
/// serialize — and every status, code, message and body byte it answers with stay the
/// framework's. This wrapper owns only whether that one refusal names its shape on the wire.
#[derive(Debug, Clone, Copy)]
pub struct UntypedRefusalOperation {
    /// The status of the one refusal the fronted operation answers untyped. Every other
    /// status it answers passes through untouched.
    untyped_status: u16,
}

impl UntypedRefusalOperation {
    /// The value a service holds for one operation, carrying the status that operation's
    /// untyped refusal travels at. A service builds it in its constructor, because
    /// `Service::override_operation` hands back a borrow of it.
    #[must_use]
    pub const fn new(untyped_status: u16) -> Self {
        Self { untyped_status }
    }
}

impl ErasedOperation for UntypedRefusalOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let definition = svc.definition();
            let modeled = definition
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| definition.routing_table().get(key));
            let answered = match modeled {
                Some(operation) => untyped(operation.run(svc, req, ctx).await, self.untyped_status),
                // Unreachable through a `Service::override_operation` that claims a request
                // only when the routing table holds its key; answered as the framework
                // answers an unroutable request rather than guessed at.
                None => definition.routing_error_response(req),
            };
            // LAST, over the bytes the client receives: the envelope's entity tag is a
            // function of the body, so it cannot be computed over one a later step
            // rewrites.
            crate::service::response_envelope::enveloped(answered)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is the
        // operation this wrapper delegates to, so its model-declared schemes decide and
        // this wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::untyped_bot_refusal::GONE_BOT_STATUS;
    use crate::service::untyped_security_group_refusal::REFUSED_SECURITY_GROUP_READ_STATUS;
    use crate::state::{BOT_GONE_MESSAGE, SECURITY_GROUP_READ_REFUSED_MESSAGE};

    /// A response at `status` carrying `body`, named by `error_type` when a renderer named
    /// the shape in the header.
    fn response(status: u16, body: &str, error_type: Option<&str>) -> RtResponse {
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        );
        if let Some(error_type) = error_type {
            resp.headers_mut().insert(ERROR_TYPE_HEADER, error_type.to_string());
        }
        resp
    }

    /// The body of `resp` as JSON.
    fn decoded(resp: &RtResponse) -> Value {
        serde_json::from_slice(resp.body().bytes().expect("an in-memory body"))
            .expect("the body is JSON")
    }

    /// RULE: a refusal this service names no error shape in names it nowhere. The rows differ
    /// along what the rule generalises over: which of this API's untyped refusals it is (and
    /// so which status it travels at), which spelling the renderer named the shape in,
    /// whether the header named it too, and whether the shape name carries a namespace.
    #[test]
    fn an_untyped_refusal_names_its_shape_in_neither_place() {
        // (case, the status it travels at, the body as rendered, the header as rendered, the
        // sentence that must survive)
        let rows: &[(&str, u16, &str, Option<&str>, &str)] = &[
            (
                "the discriminator the restJson1 renderer writes",
                GONE_BOT_STATUS,
                r#"{"__type":"ResourceNotFoundError","message":"Bot not found"}"#,
                Some("ResourceNotFoundError"),
                BOT_GONE_MESSAGE,
            ),
            (
                "a namespaced discriminator",
                GONE_BOT_STATUS,
                r#"{"__type":"com.amazonaws.wickr#ResourceNotFoundError","message":"Bot not found"}"#,
                Some("com.amazonaws.wickr#ResourceNotFoundError"),
                BOT_GONE_MESSAGE,
            ),
            (
                "the shape in the header alone",
                GONE_BOT_STATUS,
                r#"{"message":"Bot not found"}"#,
                Some("ResourceNotFoundError"),
                BOT_GONE_MESSAGE,
            ),
            (
                "the lowercase code spelling",
                GONE_BOT_STATUS,
                r#"{"code":"ResourceNotFoundError","message":"Bot not found"}"#,
                None,
                BOT_GONE_MESSAGE,
            ),
            (
                "the capitalised Code spelling",
                GONE_BOT_STATUS,
                r#"{"Code":"ResourceNotFoundError","message":"Bot not found"}"#,
                None,
                BOT_GONE_MESSAGE,
            ),
            (
                "a refused security-group read",
                REFUSED_SECURITY_GROUP_READ_STATUS,
                r#"{"__type":"UnauthorizedError","message":"invalid security group request"}"#,
                Some("UnauthorizedError"),
                SECURITY_GROUP_READ_REFUSED_MESSAGE,
            ),
            (
                "a refused security-group read named in the header alone",
                REFUSED_SECURITY_GROUP_READ_STATUS,
                r#"{"message":"invalid security group request"}"#,
                Some("UnauthorizedError"),
                SECURITY_GROUP_READ_REFUSED_MESSAGE,
            ),
        ];

        for (case, status, body, error_type, sentence) in rows {
            let answered = untyped(response(*status, body, *error_type), *status);
            assert_eq!(
                answered.status().as_u16(),
                *status,
                "case {case:?}: the status stays the renderer's, got {}",
                answered.status().as_u16()
            );
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                None,
                "case {case:?}: the header must name no shape, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            let document = decoded(&answered);
            assert!(
                DISCRIMINATOR_MEMBERS
                    .iter()
                    .all(|member| document.get(*member).is_none()),
                "case {case:?}: the body must name no shape, got {document}"
            );
            assert_eq!(
                document.get("message").and_then(Value::as_str),
                Some(*sentence),
                "case {case:?}: the sentence must survive, got {document}"
            );
        }
    }

    /// RULE: every other answer is handed back exactly as its renderer wrote it — a success,
    /// and a refusal of another class, whose shape a client still resolves. The rows include
    /// the status each of this API's untyped refusals travels at, under the OTHER refusal's
    /// wrapper, so one operation's untyped status cannot reach another's answers.
    #[test]
    fn an_answer_this_rule_does_not_own_is_returned_unchanged() {
        // (case, the wrapper's untyped status, the status answered, the body, the header)
        let rows: &[(&str, u16, u16, &str, Option<&str>)] = &[
            (
                "a bot a read reports",
                GONE_BOT_STATUS,
                200,
                r#"{"botId":"1","username":"a-bot"}"#,
                None,
            ),
            (
                "a refused member",
                GONE_BOT_STATUS,
                422,
                r#"{"__type":"ValidationError","reasons":[{"field":"username"}]}"#,
                Some("ValidationError"),
            ),
            (
                "a network read of a deleted network",
                GONE_BOT_STATUS,
                403,
                r#"{"__type":"ForbiddenError","message":"Network not found"}"#,
                Some("ForbiddenError"),
            ),
            (
                "an operation nobody has written yet",
                GONE_BOT_STATUS,
                501,
                r#"{"__type":"NotImplemented","message":"scaffolded"}"#,
                Some("NotImplemented"),
            ),
            (
                "a typed refusal at the security-group status under the gone-bot wrapper",
                GONE_BOT_STATUS,
                REFUSED_SECURITY_GROUP_READ_STATUS,
                r#"{"__type":"UnauthorizedError","message":"invalid security group request"}"#,
                Some("UnauthorizedError"),
            ),
            (
                "a denied network id under the gone-bot wrapper",
                GONE_BOT_STATUS,
                403,
                r#"{"__type":"ForbiddenError","message":"Access Denied"}"#,
                Some("ForbiddenError"),
            ),
            (
                "a not-found under the security-group wrapper",
                REFUSED_SECURITY_GROUP_READ_STATUS,
                GONE_BOT_STATUS,
                r#"{"__type":"ResourceNotFoundError","message":"Bot not found"}"#,
                Some("ResourceNotFoundError"),
            ),
            (
                "a security group a read reports",
                REFUSED_SECURITY_GROUP_READ_STATUS,
                200,
                r#"{"securityGroup":{"id":"0"}}"#,
                None,
            ),
        ];

        for (case, untyped_status, status, body, error_type) in rows {
            let answered = untyped(response(*status, body, *error_type), *untyped_status);
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                *error_type,
                "case {case:?}: the header stays as rendered, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_bytes(),
                "case {case:?}: the body must be byte-identical"
            );
        }
    }

    /// RULE: a body at the untyped status that names no shape at all, or that is not a JSON
    /// object, is left to its renderer byte for byte — and still loses the header, which is
    /// read on its own.
    #[test]
    fn a_body_that_cannot_be_rewritten_still_loses_the_header() {
        // (case, the status, the body)
        let rows: &[(&str, u16, &str)] = &[
            (
                "a body naming no shape",
                GONE_BOT_STATUS,
                r#"{"message":"Bot not found"}"#,
            ),
            ("an empty body", GONE_BOT_STATUS, ""),
            ("a body that is not JSON", GONE_BOT_STATUS, "<Error/>"),
            (
                "a body that is not an object",
                GONE_BOT_STATUS,
                r#"["ResourceNotFoundError"]"#,
            ),
            (
                "a refused security-group read naming no shape",
                REFUSED_SECURITY_GROUP_READ_STATUS,
                r#"{"message":"invalid security group request"}"#,
            ),
        ];

        for (case, status, body) in rows {
            let answered = untyped(
                response(*status, body, Some("ResourceNotFoundError")),
                *status,
            );
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                None,
                "case {case:?}: the header must name no shape, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_bytes(),
                "case {case:?}: a body with nothing to remove must be byte-identical"
            );
        }
    }

    /// RULE: a renderer that announced a body length has it corrected to the bytes the
    /// client receives, so the framing cannot disagree with the body.
    #[test]
    fn an_announced_body_length_follows_the_rewritten_body() {
        for (case, status, body) in [
            (
                "the gone-bot refusal",
                GONE_BOT_STATUS,
                r#"{"__type":"ResourceNotFoundError","message":"Bot not found"}"#,
            ),
            (
                "the refused security-group read",
                REFUSED_SECURITY_GROUP_READ_STATUS,
                r#"{"__type":"UnauthorizedError","message":"invalid security group request"}"#,
            ),
        ] {
            let mut given = response(status, body, Some("ResourceNotFoundError"));
            given
                .headers_mut()
                .insert(CONTENT_LENGTH, body.len().to_string());
            let answered = untyped(given, status);
            let served = answered.body().bytes().expect("an in-memory body").len();
            assert_eq!(
                answered.headers().get(CONTENT_LENGTH),
                Some(served.to_string().as_str()),
                "case {case:?}: expected the announced length {served}, got {:?}",
                answered.headers().get(CONTENT_LENGTH)
            );
        }
    }
}
