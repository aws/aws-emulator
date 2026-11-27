// SPDX-License-Identifier: CC0-1.0

//! What this endpoint answers a request whose FRAMING CONTRADICTS ITSELF.
//!
//! A request of this endpoint names ONE of the two framings the model declares
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json")`
//! -> protocols `["awsJson1_1", "rpcv2Cbor"]`; see [`super::request_framing`]): the
//! `awsJson1_1` one, which posts every
//! operation to `/` and names the operation in its `X-Amz-Target` header, or Smithy RPC v2
//! CBOR, which names the operation in its request PATH and declares itself with
//! `smithy-protocol: rpc-v2-cbor`. The second
//! framing's specification PROHIBITS the first framing's dispatch headers — `X-Amz-Target`
//! and `X-Amzn-Target` — precisely because the operation is already named by the path: a
//! request carrying both names its operation twice, in two framings, and the two namings
//! are not required to agree.
//!
//! RULE: a request that names the RPC v2 CBOR framing and ALSO carries a header that
//! framing prohibits is not a request in either framing — it is a malformed HTTP request,
//! and the front door refuses it with HTTP 400 whose body names the class
//! [`MALFORMED_HTTP_REQUEST`] and nothing else. The refusal is decided from the request's
//! headers and path alone, so it is settled before any byte of the body is read, before any
//! operation is named and before any handler of this service runs — which is why no
//! operation, member or value appears in the answer and why nothing of this service's state
//! is read or written for such a request.
//!
//! The class belongs to the front door rather than to this service's modelled error
//! vocabulary (that same call's common errors `InternalServiceException`,
//! `InvalidParameterException`, `ThrottledException`), so no model declares it and it is
//! stated here rather than serialized through an error schema. It carries no message: the
//! front door read no operation and no member, so it has nothing to word.
//!
//! The refusal is served through the one seam a service has for naming the operation that
//! answers a request ([`super::EmulatedService::override_operation`]), so a contradictory
//! request is answered ahead of every modelled operation of this endpoint — the rule is
//! about the FRAMING and holds for all nine of them, not for the one a given request
//! happens to name.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The headers the `awsJson` framing of this endpoint dispatches on, and which the RPC v2
/// CBOR framing therefore prohibits. Both spellings are refused: a client that sets either
/// is naming its operation in the header as well as in the path.
const PROHIBITED_TARGET_HEADERS: [&str; 2] = ["x-amz-target", "x-amzn-target"];

/// The class the front door reports a malformed HTTP request as. The whole body is this one
/// name, so the class is the answer's only content.
pub const MALFORMED_HTTP_REQUEST: &str = "MalformedHttpRequestException";

/// The status it is reported with: the fault is in the request's own framing, so it is a
/// client error, and it is terminal — the same request can never succeed, so a client must
/// not retry it.
pub const MALFORMED_HTTP_REQUEST_STATUS: u16 = 400;

/// The media type that one-member document is written in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// Whether `req` names the RPC v2 CBOR framing and also carries a header that framing
/// prohibits — the one question this module answers a request by.
///
/// The framing is recognised by [`super::request_framing::names_rpc_v2_cbor`], the one rule
/// the whole tree recognises a framing by and the one the generated entry selects this
/// request's protocol with, so this cannot refuse a request the rest of the endpoint would
/// read as `awsJson`, or miss one it would read as CBOR.
pub fn frames_itself_contradictorily(req: &RtRequest) -> bool {
    super::request_framing::names_rpc_v2_cbor(req) && carries_a_prohibited_target_header(req)
}

/// Whether `req` carries either of the target headers the CBOR framing prohibits.
fn carries_a_prohibited_target_header(req: &RtRequest) -> bool {
    PROHIBITED_TARGET_HEADERS
        .iter()
        .any(|name| req.headers().get(*name).is_some())
}

/// The whole body of that refusal: the class as the document's one member, with no content
/// under it. Built through the JSON writer rather than written out as a literal, so the one
/// name above is the only place its text lives.
#[must_use]
pub fn malformed_http_request_body() -> String {
    ::serde_json::Value::Object(
        [(
            MALFORMED_HTTP_REQUEST.to_string(),
            ::serde_json::Value::String(String::new()),
        )]
        .into_iter()
        .collect(),
    )
    .to_string()
}

/// The front door's answer to a request whose framing contradicts itself.
#[must_use]
pub fn malformed_http_request_response() -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(MALFORMED_HTTP_REQUEST_STATUS)
            .expect("the status is a constant of this module"),
        SdkBody::from(malformed_http_request_body()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
}

