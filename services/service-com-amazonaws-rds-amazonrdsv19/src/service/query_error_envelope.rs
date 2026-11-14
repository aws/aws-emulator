// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope this front door answers with: `<ErrorResponse>` carrying a
//! `<RequestId>` sibling of `<Error>`.
//!
//! AWS answers an awsQuery error on this endpoint with
//!
//! ```xml
//! <ErrorResponse xmlns="http://rds.amazonaws.com/doc/2014-10-31/">
//!   <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
//!   <RequestId>…</RequestId>
//! </ErrorResponse>
//! ```
//!
//! The `<RequestId>` element belongs to the ENVELOPE, not to any modelled error shape: it
//! is what an SDK's error object exposes as the request id, so a failed call must carry
//! one exactly as a successful one carries `ResponseMetadata/RequestId` (which the
//! framework's success renderer already writes). The framework's error renderers write
//! only `<Error>`, so this module adds the missing element to EVERY error response this
//! service emits, whichever layer produced it:
//!
//! * a handler-returned or modelled error, and a framework refusal raised while a matched
//!   operation was being served — an absent required member, a value outside a declared
//!   bound — through [`RequestIdEnvelopeOperation`], the operation wrapper this service
//!   hands back from `Service::override_operation`;
//! * a routing failure (an `Action` naming no modelled operation), a transport failure and
//!   a pre-dispatch authentication refusal, through [`RdsDefinition`], the definition this
//!   service hands back from `Service::definition`;
//! * the API-version refusal this service raises itself
//!   (`crate::service::api_version`), which calls [`with_request_id`] directly.
//!
//! Only the envelope is this module's business: the HTTP status, the `<Type>`, the
//! `<Code>`, the `<Message>` and the whole success path stay with the framework renderer
//! that owns them, so each has exactly one home.
//!
//! The same element is added the same way by two sibling services whose models also speak
//! awsQuery (`services/service-com-amazonaws-cloudformation-cloudformation` and
//! `services/service-com-amazonaws-sns-amazonsimplenotificationservice`); those copies
//! predate this one and are to be migrated onto one shared implementation under
//! `services/common/` by their own issues, not reinvented again.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity,
};

/// The closing tag of the awsQuery error envelope.
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The envelope element this module adds.
const REQUEST_ID_OPEN: &str = "<RequestId>";

/// Add `<RequestId>` to an awsQuery `<ErrorResponse>` document, as the last child of the
/// envelope (the sibling after `<Error>`).
///
/// `None` means the document needs nothing: it is not an `<ErrorResponse>` envelope (a
/// successful `<…Response>` carries its request id in `ResponseMetadata` already), or it
/// already carries one — so this is idempotent, and a future framework renderer that
/// starts emitting the element itself cannot end up with two.
fn add_request_id(body: &str, request_id: &str) -> Option<String> {
    if body.contains(REQUEST_ID_OPEN) {
        return None;
    }
    let close = body.rfind(ERROR_RESPONSE_CLOSE)?;
    Some(format!(
        "{}<RequestId>{request_id}</RequestId>{}",
        &body[..close],
        &body[close..]
    ))
}

