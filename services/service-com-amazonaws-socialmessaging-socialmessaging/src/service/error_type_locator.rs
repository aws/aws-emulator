// SPDX-License-Identifier: CC0-1.0

//! How this service NAMES the error shape of a refused PARAMETER, in the two places a
//! rendered JSON failure carries that name: the `x-amzn-errortype` header a restJson1
//! client resolves the shape from, and the body's `__type` discriminator.
//!
//! The framework's renderers write the shape's BARE name in both. What this service's wire
//! carries for the parameter refusal — the `InvalidParametersException` it answers when the
//! request's own members cannot be accepted — is the QUALIFIED form: the shape name, a
//! separator, and the service stack's locator for that shape, which is the namespace that
//! declares it under the stack's locator root:
//!
//! ```text
//! HTTP/1.1 400 Bad Request
//! x-amzn-errortype: InvalidParametersException:http://internal.amazon.com/coral/com.amazonaws.socialmessaging/
//! ```
//!
//! The locator is not part of the shape's NAME — every SDK resolves the shape from the text
//! before the separator — so nothing a client resolves changes; it is what this service's
//! wire carries, and a client that validates the whole value sees it. The same qualified
//! form is already carried by the refusal the console-only door answers
//! ([`crate::scaffolded::CONSOLE_ACCESS_DENIED_ERROR_TYPE`]), under the locator of the
//! namespace that declares THAT shape. This service's resource lookups are the other side
//! of the same fact: a `ResourceNotFoundException` was recorded naming the bare shape, so
//! only the parameter refusal is qualified here.
//!
//! The namespace is read off this service's own identity rather than written out, so the
//! value follows the model: the shape id the generated identity states
//! (`com.amazonaws.socialmessaging#SocialMessaging`) carries the namespace that declares
//! this service's shapes.
//!
//! The restatement itself — both places, in one step, with every other member and byte of
//! the body left as the renderer wrote them — is
//! [`service_common::error_type_locator::with_restated_error_shape`]; this module supplies
//! only WHICH name replaces which, which is this service's own fact.

use aws_smithy_runtime_api::http::Response as RtResponse;
use service_common::error_type_locator::with_restated_error_shape;

/// The shape this service refuses a request whose own members cannot be accepted with: the
/// model's `InvalidParametersException` (HTTP 400), as the framework's renderers name it.
pub const INVALID_PARAMETERS_EXCEPTION: &str = "InvalidParametersException";

/// What separates a shape name from the locator that follows it.
const SEPARATOR: char = ':';

/// The root every shape locator of the service stack hangs under; the namespace that
/// declares the shape follows it.
const LOCATOR_ROOT: &str = "http://internal.amazon.com/coral/";

/// `shape` with the locator of the namespace that `service_shape_id` names: the separator,
/// the locator root, then that namespace.
///
/// Derived, not written out: a model moved to another namespace locates its shapes under
/// that namespace with no edit here.
fn located(shape: &str, service_shape_id: &str) -> String {
    let namespace = service_shape_id.split('#').next().unwrap_or_default().trim();
    format!("{shape}{SEPARATOR}{LOCATOR_ROOT}{namespace}/")
}

/// `resp` as this service answers it: a parameter refusal names
/// [`INVALID_PARAMETERS_EXCEPTION`] in the located form, in its header and its body alike.
///
/// Every other response is returned untouched — a success byte for byte, and a failure of
/// any other class with the shape its renderer named, which is what this service's resource
/// lookups were recorded carrying. IDEMPOTENT: a response already naming the located form
/// no longer names the bare shape, so a second pass leaves it alone.
pub fn answered(resp: RtResponse, service_shape_id: &str) -> RtResponse {
    with_restated_error_shape(
        resp,
        INVALID_PARAMETERS_EXCEPTION,
        &located(INVALID_PARAMETERS_EXCEPTION, service_shape_id),
    )
}

#[cfg(test)]
mod tests {
    //! The naming over the forms a response can arrive in. The wire behaviour — that every
    //! operation's parameter refusal carries the located form — is asserted through the
    //! router in `tests/all/business_public_key_material.rs`.

    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;
    use service_common::error_type_locator::ERROR_TYPE_HEADER;