/// That answer, as the one [`ErasedOperation`] this service hands back for a request whose
/// framing contradicts itself.
///
/// Stateless, and reads nothing of the request or of this service's store: the refusal is
/// settled by the request's framing markers alone, so no operation runs and nothing is
/// touched.
#[derive(Debug, Clone, Copy)]
pub struct MalformedHttpRequest;

impl ErasedOperation for MalformedHttpRequest {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { malformed_http_request_response() })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The MODELLED operation the routing key names decides which schemes sign a
        // request, and the router resolves that from the routing table rather than from
        // the operation a service handed back, so nothing here weakens them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{frames_itself_contradictorily, malformed_http_request_body};
    use aws_smithy_runtime_api::http::Request as RtRequest;
    use aws_smithy_types::body::SdkBody;

    /// A request to `uri` carrying `headers`.
    fn asked(uri: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri(uri.to_string()).expect("a valid uri");
        for (name, value) in headers {
            req.headers_mut().insert(name.to_string(), value.to_string());
        }
        req
    }

    /// One row of the table below: the case, the request's uri, the headers it carries,
    /// and whether its framing contradicts itself.
    type FramingCase = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
        bool,
    );

    /// RULE: a request naming the CBOR framing that also carries a header that framing
    /// prohibits contradicts itself; a request naming the CBOR framing without one, and a
    /// request in the `awsJson` framing (whose dispatch header that is), do not.
    #[test]
    fn a_framing_contradicts_itself_when_it_also_names_the_other_ones_dispatch_header() {
        const CBOR_PATH: &str = "/service/ResourceGroupsTaggingAPI_20170126/operation/GetResources";
        const TARGET: &str = "ResourceGroupsTaggingAPI_20170126.GetResources";
        // (case, request uri, headers, whether the framing contradicts itself)
        let rows: &[FramingCase] = &[
            (
                "the CBOR framing's header and path, plus the awsJson dispatch header",
                CBOR_PATH,
                &[
                    ("smithy-protocol", "rpc-v2-cbor"),
                    ("content-type", "application/cbor"),
                    ("x-amz-target", TARGET),
                ],
                true,
            ),
            (
                "the other spelling of that header",
                CBOR_PATH,
                &[("smithy-protocol", "rpc-v2-cbor"), ("x-amzn-target", TARGET)],
                true,
            ),
            (
                "that header spelled in upper case",
                CBOR_PATH,
                &[("smithy-protocol", "rpc-v2-cbor"), ("X-Amz-Target", TARGET)],
                true,
            ),
            (
                "the CBOR framing's path alone, plus that header",
                CBOR_PATH,
                &[("x-amz-target", TARGET)],
                true,
            ),
            (
                "the CBOR framing's header alone, plus that header",
                "/",
                &[("smithy-protocol", "rpc-v2-cbor"), ("x-amz-target", TARGET)],
                true,
            ),
            (
                "an empty target value, which still names the other framing",
                CBOR_PATH,
                &[("smithy-protocol", "rpc-v2-cbor"), ("x-amz-target", "")],
                true,
            ),
            (
                "the CBOR framing with no target header",
                CBOR_PATH,
                &[
                    ("smithy-protocol", "rpc-v2-cbor"),
                    ("content-type", "application/cbor"),
                ],
                false,
            ),
            (
                "the awsJson framing, whose dispatch header that is",
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("x-amz-target", TARGET),
                ],
                false,
            ),
            (
                "a protocol header naming neither framing, plus that header",
                "/",
                &[("smithy-protocol", "rpc-v2-json"), ("x-amz-target", TARGET)],
                false,
            ),
            ("no header at all", "/", &[], false),
        ];
        for (case, uri, headers, expected) in rows.iter().copied() {
            let actual = frames_itself_contradictorily(&asked(uri, headers));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the framing to contradict itself: {expected}, got \
                 {actual}"
            );
        }
    }

    /// The class is the body's one member, and it carries no content.
    #[test]
    fn the_refusal_body_is_the_class_and_nothing_else() {
        let body = malformed_http_request_body();
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("the refusal body is JSON");
        assert_eq!(
            parsed,
            serde_json::json!({ "MalformedHttpRequestException": "" }),
            "case the refusal body: expected the class as the one member with no content, \
             got {body}"
        );
    }
}
