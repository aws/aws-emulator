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
//! The other half of that selection is what makes a request name a protocol AT ALL. Each of
//! the three is named by a HEADER of the request — `smithy-protocol: rpc-v2-cbor` for the CBOR
//! protocol, `x-amz-target` or the `application/x-amz-json-*` content type for the JSON one,
//! the form content type for the query one — so a request carrying none of them names no
//! protocol of this service and is routed nowhere ([`names_a_protocol`]): the answer is the
//! not-found the framework already gives a request that names none of a service's declared
//! protocols, rather than the malformed-body refusal of a protocol the request never asked
//! for. The framework's own detection ALSO reads the CBOR protocol off a
//! `/service/{Service}/operation/{Operation}` path, which that protocol's specification pairs
//! with the header; this service is reached on one endpoint by clients of all three, so the
//! header is what discriminates, and a request to that path without it is as unrouted as one
//! to any other path.
//!
//! The second half is the request id: AWS answers a failed `awsQuery` call with
//! `<ErrorResponse><Error>…</Error><RequestId>…</RequestId></ErrorResponse>`, the failure's
//! counterpart of the `ResponseMetadata/RequestId` a success carries, and it names the request of
//! EVERY answer in the `x-amzn-requestid` header — which is where an SDK reads a failed call's
//! `ResponseMetadata.RequestId` from when the call was made in either of the two binary
//! protocols, whose error bodies have no element for it. Both reports are the shared writer's
//! ([`naming_the_request`], `service_common::query_error_envelope::named_request`), under ONE id
//! per answer, wrapped around this definition for the failures no operation produced and around
//! each operation ([`ReportingTheRequestId`]) for the ones a handler or the constraint walk did.
//! For an operation the id is DECIDED as that wrapper is entered rather than as the answer is
//! rendered, because one refusal of this service states it in its own words — the backend trailer
//! CloudWatch closes a metric-math expression refusal with — and a handler can only state the id
//! it is answering under if that id already exists.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity, WireProtocol,
};

/// The content type a query-protocol call carries its form body under: the marker that names
/// the protocol a request is served in.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// Whether `content_type` names the encoding a query-protocol call carries its body in.
///
/// The media type alone decides; a `charset` or any other parameter after it is part of the
/// header, not of the type, and the comparison is case-insensitive because a media type is.
/// The ONE reading of that header in this service, so the layer that selects the protocol and
/// the layer that renders a value in it cannot disagree about which protocol a request is.
fn names_the_form_encoding(content_type: &str) -> bool {
    content_type
        .trim()
        .to_ascii_lowercase()
        .starts_with(FORM_CONTENT_TYPE)
}

/// Whether `req` carries a form-encoded body, i.e. whether it is a query-protocol call.
fn is_form_encoded(req: &RtRequest) -> bool {
    req.headers()
        .get("content-type")
        .is_some_and(names_the_form_encoding)
}

/// Whether the request these `parts` belong to is a query-protocol call — the same question
/// [`is_form_encoded`] answers, asked where a handler holds the request's metadata rather than
/// the request ([`smithy_server_runtime::OperationContext::parts`]).
///
/// Needed because the WIRE FORM of a value can differ per protocol: a member this service
/// renders differently on the XML wire than on the two binary ones is rendered inside the
/// operation's own answer, where the only thing naming the protocol is the request's metadata
/// (`super::decimal_floats`).
pub fn is_a_query_call(parts: &smithy_server_runtime::http::request::Parts) -> bool {
    parts
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(names_the_form_encoding)
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

/// The header that names the Smithy RPC v2 CBOR protocol, and the value it carries: "the
/// `smithy-protocol` header… MUST be set to `rpc-v2-cbor`" (that protocol's specification).
///
/// `pub(super)` because the ERROR ENVELOPE of that route answers a refusal of its own — a
/// request it cannot frame — and such an answer names the route it travels on exactly as every
/// other answer of it does (`super::cbor_error_envelope`).
pub(super) const SMITHY_PROTOCOL_HEADER: &str = "smithy-protocol";
pub(super) const RPC_V2_CBOR: &str = "rpc-v2-cbor";

/// Whether `req` is a call on the Smithy RPC v2 CBOR route: the one marker that protocol's
/// specification names it by, read as the ONE reading of that header in this service
/// ([`names_a_protocol`], which asks the same question of the same header).
pub(super) fn is_a_cbor_call(req: &RtRequest) -> bool {
    req.headers()
        .get(SMITHY_PROTOCOL_HEADER)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(RPC_V2_CBOR))
}