    /// This service's Smithy shape id, as the generated identity states it
    /// (`smithy_query(query="service_info")`).
    const SHAPE_ID: &str = "com.amazonaws.socialmessaging#SocialMessaging";

    /// The located form of this service's parameter refusal.
    const LOCATED: &str =
        "InvalidParametersException:http://internal.amazon.com/coral/com.amazonaws.socialmessaging/";

    /// One rendered response: a status, the shape its renderer named, and that shape in the
    /// body the way the JSON renderers write it.
    fn response(status: u16, shape: &str) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(format!(r#"{{"__type":"{shape}","message":"no"}}"#)),
        );
        resp.headers_mut()
            .insert(ERROR_TYPE_HEADER, shape.to_string());
        resp
    }

    /// The locator names the namespace that declares the shapes, whatever that namespace
    /// is — so the value follows the model rather than being written out.
    #[test]
    fn the_locator_names_the_namespace_that_declares_the_shape() {
        for (case, shape_id, expected) in [
            ("this service", SHAPE_ID, LOCATED),
            (
                "another namespace",
                "com.amazon.coral.service#UnknownOperationException",
                "InvalidParametersException:http://internal.amazon.com/coral/com.amazon.coral.service/",
            ),
        ] {
            let actual = located(INVALID_PARAMETERS_EXCEPTION, shape_id);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the located name {expected:?}, got {actual:?}"
            );
        }
    }

    /// A parameter refusal carries the located form in BOTH places, whichever status the
    /// renderer answered it under and however the body spelled the shape.
    #[test]
    fn a_parameter_refusal_names_the_located_shape_in_the_header_and_the_body() {
        for (case, rendered) in [
            ("the bare shape", INVALID_PARAMETERS_EXCEPTION.to_string()),
            (
                "a namespaced shape",
                format!("com.amazonaws.socialmessaging#{INVALID_PARAMETERS_EXCEPTION}"),
            ),
        ] {
            let answer = answered(response(400, &rendered), SHAPE_ID);
            assert_eq!(
                answer.headers().get(ERROR_TYPE_HEADER),
                Some(LOCATED),
                "case {case:?}: expected the header {LOCATED:?}, got {:?}",
                answer.headers().get(ERROR_TYPE_HEADER)
            );
            let body: serde_json::Value =
                serde_json::from_slice(answer.body().bytes().expect("an in-memory body"))
                    .expect("the error body is JSON");
            assert_eq!(
                body.get("__type").and_then(serde_json::Value::as_str),
                Some(LOCATED),
                "case {case:?}: expected the body to name {LOCATED:?}, got {body}"
            );
            assert_eq!(
                body.get("message").and_then(serde_json::Value::as_str),
                Some("no"),
                "case {case:?}: the message must survive the restatement, got {body}"
            );
        }
    }

    /// Nothing to restate: a success, a failure of another class, and a response already
    /// naming the located form are each answered exactly as they arrived — so no other
    /// shape this service answers is renamed, and one pass is the same as two.
    #[test]
    fn every_other_response_is_answered_as_the_renderer_wrote_it() {
        for (case, status, shape) in [
            ("a success", 200, INVALID_PARAMETERS_EXCEPTION),
            ("a resource lookup miss", 404, "ResourceNotFoundException"),
            ("a constraint refusal", 400, "ValidationException"),
            ("a throttled request", 429, "ThrottledRequestException"),
            ("already located", 400, LOCATED),
        ] {
            let arrived = response(status, shape);
            let body = arrived
                .body()
                .bytes()
                .expect("an in-memory body")
                .to_vec();
            let answer = answered(arrived, SHAPE_ID);
            assert_eq!(
                answer.headers().get(ERROR_TYPE_HEADER),
                Some(shape),
                "case {case:?}: expected the header left at {shape:?}, got {:?}",
                answer.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                answer.body().bytes().expect("an in-memory body"),
                body.as_slice(),
                "case {case:?}: expected the body left as the renderer wrote it"
            );
        }
    }
}
