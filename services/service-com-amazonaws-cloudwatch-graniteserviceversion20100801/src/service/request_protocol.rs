// SPDX-License-Identifier: CC0-1.0

//! The protocol ONE request of this service is served in, and the request id its answer
//! names it by.
//!
//! This service's model declares three wire protocols side by side — `awsJson1_0`, `awsQuery`
//! and `rpcv2Cbor`
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json")`)
//! — and a client may call it in any of them, so each request is read AND answered in the one
//! it arrived in. Which one that is follows from the request's CONTENT TYPE, because the
//! content type is what states how the body is encoded and no body can be read as two codecs
//! at once: a `application/x-www-form-urlencoded` body is a query-protocol call, whose
//! operation is named by the `Action` field of the form and whose answer is the XML envelope
//! of that protocol, whatever OTHER dispatch header the client also sent.
//!
//! The framework's own per-request selection prefers an RPC target header over the content
//! type (`smithy_server_runtime::wire_protocol`), so a form-encoded request that also carries
//! `x-amz-target` — as a client migrating between the two protocols sends — would be read
//! with the JSON codec and refused for a body it cannot parse. [`TheProtocolOfTheRequest`]
//! restores the rule above: a form-encoded body is served in this service's declared
//! `awsQuery` protocol, selected from the generated entry rather than built here, so the
//! codec, its XML namespace and its error envelope are still the model's.
//!
//! The second half is the request id: AWS answers a failed `awsQuery` call with
//! `<ErrorResponse><Error>…</Error><RequestId>…</RequestId></ErrorResponse>`, the failure's
//! counterpart of the `ResponseMetadata/RequestId` a success carries. The element belongs to
//! the envelope rather than to any modelled error shape, so it is written by the shared
//! writer every `awsQuery` service in this workspace reports it with
//! (`service_common::query_error_envelope`), wrapped around this definition for the failures
//! no operation produced and around each operation ([`ReportingTheRequestId`]) for the ones a
//! handler or the constraint walk did.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity, WireProtocol,
};

/// The content type a query-protocol call carries its form body under: the marker that names
/// the protocol a request is served in.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// Whether `req` carries a form-encoded body, i.e. whether it is a query-protocol call.
///
/// The media type alone decides; a `charset` or any other parameter after it is part of the
/// header, not of the type, and the comparison is case-insensitive because a media type is.
fn is_form_encoded(req: &RtRequest) -> bool {
    req.headers()
        .get("content-type")
        .is_some_and(|value| {
            value
                .trim()
                .to_ascii_lowercase()
                .starts_with(FORM_CONTENT_TYPE)
        })
}

/// A request that names the query protocol and nothing else, for asking a definition which of
/// its declared protocols serves one.
///
/// The protocol is asked FOR rather than constructed: the entry holds the codec the model
/// declares, with its XML namespace and its error envelope, and this is the only way to reach
/// one of several declared protocols by name.
fn names_the_query_protocol() -> RtRequest {
    let mut marker = RtRequest::new(SdkBody::empty());
    marker.headers_mut().insert("content-type", FORM_CONTENT_TYPE);
    marker
}

/// This service's model-derived half, serving each request in the protocol the request itself
/// names ([module docs](self)).
///
/// Identity, the routing table and the declared protocols come from the wrapped definition
/// untouched — nothing model-derived is restated here. Generic over that definition so the
/// request-id envelope writer can wrap this one, or this one that writer, in either order.
pub struct TheProtocolOfTheRequest<D> {
    inner: D,
}

impl<D: ErasedServiceDefinition> TheProtocolOfTheRequest<D> {
    /// Wrap a definition — the generated entry, or another wrapper around it.
    pub fn new(inner: D) -> Self {
        Self { inner }
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for TheProtocolOfTheRequest<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.inner.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.inner.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.inner.routing_table()
    }

    /// Every framework-level failure renders in the envelope of the protocol the request is
    /// served in — the three renderers below read [`Self::wire_protocol`] for that reason, so
    /// a routing failure, a transport failure and a refusal cannot answer a query call in the
    /// JSON envelope.
    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.wire_protocol(req).routing_error_response(req)
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        self.wire_protocol(req).framework_error_response(error)
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        self.wire_protocol(req).refusal_response(error, chosen)
    }

    fn wire_protocol(&self, req: &RtRequest) -> WireProtocol<'_> {
        if is_form_encoded(req) {
            let query = self.inner.wire_protocol(&names_the_query_protocol());
            if !query.is_unrecognised() {
                return query;
            }
        }
        self.inner.wire_protocol(req)
    }
}

/// One operation, served so that whatever failure it answers names the request it answered —
/// the `<RequestId>` of the `awsQuery` error envelope and the header AWS reports the same id
/// in (`service_common::query_error_envelope::with_request_id`).
///
/// It re-implements nothing: the wrapped operation is the one this service would have served
/// anyway, and only the rendered answer is completed on the way out. Wrapped around each
/// operation because a MODELLED error and a constraint refusal are both rendered inside the
/// framework's per-request sequence, which no definition-level renderer sees; the failures no
/// operation produced are completed by the same writer wrapped around the definition.
#[derive(Debug)]
pub struct ReportingTheRequestId<O> {
    operation: O,
}

impl<O> ReportingTheRequestId<O> {
    /// Wrap the operation, already constructed from the shared engine.
    pub fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for ReportingTheRequestId<O>
where
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let answered = self.operation.run(svc, req, ctx).await;
            service_common::query_error_envelope::with_request_id(answered)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::{is_form_encoded, FORM_CONTENT_TYPE};
    use aws_smithy_runtime_api::http::Request as RtRequest;
    use aws_smithy_types::body::SdkBody;

    /// A request carrying `content_type`, and whatever other headers the case names.
    fn asked(headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        for (name, value) in headers {
            req.headers_mut().insert(name.to_string(), value.to_string());
        }
        req
    }

    /// One row of the table below: the case, the headers the request carries, and whether it
    /// is a query-protocol call. Named so the row type stays simple enough to read.
    type ContentTypeCase = (
        &'static str,
        &'static [(&'static str, &'static str)],
        bool,
    );

    /// A form-encoded body is a query call however the header spells the media type and
    /// whatever other dispatch header the client sent beside it; nothing else is one.
    #[test]
    fn the_content_type_is_what_names_a_query_call() {
        // (case, headers, whether the request is a query-protocol call)
        let rows: [ContentTypeCase; 6] = [
            ("the bare media type", &[("content-type", FORM_CONTENT_TYPE)], true),
            (
                "a charset parameter after it",
                &[(
                    "content-type",
                    "application/x-www-form-urlencoded; charset=utf-8",
                )],
                true,
            ),
            (
                "the media type in upper case",
                &[("content-type", "Application/X-WWW-Form-Urlencoded")],
                true,
            ),
            (
                "a form body beside an RPC target header",
                &[
                    ("content-type", FORM_CONTENT_TYPE),
                    ("x-amz-target", "GraniteServiceVersion20100801.GetMetricStream"),
                ],
                true,
            ),
            (
                "a JSON call",
                &[
                    ("content-type", "application/x-amz-json-1.0"),
                    ("x-amz-target", "GraniteServiceVersion20100801.GetMetricStream"),
                ],
                false,
            ),
            ("no content type at all", &[], false),
        ];
        for (case, headers, expected) in rows {
            let actual = is_form_encoded(&asked(headers));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected a query call to be {expected}, got {actual}"
            );
        }
    }
}