/// The two path steps the CBOR route addresses a call with: "every request is
/// `POST /service/{ServiceName}/operation/{OperationName}`" (that protocol's specification).
const CBOR_ROUTE_SERVICE_STEP: &str = "/service/";
const CBOR_ROUTE_OPERATION_STEP: &str = "/operation/";

/// Whether `req` is a CBOR-route call addressed to a Smithy service OTHER than `shape_name`.
///
/// This route names the SERVICE in the path beside the operation, so the path addresses one
/// service's surface: a request naming another service's shape names no operation of THIS
/// service, whatever operation the rest of the path spells, and is as unrouted here as a
/// request naming an operation this service does not have. The framework's own routing key is
/// the operation step alone (`aws_smithy_schema_server::rpc::routing_key_from_path`), which is
/// why the service step is read here.
///
/// `false` for a call in either of this service's other protocols, whose requests carry no such
/// path, and for a path that does not spell the two steps at all — the framework's own key
/// resolution already refuses that, and this answers only the question it does not ask.
pub(super) fn names_another_service(req: &RtRequest, shape_name: &str) -> bool {
    if !is_a_cbor_call(req) {
        return false;
    }
    let path = req.uri().split('?').next().unwrap_or_default();
    let Some(addressed) = path.strip_prefix(CBOR_ROUTE_SERVICE_STEP) else {
        return false;
    };
    addressed
        .split_once(CBOR_ROUTE_OPERATION_STEP)
        .is_some_and(|(named, _)| named != shape_name)
}

/// The header an `awsJson` client names the operation in, which is what names that protocol on
/// a request to this service, and the media-type prefix its body carries.
const RPC_TARGET_HEADER: &str = "x-amz-target";
const AWS_JSON_CONTENT_TYPE: &str = "application/x-amz-json";

/// Whether `req` names one of the protocols this service declares ([module docs](self)).
///
/// Keyed on the request's HEADERS alone, as each protocol's own marker is: nothing of the path
/// or of the body is read, so a request that names no protocol is recognised as such before
/// anything is decoded.
fn names_a_protocol(req: &RtRequest) -> bool {
    let headers = req.headers();
    if headers
        .get(SMITHY_PROTOCOL_HEADER)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(RPC_V2_CBOR))
    {
        return true;
    }
    if headers.get(RPC_TARGET_HEADER).is_some() {
        return true;
    }
    let content_type = headers
        .get("content-type")
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    content_type.starts_with(AWS_JSON_CONTENT_TYPE) || content_type.starts_with(FORM_CONTENT_TYPE)
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
        if !names_a_protocol(req) {
            return WireProtocol::unrecognised();
        }
        self.inner.wire_protocol(req)
    }
}

/// One answer, naming the request it answers wherever its protocol states that id: the
/// `<RequestId>` element of an `awsQuery` error envelope, and the `x-amzn-requestid` header of
/// every answer in any of the three protocols this service declares.
///
/// One id per answer, so a client correlating on the header and one reading the envelope see the
/// same value, and an answer a layer already named its request in keeps the id it has
/// (`service_common::query_error_envelope::named_request`). This is the report BOTH seams of this
/// service use — the definition wrapper for the failures no operation produced, and
/// [`ReportingTheRequestId`] for the ones a handler or the constraint walk did — so which layer
/// refused a request never decides whether its answer names it.
pub fn naming_the_request(answered: RtResponse) -> RtResponse {
    service_common::query_error_envelope::named_request(answered).0
}

