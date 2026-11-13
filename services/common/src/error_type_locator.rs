// SPDX-License-Identifier: CC0-1.0

//! The TWO places a rendered JSON failure names its error SHAPE — the body's `__type`
//! discriminator and the `x-amzn-errortype` header a REST client resolves the shape from —
//! and the ONE restatement of that shape, for a service whose wire names a different shape
//! there from the one the layer that rendered the failure wrote.
//!
//! # Why a service needs this
//!
//! The framework's pre-handler renderers pick the error shape themselves: a constraint
//! refusal is rendered under the generic validation shape for every service, and
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is where a
//! service states its own instead. That seam is handed the violations ALONE, so a service
//! whose wire answers one shape for some operations and another for the rest — because two
//! stacks front its operations, each with its own refusal envelope — cannot decide there: it
//! has to restate the shape later, where the operation that answered is known.
//!
//! Restating it means writing it in BOTH places, or a client that reads the header and one
//! that reads the body disagree about what was refused. That is the whole of this module;
//! WHICH shape replaces which is the service's fact, read off its recordings, so the caller
//! states the pair.
//!
//! # What it does not touch
//!
//! The status, the detail text and every other member stay with the renderer that wrote them
//! (the member that text travels in is [`crate::error_detail_member`]'s), and a success is
//! returned byte for byte. Only a failure whose body names the `from` shape is rewritten, so
//! an error a handler serialized from its model's own shape, and a failure of any other
//! class, keep the shape they named.
//!
//! # The FORM that header names a shape in
//!
//! The second half of this module is about the same header on a service whose wire names the
//! RIGHT shape in a form the framework's renderers do not write: the shape name, a ':', and
//! then the LOCATOR of the namespace that declares it, under the service stack's locator
//! root —
//!
//! ```text
//! x-amzn-errortype: ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.example/
//! ```
//!
//! The locator is not part of the shape's NAME — every SDK resolves the shape from the text
//! before the separator (and after a '#') — but it is what such a service's wire carries, and
//! a client that reads or validates the whole value sees it. [`shape_locator`] derives it
//! from the service's own Smithy shape id, [`with_error_type_locator`] completes a rendered
//! response with it, and [`QualifiedErrorTypeDefinition`] covers the responses no operation
//! produced. A service opts in by wrapping its generated definition and routing its
//! operations' responses through the same function; nothing here decides whether a service's
//! wire carries the form.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{
    ErasedServiceDefinition, OperationRoutingTable, Refused, ServerHttpProtocol, ServiceIdentity,
};

/// The header a REST-JSON client resolves a failure's error shape from.
pub const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The member a JSON error envelope names that same shape in.
pub const SHAPE_DISCRIMINATOR: &str = "__type";

/// The lowest status a JSON error envelope is rendered under: below it a response is a
/// success and names no error shape at all.
const FIRST_ERROR_STATUS: u16 = 400;

/// The bare error SHAPE a rendered value names, which is what both places are matched on.
///
/// A value may arrive namespaced (`com.amazonaws.example#ValidationException`); the namespace
/// is not part of the shape's name, and no SDK reads it as one.
#[must_use]
pub fn shape_named(raw: &str) -> &str {
    raw.trim().rsplit('#').next().unwrap_or(raw).trim()
}

/// `body` with the error shape it names replaced by `to`, or `None` when there is nothing to
/// restate: the body is not a JSON object, or the shape it names is not `from`.
///
/// Every other member is written back as it was, so only the discriminator moves.
#[must_use]
pub fn restated_error_shape(body: &[u8], from: &str, to: &str) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let named = object.get(SHAPE_DISCRIMINATOR).and_then(Value::as_str)?;
    if shape_named(named) != from {
        return None;
    }
    object.insert(SHAPE_DISCRIMINATOR.to_string(), Value::String(to.to_string()));
    serde_json::to_vec(&document).ok()
}

