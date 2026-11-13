// SPDX-License-Identifier: CC0-1.0

//! The `@connections` ENDPOINT a request was addressed through, and the refusal an endpoint
//! that serves nothing earns.
//!
//! RULE: these three operations are served on the OWNING api's own invoke endpoint —
//! `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`, since
//! "the stage name is typically appended to this URI to form a complete path to a deployed
//! API stage" (the `Api$ApiEndpoint` doc of the API Gateway V2 model) — so the stage in that
//! path is part of the request, and an addressing that names no DEPLOYED stage is not the
//! caller's to invoke: it is refused with the modelled `ForbiddenException`
//! (`@httpError(403)`, "The caller is not authorized to invoke this operation.") carrying the
//! sentence API Gateway's `execute-api` endpoint words that refusal in, BEFORE the request is
//! dispatched and so before the connection store is consulted at all.
//!
//! WHY it must be refused rather than answered: `GoneException` (410) is the documented
//! signal that a peer went away, and a sender prunes its own record of that connection on
//! it. A client pointed at a stage that was never deployed — a typo, a stale or deleted
//! stage — would then be told its subscribers had vanished rather than that it may not
//! invoke the endpoint, so a misconfiguration is indistinguishable from a closed socket and
//! the per-stage boundary that exists on AWS does not exist here.
//!
//! WHO decides it: the apis, the stages and the deployment released to each stage are API
//! Gateway V2's records, which this service cannot read, so that service installs its own
//! reading into the shared cell this module asks
//! (`crate::state::connections_endpoint_authority`). An emulator it is not mounted in judges
//! no addressing, and every request is then served as the modelled binding alone decides.

use std::any::Any;

use ::service_common::execute_api_connections_endpoint::EndpointAuthority;
use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::OperationError;

/// The response header a restJson1 client resolves an error SHAPE from.
///
/// From `service_common::error_type_locator`, the one place in the tree that owns where a
/// rendered JSON failure names its shape, so this refusal and that reading cannot disagree
/// about the header's name.
use ::service_common::error_type_locator::ERROR_TYPE_HEADER;

/// The media type a restJson1 answer — success or failure — is rendered in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The member an API Gateway `execute-api` refusal carries its sentence under: those bodies
/// are `{"message": …}`.
const MESSAGE_MEMBER: &str = "message";

/// The sentence API Gateway's `execute-api` endpoint refuses a request on a stage it does not
/// serve with. Fixed text with no variable parts: it names neither the stage nor the api,
/// because an endpoint that serves nothing tells a caller nothing about what is behind it.
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// Whether the endpoint this request was addressed through serves NO deployed stage, so the
/// request must be refused ([`forbidden`]) instead of dispatched.
///
/// `prefix` is the invoke path in front of the modelled `/@connections/{ConnectionId}`
/// binding and `host` the endpoint host the request named, when it named one. An empty prefix
/// is the modelled path itself, which addresses no stage and so is never refused here.
pub fn addresses_nothing<Eng: Send + Sync + 'static>(
    engine: &Eng,
    host: Option<&str>,
    prefix: &str,
) -> bool {
    if prefix.is_empty() {
        return false;
    }
    let Some(cell) = crate::state::connections_endpoint_authority(engine) else {
        return false;
    };
    // By value, so the cell's own lock is released before the reading takes the store it
    // resolves against — the lock order this service and the owner of stages share.
    let Some(reading) = cell.lock().installed() else {
        return false;
    };
    reading(engine as &dyn Any, host, prefix) == EndpointAuthority::Undeployed
}

/// The refusal an endpoint that serves no deployed stage answers with: the modelled
/// `ForbiddenException`'s own status and error code, carrying the sentence API Gateway words
/// that refusal in.
///
/// The status and the code are read off the operation's own modelled error
/// ([`OperationError`]) rather than spelled out, so the model stays the one source of both.
/// The BODY is written here because the modelled shape has NO members — there is nothing to
/// serialize a sentence into — while the recorded refusal carries one, so the framework's
/// error renderer cannot produce it.
pub fn forbidden() -> RtResponse {
    let modeled = crate::ops::get_connection::GetConnectionErrorModeled::ForbiddenException(
        crate::ops::types::ForbiddenException {},
    );
    json_refusal(modeled.http_status(), modeled.shape_name(), FORBIDDEN_MESSAGE)
}

/// One refusal on the wire: `status`, the error shape in [`ERROR_TYPE_HEADER`], and a
/// `{"message": …}` body.
fn json_refusal(status: u16, error_code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ MESSAGE_MEMBER: message })
        .to_string()
        .into_bytes();
    let length = body.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or_else(|_| {
            StatusCode::try_from(403u16).expect("403 is a valid status code")
        }),
        SdkBody::from(body),
    );
    let headers = response.headers_mut();
    let _ = headers.try_insert("content-type", JSON_CONTENT_TYPE);
    let _ = headers.try_insert("content-length", length.to_string());
    let _ = headers.try_insert(ERROR_TYPE_HEADER, error_code.to_string());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal's envelope comes from the MODEL: `ForbiddenException` is `@httpError(403)`
    /// and its shape name is the error code a restJson1 client resolves
    /// (`smithy_query(query="shape", shape="ForbiddenException")`).
    #[test]
    fn the_refusal_carries_the_modelled_status_and_code_with_the_recorded_sentence() {
        let response = forbidden();
        assert_eq!(
            response.status().as_u16(),
            403,
            "expected the modelled 403, got {}",
            response.status().as_u16()
        );
        let error_type = response
            .headers()
            .get(ERROR_TYPE_HEADER)
            .unwrap_or("<absent>");
        assert_eq!(
            error_type, "ForbiddenException",
            "expected the modelled error shape in {ERROR_TYPE_HEADER}, got {error_type:?}"
        );
        let body = response.body().bytes().expect("the refusal body is in memory");
        let document: ::serde_json::Value =
            ::serde_json::from_slice(body).expect("the refusal body is JSON");
        assert_eq!(
            document,
            ::serde_json::json!({ "message": "Forbidden" }),
            "expected the recorded sentence and nothing else, got {document}"
        );
    }
}