/// The same report for an answer whose request id was decided BEFORE it was served: the
/// `<RequestId>` of the `awsQuery` error envelope and the header, both stating `request_id`
/// (`service_common::query_error_envelope::named_request_under`).
///
/// One answer of this service carries its request id in the WORDS of a refusal as well — the
/// backend trailer CloudWatch closes a metric-math expression refusal with — so the id cannot
/// be minted as that answer is rendered: it is decided before the walk begins
/// ([`ReportingTheRequestId`]) and read by the handler that states it
/// (`service_common::query_error_envelope::served_request_id`). This is what reports that same
/// id, so the message, the envelope and the header of one answer can never name different ids.
fn naming_the_request_under(answered: RtResponse, request_id: &str) -> RtResponse {
    service_common::query_error_envelope::named_request_under(answered, request_id)
}

/// One operation, served so that whatever failure it answers names the request it answered —
/// the `<RequestId>` of the `awsQuery` error envelope and the header AWS reports the same id
/// in ([`naming_the_request_under`]).
///
/// It re-implements nothing: the wrapped operation is the one this service would have served
/// anyway, and only the rendered answer is completed on the way out. Wrapped around each
/// operation because a MODELLED error and a constraint refusal are both rendered inside the
/// framework's per-request sequence, which no definition-level renderer sees; the failures no
/// operation produced are completed by the same writer wrapped around the definition.
///
/// The id itself is decided as this wrapper is entered rather than as the answer is rendered,
/// because one refusal of this service states it in its own WORDS and so has to read it while
/// the handler is building them
/// (`service_common::query_error_envelope::served_request_id`).
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
        // The id is decided HERE, before the wrapped operation runs, rather than as its answer
        // is rendered: a refusal whose own words state the request id has to be able to read
        // it while they are being built, and the answer is then reported under that very id.
        Box::pin(service_common::query_error_envelope::answered_under_one_request_id(
            self.operation.run(svc, req, ctx),
            naming_the_request_under,
        ))
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::{is_form_encoded, names_a_protocol, FORM_CONTENT_TYPE};
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

    /// A request names one of this service's protocols when, and only when, it carries that
    /// protocol's own marker header: the rows span every marker and every way of missing one.
    #[test]
    fn a_request_names_a_protocol_only_through_its_own_marker_header() {
        // (case, headers, whether the request names one of this service's protocols)
        let rows: [ContentTypeCase; 10] = [
            (
                "the CBOR protocol's own header",
                &[
                    ("smithy-protocol", "rpc-v2-cbor"),
                    ("content-type", "application/cbor"),
                ],
                true,
            ),
            (
                "that header spelled in upper case",
                &[("smithy-protocol", "RPC-V2-CBOR")],
                true,
            ),
            (
                "a protocol header naming something else",
                &[("smithy-protocol", "rpc-v2-json")],
                false,
            ),
            (
                "a CBOR body with no protocol header",
                &[("content-type", "application/cbor")],
                false,
            ),
            (
                "the JSON protocol's target header",
                &[("x-amz-target", "GraniteServiceVersion20100801.GetMetricStream")],
                true,
            ),
            (
                "the JSON protocol's media type alone",
                &[("content-type", "application/x-amz-json-1.0")],
                true,
            ),
            (
                "the query protocol's media type",
                &[("content-type", FORM_CONTENT_TYPE)],
                true,
            ),
            (
                "the query protocol's media type with a parameter",
                &[("content-type", "application/x-www-form-urlencoded; charset=utf-8")],
                true,
            ),
            ("no header at all", &[], false),
            (
                "a media type of no protocol this service declares",
                &[("content-type", "application/json")],
                false,
            ),
        ];
        for (case, headers, expected) in rows {
            let actual = names_a_protocol(&asked(headers));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the request to name a protocol: {expected}, got \
                 {actual}"
            );
        }
    }
}