/// The response with a freshly minted `<RequestId>` in its `<ErrorResponse>` envelope.
///
/// Status, headers and every other byte of the body are left exactly as the renderer
/// wrote them; a response that is not an awsQuery error envelope (or already carries a
/// request id) is returned untouched.
pub fn with_request_id(mut resp: RtResponse) -> RtResponse {
    let rewritten = {
        // An error envelope is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error envelope and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return resp;
        };
        match add_request_id(text, &crate::state::new_request_id()) {
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

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then adds the envelope's `<RequestId>` to the response.
///
/// The operation it delegates to is the one THIS service names for the request: its
/// audited table first (every modelled operation, served behind the recorder that makes
/// each served call a management event — see [`crate::service::management_events`]), and
/// the generated routing table resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`] for anything that table does not hold. Either way the
/// framework still owns the whole deserialize → validate → handle → serialize sequence
/// (and every status, code and message it answers with), and this wrapper owns one
/// element of the envelope.
///
/// It is also where the ONE refusal that cannot wait for that sequence is answered: the
/// not-found fault of a resource a request REFERENCES, for a request the framework would
/// refuse for an absent member before any handler ran (see
/// [`crate::service::unresolved_reference`]). Every other request is served exactly as
/// before.
///
/// And it is where the events a served call RECORDED are published onto the account's
/// default event bus ([`crate::state::bus_events`]): every request of this front door passes
/// through here, so the publication is made once, from outside the handler that recorded the
/// change and after the response has been rendered — never from a handler holding this
/// service's store.
pub struct RequestIdEnvelopeOperation<Eng: Send + Sync + 'static> {
    /// Every modelled operation, each served behind the audit recorder, keyed by the same
    /// routing key the generated table uses.
    audited: OperationRoutingTable,
    /// The shared engine, for the one pre-dispatch refusal above — which reads this
    /// service's store and writes nothing to it — and for the publication of what the
    /// served call recorded.
    engine: std::sync::Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> RequestIdEnvelopeOperation<Eng> {
    /// Serve requests through `audited`, falling back to the generated routing table for
    /// a key it does not hold.
    pub fn new(engine: std::sync::Arc<Eng>, audited: OperationRoutingTable) -> Self {
        Self { audited, engine }
    }

    /// The operation that serves this request, or `None` when nothing names one.
    fn operation_for<'s>(
        &'s self,
        svc: &'s dyn Service,
        req: &RtRequest,
    ) -> Option<&'s dyn ErasedOperation> {
        let key = svc.definition().protocol().routing_key(req).ok()?;
        self.audited
            .get(key)
            .or_else(|| svc.definition().routing_table().get(key))
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for RequestIdEnvelopeOperation<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(resp) = crate::service::unresolved_reference::refuse_absent_db_cluster(
                &*self.engine,
                svc,
                req,
                ctx,
            ) {
                return with_request_id(resp);
            }
            match self.operation_for(svc, req) {
                Some(op) => {
                    let resp = with_request_id(op.run(svc, req, ctx).await);
                    // Every lifecycle change the call just recorded is also an event on the
                    // account's default bus, which Amazon RDS publishes with no opt-in. It
                    // is handed over HERE, after the response the caller gets has been
                    // rendered and this service's store released, rather than from a
                    // handler: EventBridge's own handler matches the event against the
                    // account's rules and delivers it to each of their targets, and a target
                    // may call back into this service (see `crate::state::bus_events`).
                    crate::state::publish_recorded_events(&*self.engine, ctx.callback()).await;
                    resp
                }
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

/// This service's model-derived half, with the framework-level error renderers wrapped so
/// a routing failure, a transport failure and a pre-dispatch refusal answer in the same
/// envelope every other error does.
///
/// Identity, the routing table and the protocol come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated here.
pub struct RdsDefinition<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> RdsDefinition<Eng> {
    /// Build the definition from the shared engine, which the generated entry threads
    /// into every operation.
    pub fn new(engine: std::sync::Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which this service claims it for [`RequestIdEnvelopeOperation`].
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for RdsDefinition<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        with_request_id(self.entry.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(self.entry.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(ErasedServiceDefinition::refusal_response(
            &self.entry,
            error,
            chosen,
            req,
        ))
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.entry.wire_protocol(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The element lands as the last child of the envelope — after `</Error>`, inside
    /// `</ErrorResponse>` — whether or not the root carries the service namespace.
    #[test]
    fn request_id_is_the_last_child_of_the_envelope() {
        for case in [
            "<?xml version=\"1.0\"?><ErrorResponse><Error><Type>Sender</Type>\
             <Code>DBInstanceNotFound</Code><Message>no</Message></Error></ErrorResponse>",
            "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"ns\"><Error><Type>Sender</Type>\
             <Code>DBInstanceNotFound</Code><Message>no</Message></Error></ErrorResponse>",
        ] {
            let rewritten =
                add_request_id(case, "11112222-3333-4444-5555-666677778888").expect("rewritten");
            assert!(
                rewritten.ends_with(
                    "</Error><RequestId>11112222-3333-4444-5555-666677778888</RequestId>\
                     </ErrorResponse>"
                ),
                "case {case:?}: expected <RequestId> as the envelope's last child, got \
                 {rewritten:?}"
            );
        }
    }

    /// Nothing to add: a document that already carries a request id, and one that is not
    /// an error envelope at all, are both left alone.
    #[test]
    fn nothing_is_added_twice_and_nothing_else_is_touched() {
        for case in [
            "<ErrorResponse><Error/><RequestId>already</RequestId></ErrorResponse>",
            "<DescribeDBInstancesResponse><DescribeDBInstancesResult/><ResponseMetadata>\
             <RequestId>a</RequestId></ResponseMetadata></DescribeDBInstancesResponse>",
            "<DescribeDBInstancesResponse><DescribeDBInstancesResult/>\
             </DescribeDBInstancesResponse>",
        ] {
            assert!(
                add_request_id(case, "fresh").is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }
}
