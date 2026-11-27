// SPDX-License-Identifier: CC0-1.0

//! WHICH wire protocol one request of this service is served in.
//!
//! This service's model declares two protocols side by side — `awsJson1_1` and `rpcv2Cbor`
//! (`smithy_query query="service_info"`) — so a client may call it in either, and each
//! request is read and answered in the one it arrived in. Which one that is follows from the
//! request's own MARKER, which the framework reads off its headers and path
//! (`smithy_server_runtime::wire_protocol`): `smithy-protocol: rpc-v2-cbor`, or a
//! `/service/{Service}/operation/{Operation}` path, names the CBOR protocol, and an
//! `x-amz-target` header or an `application/x-amz-json-*` content type names the JSON one. A
//! request that names NEITHER names no protocol this service declares, and is refused before
//! any operation is asked for it — there is no codec to read such a request with, and no
//! envelope to answer it in.
//!
//! That refusal is the right answer for a client calling the API, and the wrong one for the
//! request of this service that is not an API call at all: the POST a third party makes to
//! the trigger URL a webhook published ([`crate::service::webhook_trigger`]). It carries the
//! provider's own payload under the provider's own content type, names no AWS operation and
//! holds no AWS credential, so it names none of the protocols the model declares and never
//! will; what identifies it is the PATH this service minted for it, and what answers it is
//! the plane itself, which renders its own response rather than going through any codec of
//! the model.
//!
//! [`ServedWhateverProtocolItNames`] is what keeps the per-request selection from refusing
//! such a request: a request addressed to a plane this service PUBLISHED is served in the
//! protocol it names when it names one of this service's, and in this service's JSON
//! protocol otherwise. Which of the two that falls back to changes no answer of the plane —
//! the protocol decides the codec a modelled call is read with and the envelope its
//! framework-level failures render in, and the plane uses neither — so the fallback is the
//! protocol a client of this API otherwise calls it in, asked FOR rather than constructed
//! here, and nothing model-derived is restated.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, ErasedServiceDefinition, OperationRoutingTable, Refused, ServerHttpProtocol,
    ServiceIdentity, WireProtocol, WireProtocolKind,
};

/// The media type an `awsJson1_1` call of this service carries its body under: the marker a
/// request names that protocol by, which is how one of several declared protocols is asked
/// for by name.
const AWS_JSON_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// A request that names this service's JSON protocol and nothing else, for asking a
/// definition which of its declared protocols serves one.
///
/// The protocol is asked FOR rather than built: the generated entry holds the codec the model
/// declares and the error envelope that goes with it, and this is the only way to reach one
/// of several declared protocols by name.
fn names_the_json_protocol() -> RtRequest {
    let mut marker = RtRequest::new(SdkBody::empty());
    marker
        .headers_mut()
        .insert("content-type", AWS_JSON_CONTENT_TYPE);
    marker
}

/// The protocol family the call these request parts belong to names, read by the FRAMEWORK's
/// own rule (`smithy_server_runtime::WireProtocolKind::detect`) rather than by a second copy
/// of it here.
///
/// That rule reads a request's headers and its path and nothing else, so the parts are handed
/// to it as the request they belong to, carrying no body.
fn protocol_named_by(parts: &http::request::Parts) -> Option<WireProtocolKind> {
    let mut named = RtRequest::new(SdkBody::empty());
    let uri = parts.uri.to_string();
    if named.set_uri(uri.as_str()).is_err() {
        return None;
    }
    for (name, value) in parts.headers.iter() {
        // One entry per VALUE, as a repeated header yields: appended rather than inserted so
        // a request that carries a marker header twice keeps both values.
        if let Ok(value) = value.to_str() {
            named
                .headers_mut()
                .append(name.as_str().to_string(), value.to_string());
        }
    }
    WireProtocolKind::detect(&named)
}

