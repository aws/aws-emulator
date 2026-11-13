// SPDX-License-Identifier: CC0-1.0

//! The ONE in-process call this service makes to another emulated service.
//!
//! Three of the resources a scalable target names live where this crate cannot read them
//! through a state type — an Amazon Keyspaces table ([`super::keyspaces_tables`]), a Lambda
//! version or alias ([`super::lambda_functions`]) and the endpoint a `custom-resource` target
//! IS ([`super::custom_resource_endpoint`]) — so each of them is ASKED for over the callback
//! transport. What that takes is the same for all three whatever protocol the called service
//! speaks: sign the request as the identity the framework hands this service, dispatch it over
//! the in-process connector, and tell the called service's own answer from the emulator's own
//! front door refusing a call nothing served. Written once here, so a change to the signing or
//! to the reading of a refusal is made in one place.
//!
//! Nothing here opens a socket or leaves the host: the connector dispatches the request back
//! into this same emulator, and an address no mounted service serves is answered by the front
//! door ([`Answered::unserved`]).

use ::serde_json::Value;

/// How much of an answer this service copies and reads.
///
/// The EMULATOR's own bound, and the same one for every called service: each of these answers
/// is a handful of members (a table's capacity specification, a function's configuration, a
/// capacity document), and one of the endpoints is the CALLER's own, so the bytes a read of it
/// costs are fixed here rather than by whatever that endpoint answers.
const MAX_ANSWER_BYTES: usize = 64 * 1024;

/// The header an AWS protocol names a refusal's own shape in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// What one in-process call answered.
#[derive(Debug)]
pub struct Answered {
    /// The status the answer carried.
    pub status: u16,
    /// The shape a refusal named itself by, empty for an answer that named none.
    pub code: String,
    /// The body it carried, up to [`MAX_ANSWER_BYTES`].
    pub payload: Vec<u8>,
}

impl Answered {
    /// Whether the called service answered a success.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Whether this is the emulator's own FRONT DOOR refusing a call no mounted service
    /// served, rather than an answer from the service that was addressed
    /// (`::service_common::in_process_refusal`) — which is the answer an engine that does not
    /// hold the called service gives, and resolves nothing rather than refusing anything.
    pub fn unserved(&self) -> bool {
        ::service_common::in_process_refusal::is_transport_refusal(&self.payload)
    }

    /// The document the answer carries, when the body is one.
    pub fn document(&self) -> Option<Value> {
        ::serde_json::from_slice(&self.payload).ok()
    }
}

/// Sign `request` for `signing_name` in `region` and dispatch it in process, answering what
/// the called service answered — or `None` when the call could not be MADE at all (it could
/// not be signed, or this process carries nothing that could dispatch it), which is a fact
/// about this process rather than about the request.
pub async fn call(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    signing_name: &str,
    region: &str,
    mut request: ::http::Request<Vec<u8>>,
) -> Option<Answered> {
    // The emulator verifies every request signature, so an in-process call carries one too:
    // the credential the framework hands this service, signed through the tree's one
    // in-process signer. It is never reported or logged.
    ::service_common::in_process_sigv4::sign_callback_request(
        &mut request,
        signing_name,
        region,
        callback,
    )
    .await
    .ok()?;
    let (parts, body) = request.into_parts();
    let request = ::aws_smithy_runtime_api::http::Request::try_from(::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    ))
    .ok()?;
    let connector = callback.http_connector();
    let answer = {
        use ::aws_smithy_runtime_api::client::http::HttpConnector;
        HttpConnector::call(&connector, request).await.ok()?
    };
    let status = answer.status().as_u16();
    let error_type = answer
        .headers()
        .get(ERROR_TYPE_HEADER)
        .unwrap_or_default()
        .to_string();
    // Only the prefix one of these answers can occupy is copied and parsed: everything past
    // the cap is dropped, which leaves a long body an answer this reader cannot read rather
    // than an allocation the answer chose the size of.
    let payload: Vec<u8> = answer
        .body()
        .bytes()
        .unwrap_or_default()
        .iter()
        .take(MAX_ANSWER_BYTES)
        .copied()
        .collect();
    Some(Answered {
        status,
        code: refusal_code(&error_type, &payload),
        payload,
    })
}

/// The code a refusal names itself by: the shape in the error-type header, else the one the
/// body's `__type` member names — the shape's own NAME either way, since both carry the shape
/// id the called service declared the error under, which a protocol may follow with a `:` and
/// a trailer of its own.
fn refusal_code(error_type: &str, payload: &[u8]) -> String {
    let named = if error_type.is_empty() {
        ::serde_json::from_slice::<Value>(payload)
            .ok()
            .as_ref()
            .and_then(|document| document.get("__type"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        error_type.to_string()
    };
    named
        .split(':')
        .next()
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a refusal's code is the NAME of the shape it names itself by, taken from the
    /// error-type header when it carries one and from the body's `__type` otherwise.
    #[test]
    fn a_refusals_code_is_the_shape_it_names_itself_by() {
        /// (case, the error-type header, the body, the code it names)
        type Row<'a> = (&'a str, &'a str, &'a str, &'a str);
        let rows: &[Row<'_>] = &[
            (
                "an awsJson protocol, which names the shape in the body",
                "",
                r#"{"__type":"com.amazonaws.keyspaces#ResourceNotFoundException"}"#,
                "ResourceNotFoundException",
            ),
            (
                "a REST protocol, which names the shape in the header",
                "ResourceNotFoundException",
                r#"{"Message":"Function not found"}"#,
                "ResourceNotFoundException",
            ),
            (
                "a shape id followed by the trailer a protocol adds",
                "",
                r#"{"__type":"com.amazon.coral.service#AccessDenied:http://internal"}"#,
                "AccessDenied",
            ),
            (
                "the header, which wins over the body",
                "ForbiddenException",
                r#"{"__type":"UnknownService"}"#,
                "ForbiddenException",
            ),
            ("a body that names nothing", "", r#"{"message":"x"}"#, ""),
            ("no body at all", "", "", ""),
        ];
        for (case, error_type, body, expected) in rows {
            let actual = refusal_code(error_type, body.as_bytes());
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the emulator's own front-door refusal of a call nothing served is told from the
    /// called service's own answer, so an engine that does not hold that service resolves
    /// nothing rather than refusing anything.
    #[test]
    fn the_front_doors_own_refusal_is_told_from_a_services_answer() {
        /// (case, the status, the body, whether nothing served the call)
        type Row<'a> = (&'a str, u16, &'a str, bool);
        let rows: &[Row<'_>] = &[
            (
                "no mounted service claimed the call",
                404,
                r#"{"__type":"com.amazonaws.lambda#UnknownService","message":"none"}"#,
                true,
            ),
            (
                "the called service's own refusal",
                404,
                r#"{"__type":"ResourceNotFoundException","message":"absent"}"#,
                false,
            ),
            ("the called service's own answer", 200, r#"{"Item":{}}"#, false),
        ];
        for (case, status, body, expected) in rows {
            let answered = Answered {
                status: *status,
                code: refusal_code("", body.as_bytes()),
                payload: body.as_bytes().to_vec(),
            };
            assert_eq!(
                answered.unserved(),
                *expected,
                "case {case:?}: expected unserved {expected}, got {}",
                answered.unserved()
            );
        }
    }
}
