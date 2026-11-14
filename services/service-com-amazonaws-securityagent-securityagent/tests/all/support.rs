// SPDX-License-Identifier: CC0-1.0

//! Not a suite of its own: the wire harness every suite in this crate shares, reached
//! as `crate::support`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name (sources: this crate's `OPERATIONS.md` "Wire protocol" section and the
//! per-operation `Route` lines). So nothing driven through here can pass against a
//! handler that answers a canned body: a value must come back out of the shared store
//! through a read before a case counts.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_securityagent_securityagent::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answer from the emulator.
pub struct Answer {
    pub status: u16,
    /// The wire error code: restJson1 carries it in the `x-amzn-errortype` header and
    /// in the body's `__type` discriminator. Empty on a success.
    pub error_code: String,
    pub body: Value,
}

/// A router carrying only this service, over ONE shared engine — the same
/// `EmulatedService::new(engine)` seam `core/router` mounts.
pub fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] over an engine the CALLER holds, for a suite that has to reach the
/// shared store itself — the state a listing reads when the operations that write it
/// are still scaffolds.
pub fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator front door's concern, not this service's; these
    // cases drive unsigned requests whose credential scope still names the service.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// `POST /<operation>` with `body` as the JSON payload, signed for `region`.
///
/// The `Authorization` credential scope names `securityagent`, the signing name
/// `resolve_route_key` reads to pick this service, and the region inside it is what
/// the service reads when it derives a region-bearing value.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://securityagent.{region}.amazonaws.com/{operation}"
        ))
        .header("host", format!("securityagent.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/securityagent/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&raw)
            )
        })
    };
    let error_code = if header_code.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    // Both renderings may carry a namespace or a URI prefix; a client matches on the
    // shape name.
    let error_code = error_code
        .rsplit(['#', '/', ':'])
        .next()
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_code,
        body,
    }
}

/// `call_in`, in the region every case that is not about the region uses.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    call_in(router, "us-east-1", operation, body).await
}

/// `call`, requiring HTTP 200 and naming the case when it is not.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation} to answer 200, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// A string member of a payload, or `None` when it is absent.
pub fn member(payload: &Value, name: &str) -> Option<String> {
    payload.get(name).and_then(Value::as_str).map(str::to_string)
}

/// How many characters a hyphenated uuid carries, and where its hyphens fall: the
/// `8-4-4-4-12` groups. Shared because more than one suite asserts on the uuid BODY an
/// identifier of this service carries behind its resource class's prefix.
pub const UUID_LEN: usize = 36;
/// See [`UUID_LEN`].
pub const UUID_HYPHENS: [usize; 4] = [8, 13, 18, 23];

/// `value` is the hyphenated lowercase uuid a minted identifier's body is: the
/// [`UUID_LEN`] characters of the `8-4-4-4-12` groups, hex and nothing else, so the body
/// can carry no text from the request that asked for it.
pub fn assert_uuid_body(value: &str, case: &str, member: &str) {
    assert_eq!(
        value.len(),
        UUID_LEN,
        "case {case:?}: expected a {UUID_LEN}-character uuid body on {member}, got \
         {value:?}"
    );
    for (index, byte) in value.bytes().enumerate() {
        if UUID_HYPHENS.contains(&index) {
            assert_eq!(
                byte, b'-',
                "case {case:?}: expected a hyphen at {index} of the {member} uuid body, \
                 got {value:?}"
            );
        } else {
            assert!(
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
                "case {case:?}: expected lowercase hex at {index} of the {member} uuid \
                 body, got {value:?}"
            );
        }
    }
}