/// Whether the call these request parts belong to is served in this service's `awsJson1_1`
/// protocol.
///
/// Asked where a handler holds the request's metadata rather than the request
/// (`smithy_server_runtime::OperationContext::parts`), because the WIRE FORM of a value can
/// differ per protocol: the shape discriminator some structures of this service carry is a
/// member of the awsJson wire form ([`crate::service::shape_discriminator`]), written inside
/// the operation's own answer, where the only thing naming the protocol is that metadata.
#[must_use]
pub fn is_a_json_call(parts: &http::request::Parts) -> bool {
    protocol_named_by(parts) == Some(WireProtocolKind::AwsJson)
}

/// This service's model-derived half, serving a request addressed to a plane this service
/// PUBLISHED whatever protocol that request names ([module docs](self)).
///
/// Identity, the routing table, the declared protocols and every error renderer come from the
/// wrapped definition untouched — nothing model-derived is restated here. Generic over that
/// definition so it can wrap the generated entry or another wrapper around it.
pub struct ServedWhateverProtocolItNames<D> {
    inner: D,
}

impl<D: ErasedServiceDefinition> ServedWhateverProtocolItNames<D> {
    /// Wrap a definition — the generated entry, or another wrapper around it.
    pub fn new(inner: D) -> Self {
        Self { inner }
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for ServedWhateverProtocolItNames<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.inner.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.inner.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.inner.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.inner.routing_error_response(req)
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        self.inner.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        self.inner.refusal_response(error, chosen, req)
    }

    fn wire_protocol(&self, req: &RtRequest) -> WireProtocol<'_> {
        let named = self.inner.wire_protocol(req);
        // A request that names one of the declared protocols is served in it, whatever it is
        // addressed to: only a request naming NONE of them is reconsidered here.
        if !named.is_unrecognised() || !crate::service::webhook_trigger::claims_request(req) {
            return named;
        }
        self.inner.wire_protocol(&names_the_json_protocol())
    }
}

#[cfg(test)]
mod tests {
    use super::is_a_json_call;

    /// One row of the table below: the case, the path the request is addressed to, the
    /// headers it carries, and whether the call is served in this service's JSON protocol.
    /// Named so the row type stays simple enough to read.
    type ProtocolCase = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
        bool,
    );

    /// Which wire form a call is served in is read off the request's own markers, in the
    /// order the framework reads them: the rows span every marker either protocol of this
    /// model is named by, a request carrying both, and a request carrying none.
    #[test]
    fn the_json_wire_form_is_named_by_its_own_markers() {
        // (case, path, headers, whether the call is served in the JSON protocol)
        let rows: &[ProtocolCase] = &[
            (
                "the JSON protocol's target header",
                "/",
                &[("x-amz-target", "CodePipeline_20150709.GetPipeline")],
                true,
            ),
            (
                "the JSON protocol's media type alone",
                "/",
                &[("content-type", "application/x-amz-json-1.1")],
                true,
            ),
            (
                "the CBOR protocol's own header",
                "/service/CodePipeline_20150709/operation/GetPipeline",
                &[
                    ("smithy-protocol", "rpc-v2-cbor"),
                    ("content-type", "application/cbor"),
                ],
                false,
            ),
            (
                "the CBOR protocol's route with no header",
                "/service/CodePipeline_20150709/operation/GetPipeline",
                &[("content-type", "application/cbor")],
                false,
            ),
            (
                "both markers, which that protocol's header decides",
                "/",
                &[
                    ("smithy-protocol", "rpc-v2-cbor"),
                    ("x-amz-target", "CodePipeline_20150709.GetPipeline"),
                ],
                false,
            ),
            (
                "a provider's own media type, which names neither",
                "/_aws/codepipeline-webhooks/us-east-1/trigger",
                &[("content-type", "application/json")],
                false,
            ),
            ("no header at all", "/", &[], false),
        ];
        for (case, path, headers, expected) in rows {
            let mut request = ::http::Request::builder().uri(*path);
            for (name, value) in *headers {
                request = request.header(*name, *value);
            }
            let parts = request
                .body(())
                .expect("a valid request")
                .into_parts()
                .0;
            let actual = is_a_json_call(&parts);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected a JSON call to be {expected}, got {actual}"
            );
        }
    }
}