/// `resp` as the serving service answers it: a failure that named `from` names `to` instead,
/// in its body and in its [`ERROR_TYPE_HEADER`] alike ([`restated_error_shape`]).
///
/// A response that is not a failure, or whose body does not name `from`, is returned
/// untouched — status, headers and every byte of the body exactly as the renderer wrote them.
/// A header value the HTTP layer will not carry is left as it was rather than turned into a
/// transport failure, which on an error path would replace a modelled 4xx with a 500 an SDK
/// retries.
#[must_use]
pub fn with_restated_error_shape(mut resp: RtResponse, from: &str, to: &str) -> RtResponse {
    if resp.status().as_u16() < FIRST_ERROR_STATUS {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable here; a
        // streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match restated_error_shape(bytes, from, to) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if it
        // ever did, the header is REMOVED rather than left describing the body before the
        // rewrite, so the two can never disagree.
        if resp
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
        {
            resp.headers_mut().remove("content-length");
        }
    }
    let _ = resp
        .headers_mut()
        .try_insert(ERROR_TYPE_HEADER, to.to_string());
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

// =============================================================================
// The qualified FORM the header names a shape in
// =============================================================================

/// What separates the shape name from the locator that qualifies it.
const LOCATOR_SEPARATOR: char = ':';

/// The root every shape locator of the service stack hangs under; the namespace that
/// declares the shape follows it.
const LOCATOR_ROOT: &str = "http://internal.amazon.com/coral/";

/// The locator a shape name is qualified with, for the service whose Smithy shape id is
/// `service_shape_id`: the separator, the locator root, then the namespace that id carries.
///
/// Derived, not written out: a model moved to another namespace qualifies its shapes under
/// that namespace with no edit at the call site.
#[must_use]
pub fn shape_locator(service_shape_id: &str) -> String {
    let namespace = service_shape_id
        .split('#')
        .next()
        .unwrap_or_default()
        .trim();
    format!("{LOCATOR_SEPARATOR}{LOCATOR_ROOT}{namespace}/")
}

/// The bare error SHAPE a HEADER value names: the text before the separator, and after any
/// namespace — which is exactly what an SDK resolves the shape from.
///
/// A value may arrive bare, namespaced (`com.amazonaws.example#ValidationException`) or with
/// a locator already appended; only the name is the shape's.
#[must_use]
pub fn header_shape_named(raw: &str) -> &str {
    let named = shape_named(raw);
    named
        .split(LOCATOR_SEPARATOR)
        .next()
        .unwrap_or(named)
        .trim()
}

/// `resp` with the error shape its [`ERROR_TYPE_HEADER`] names qualified by `locator` (see
/// [`shape_locator`]).
///
/// A success is returned untouched, as is a failure whose renderer named no shape: there is
/// nothing to qualify, and this invents no header the renderer did not write. IDEMPOTENT: the
/// shape is read back out of whatever the header carries ([`header_shape_named`]), so a value
/// that already carries the locator is rewritten to itself and two layers applying this
/// cannot report two locators for one call. A header value the HTTP layer will not carry is
/// left as it was rather than turned into a transport failure, which on an error path would
/// replace a modelled 4xx with a 500 an SDK retries. The body keeps naming the bare shape in
/// [`SHAPE_DISCRIMINATOR`], exactly as the renderer wrote it.
#[must_use]
pub fn with_error_type_locator(mut resp: RtResponse, locator: &str) -> RtResponse {
    if resp.status().as_u16() < FIRST_ERROR_STATUS {
        return resp;
    }
    let Some(named) = resp.headers().get(ERROR_TYPE_HEADER) else {
        return resp;
    };
    let shape = header_shape_named(named);
    if shape.is_empty() {
        return resp;
    }
    let qualified = format!("{shape}{locator}");
    let _ = resp.headers_mut().try_insert(ERROR_TYPE_HEADER, qualified);
    resp
}

/// A service's generated definition with its framework-level error renderers wrapped, so a
/// response NO operation produced names its error shape in the qualified form too — a routing
/// failure, a transport failure detected up front, and a pre-dispatch refusal.
///
/// The locator comes from the wrapped definition's own identity, and identity, the routing
/// table and the protocol are read straight off it: nothing model-derived is restated here.
/// The responses operations DO produce are the service's own to route through
/// [`with_error_type_locator`], at whatever per-operation layer it already has.
pub struct QualifiedErrorTypeDefinition<D>(D);

impl<D: ErasedServiceDefinition> QualifiedErrorTypeDefinition<D> {
    /// Wrap `inner`, qualifying the error shape of every framework-level response it renders.
    pub fn new(inner: D) -> Self {
        Self(inner)
    }

    /// Whether the wrapped definition's generated routing table can serve this request — the
    /// condition a service claims a request for its own per-operation wrapper under, so a
    /// request naming no modelled operation stays the framework's routing failure.
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.0
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.0.routing_table().get(key))
            .is_some()
    }

    /// The locator this definition's identity names: the one value every layer qualifies
    /// with.
    #[must_use]
    pub fn locator(&self) -> String {
        shape_locator(self.0.identity().shape_id())
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for QualifiedErrorTypeDefinition<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.0.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.0.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.0.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        with_error_type_locator(self.0.routing_error_response(req), &self.locator())
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_error_type_locator(self.0.framework_error_response(error, req), &self.locator())
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_error_type_locator(
            self.0.refusal_response(error, chosen, req),
            &self.locator(),
        )
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.0.wire_protocol(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;

    /// The shape a framework refusal renderer names, and the one a service restates it as.
    const RENDERED: &str = "ValidationException";
    const SERVICE_OWN: &str = "BadRequestException";

    /// One response at `status`, carrying `body` and naming `error_type` in the header.
    fn response(status: u16, body: &str, error_type: &str) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        );
        resp.headers_mut()
            .insert(ERROR_TYPE_HEADER, error_type.to_string());
        resp
    }

    /// The body of `resp` as JSON.
    fn decoded(resp: &RtResponse) -> Value {
        serde_json::from_slice(resp.body().bytes().expect("an in-memory body"))
            .expect("the body is JSON")
    }

    /// The shape moves in BOTH places, whether the body named it bare or namespaced, and
    /// every other member is written back as it was.
    #[test]
    fn the_shape_is_restated_in_the_body_and_in_the_header() {
        for (case, body) in [
            (
                "a bare shape",
                format!(r#"{{"__type":"{RENDERED}","message":"no","retryable":false}}"#),
            ),
            (
                "a namespaced shape",
                format!(
                    r#"{{"__type":"com.amazonaws.example#{RENDERED}","message":"no","retryable":false}}"#
                ),
            ),
        ] {
            let answered =
                with_restated_error_shape(response(400, &body, RENDERED), RENDERED, SERVICE_OWN);
            let document = decoded(&answered);
            assert_eq!(
                document.get(SHAPE_DISCRIMINATOR).and_then(Value::as_str),
                Some(SERVICE_OWN),
                "case {case:?}: expected the body to name {SERVICE_OWN:?}, got {document}"
            );
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(SERVICE_OWN),
                "case {case:?}: expected the header to name {SERVICE_OWN:?}, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                document.get("message").and_then(Value::as_str),
                Some("no"),
                "case {case:?}: the detail text must survive, got {document}"
            );
            assert_eq!(
                document.get("retryable"),
                Some(&Value::Bool(false)),
                "case {case:?}: every other member must survive, got {document}"
            );
        }
    }

    /// Nothing to restate: a success, a failure naming another shape, a body that is not a
    /// JSON object, one that is not JSON at all, and one naming no shape — each is answered
    /// byte for byte, header included.
    #[test]
    fn a_response_naming_another_shape_is_answered_unchanged() {
        for (case, status, body, named) in [
            (
                "a success",
                200,
                format!(r#"{{"__type":"{RENDERED}"}}"#),
                RENDERED,
            ),
            (
                "another shape",
                404,
                r#"{"__type":"NotFoundException","Message":"gone"}"#.to_string(),
                "NotFoundException",
            ),
            (
                "not an object",
                400,
                r#"["ValidationException"]"#.to_string(),
                RENDERED,
            ),
            ("not JSON", 400, "<Error/>".to_string(), RENDERED),
            (
                "no shape at all",
                400,
                r#"{"message":"no"}"#.to_string(),
                RENDERED,
            ),
        ] {
            let answered =
                with_restated_error_shape(response(status, &body, named), RENDERED, SERVICE_OWN);
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_bytes(),
                "case {case:?}: expected the body unchanged"
            );
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(named),
                "case {case:?}: expected the header unchanged, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }

    /// A Smithy service shape id, as a generated identity states one.
    const SHAPE_ID: &str = "com.amazonaws.example#ExampleServiceLambda";

    /// The locator names the namespace that DECLARES the shapes, whatever that namespace is.
    #[test]
    fn the_locator_names_the_namespace_of_the_shape_id() {
        for (case, shape_id, expected) in [
            (
                "a service's own namespace",
                SHAPE_ID,
                ":http://internal.amazon.com/coral/com.amazonaws.example/",
            ),
            (
                "another namespace",
                "com.amazon.coral.service#UnknownOperationException",
                ":http://internal.amazon.com/coral/com.amazon.coral.service/",
            ),
        ] {
            let actual = shape_locator(shape_id);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the locator {expected:?}, got {actual:?}"
            );
        }
    }

    /// Whatever shape a renderer named is the shape that gets qualified, so the value is not
    /// keyed to one refusal — and a value already carrying the locator is not doubled.
    #[test]
    fn the_header_qualifies_whatever_shape_was_named_once() {
        let locator = shape_locator(SHAPE_ID);
        for (case, named) in [
            ("a bare shape", RENDERED.to_string()),
            ("another bare shape", "ThrottlingException".to_string()),
            ("a synthetic framework shape", "NotImplemented".to_string()),
            (
                "a namespaced shape",
                format!("com.amazonaws.example#{RENDERED}"),
            ),
            ("an already qualified shape", format!("{RENDERED}{locator}")),
        ] {
            let bare = header_shape_named(&named).to_string();
            let answered = with_error_type_locator(
                response(400, r#"{"__type":"X"}"#, &named),
                &locator,
            );
            let expected = format!("{bare}{locator}");
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }

    /// Nothing to qualify: a success, and a failure whose renderer named no shape or an empty
    /// one, are left exactly as they were — status included.
    #[test]
    fn a_response_naming_no_shape_keeps_its_header() {
        let locator = shape_locator(SHAPE_ID);
        for (case, status, named) in [
            ("a success", 200, RENDERED),
            ("an empty header", 404, ""),
        ] {
            let answered =
                with_error_type_locator(response(status, r#"{"__type":"X"}"#, named), &locator);
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(named),
                "case {case:?}: expected the header left as it was ({named:?}), got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                answered.status().as_u16(),
                status,
                "case {case:?}: the status must not move, got {}",
                answered.status().as_u16()
            );
        }
    }
}
