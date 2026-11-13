// SPDX-License-Identifier: CC0-1.0

//! The header a refusal of this service reports its correlation id in.
//!
//! Every error shape Directory Service models carries a `RequestId` member, and
//! `crate::state::refusal` is what fills it — so a refusal this service words already
//! STATES the id of the call it answers, in the body and at the end of `Message`. AWS
//! reports that same id in the response's own `x-amzn-requestid` header, which is where an
//! SDK's response metadata reads a failed call's request id from, so a response that
//! states an id in its document must carry it in that header too: a client correlating on
//! the header and one reading the document have to see ONE value.
//!
//! The framework's awsJson error renderer writes the document and no such header
//! (`vendor/` is read-only), so this module adds it, from the id the document already
//! states — it mints nothing and rewrites no byte of the body. A response that states no
//! id (a success, whose awsJson envelope has no request-id member) is returned untouched.
//!
//! It reaches every answer the routing table's operation produces — a handler-returned
//! modelled error, the scaffold's 501 and a refusal raised while that operation was being
//! served — through [`RequestIdHeaderOperation`], the operation wrapper
//! `Service::override_operation` hands back.
//!
//! That wrapper is also where a refusal this service states from the REQUEST is answered,
//! for the two classes the centralized walk cannot answer on its own: a constraint a list's
//! ELEMENTS break, stated at the list member quoting the list the request sent
//! ([`crate::service::list_element_refusals`]), and a `@pattern` whose look-around that
//! walk's regex engine cannot compile, which it therefore never applied
//! ([`crate::service::model_pattern_refusals`]). A request that breaks neither is handed to
//! the routing table's operation untouched.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The header AWS reports a response's own request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The member every error shape of this service states that id in; see
/// `crate::state::refusal`.
const REQUEST_ID_MEMBER: &str = "RequestId";

/// The request id the response's document states, or `None` when it states none.
///
/// An error envelope is built in memory by its renderer, so the bytes are readable here; a
/// streamed body is not one and yields `None`.
fn stated_request_id(resp: &RtResponse) -> Option<String> {
    let bytes = resp.body().bytes()?;
    let document: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    document
        .get(REQUEST_ID_MEMBER)?
        .as_str()
        .map(str::to_string)
}

/// The response with its stated request id in [`REQUEST_ID_HEADER`].
///
/// Status, body and every other header are left exactly as the renderer wrote them, and a
/// response that already carries the header, or states no id, is returned untouched — so
/// this is idempotent and cannot affect the success path.
pub fn with_request_id_header(mut resp: RtResponse) -> RtResponse {
    if resp.headers().get(REQUEST_ID_HEADER).is_some() {
        return resp;
    }
    let Some(request_id) = stated_request_id(&resp) else {
        return resp;
    };
    // Dropped rather than fatal if the HTTP layer cannot carry it: the document already
    // states the id, and a header is not worth failing a modelled answer for.
    let _ = resp.headers_mut().try_insert(REQUEST_ID_HEADER, request_id);
    resp
}

/// The modelled operation this service's routing table names for this request, or `None`
/// when the request names none.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then reports the answer's stated request id in the header AWS carries it in.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, so the framework still owns the whole
/// deserialize → validate → handle → serialize sequence — and every status, code and
/// message it answers with — while this wrapper owns one header.
pub struct RequestIdHeaderOperation;

impl ErasedOperation for RequestIdHeaderOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The refusals this service states from the request itself, for the two classes
            // the centralized walk cannot answer on its own: a constraint a list's ELEMENTS
            // break, stated at the list member quoting the list the request sent, and a
            // `@pattern` that walk could not compile and so never applied. `None` from both
            // leaves the whole sequence below exactly as it was. The definition is read,
            // used and dropped inside each statement: `&dyn ErasedServiceDefinition` is not
            // `Sync`, so holding it across the await below would make this future
            // non-`Send`.
            let refused = crate::service::list_element_refusals::stated(
                svc.definition().protocol(),
                req,
            )
            .or_else(|| {
                crate::service::model_pattern_refusals::stated(svc.definition().protocol(), req)
            });
            if let Some((violations, stated)) = refused {
                return with_request_id_header(svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                ));
            }
            match modeled_operation(svc, req) {
                Some(op) => with_request_id_header(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — the
        // operation this wrapper delegates to, so its model-declared schemes decide and
        // this wrapper cannot weaken them.
        None
    }
}
