// SPDX-License-Identifier: CC0-1.0

//! Which CLASS a read of one network refuses in, driven over HTTP against one running
//! emulator.
//!
//! The rule under test: a read of one network refuses a `networkId` it cannot resolve to a
//! network of this authenticated account in the PERMISSION class — the modelled
//! `ForbiddenError` (`@httpError(403)`, from
//! `smithy_query(query="operation",operation="GetNetwork")`) — whatever the id was or became;
//! the authentication class (`UnauthorizedError`, `@httpError(401)`) states that credentials
//! are missing or invalid, which nothing in the path can make true, so no refusal of this
//! read carries it.
//!
//! The rows span what the store can answer about an id: one it never minted (over the
//! `NetworkId` digit space, both published bounds included), one it minted and deleted, and a
//! label outside `NetworkId` altogether. A network the store does hold is the control: it is
//! reported, so the rule is not "every read is refused".
//!
//! Every case goes through the real multi-service router — route resolution, restJson1
//! deserialization, modelled-constraint validation, the handler and response serialization —
//! so what it asserts is the wire behaviour a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_wickr_wickradminapi::state::{
    ACCESS_DENIED_MESSAGE, NETWORK_GONE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The status the model declares for `ForbiddenError` (`@httpError(403)`) — the permission
/// class every refusal of this read travels in.
const FORBIDDEN_STATUS: u16 = 403;

/// The status the model declares for `UnauthorizedError` (`@httpError(401)`) — the
/// authentication class, which no refusal of this read may carry.
const UNAUTHORIZED_STATUS: u16 = 401;

/// One answer as a client sees it: the status, the `x-amzn-errortype` header restJson1
/// resolves an error shape from, and the parsed body.
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The `message` member of the body, or `""` when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `__type` discriminator of the body, or `""` when there is none.
    fn wire_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// A fresh emulator holding only this service, over its own engine, so one test's networks
/// are invisible to another's.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is not the subject here: every request below is an AUTHENTICATED one, and
    // what is under test is how the read classifies the id it carries.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_wickr_wickradminapi::EmulatedService::new(
        engine,
    ));
    router
}

/// One request/response round trip.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "admin.wickr.us-east-1.amazonaws.com")
        // The SigV4 credential scope is how the router picks the service.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260102/us-east-1/wickr/aws4_request, \
             SignedHeaders=host, Signature=00",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Create a network and return its minted id, failing the test if it was refused.
async fn create_network(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/networks",
        Some(json!({ "networkName": name, "accessLevel": "STANDARD" })),
    )
    .await;
    assert!(
        (200..300).contains(&answer.status),
        "CreateNetwork {name:?}: expected a success, got {} with {}",
        answer.status,
        answer.body
    );
    answer.body["networkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateNetwork {name:?}: no networkId in {}", answer.body))
        .to_string()
}

#[tokio::test]
async fn every_network_read_refusal_is_the_permission_class() {
    let router = emulator();
    // A network this store minted and then deleted, so the id can still be placed; and one it
    // keeps, as the control that a resolvable id is reported rather than refused.
    let deleted = create_network(&router, "class-doomed-net").await;
    let kept = create_network(&router, "class-kept-net").await;
    let answer = call(&router, "DELETE", &format!("/networks/{deleted}"), None).await;
    assert!(
        (200..300).contains(&answer.status),
        "DeleteNetwork: expected a success, got {} with {}",
        answer.status,
        answer.body
    );

    // (case, the id asked for, the sentence this API states for it)
    // The unplaceable rows walk the `NetworkId` space (`@length` 8, `@pattern ^[0-9]{8}$`):
    // both published bounds, a repeated digit, one just below the minted range and one in the
    // middle. The last two rows are the other things an id can be: owned-and-deleted, and a
    // label outside the shape entirely.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "the low bound of the id space",
            "00000000".to_string(),
            ACCESS_DENIED_MESSAGE,
        ),
        (
            "the high bound of the id space",
            "99999999".to_string(),
            ACCESS_DENIED_MESSAGE,
        ),
        (
            "one digit repeated",
            "77777777".to_string(),
            ACCESS_DENIED_MESSAGE,
        ),
        (
            "just below the minted range",
            "09999999".to_string(),
            ACCESS_DENIED_MESSAGE,
        ),
        (
            "the middle of the id space",
            "45678901".to_string(),
            ACCESS_DENIED_MESSAGE,
        ),
        ("owned and deleted", deleted.clone(), NETWORK_GONE_MESSAGE),
        (
            "a label outside the modelled shape",
            "12x".to_string(),
            ACCESS_DENIED_MESSAGE,
        ),
    ];

    for (case, network_id, message) in &cases {
        assert_ne!(
            network_id, &kept,
            "case {case:?}: the refused id must not be the one this store still holds"
        );
        let answer = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_eq!(
            answer.status, FORBIDDEN_STATUS,
            "case {case:?}: expected the permission class {FORBIDDEN_STATUS}, got {} with {}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.status, UNAUTHORIZED_STATUS,
            "case {case:?}: a refusal of this read is never the authentication class, got {}",
            answer.body
        );
        assert_eq!(
            answer.wire_code(),
            "ForbiddenError",
            "case {case:?}: expected the modelled ForbiddenError in the body, got {}",
            answer.body
        );
        assert_eq!(
            answer.error_type, "ForbiddenError",
            "case {case:?}: expected x-amzn-errortype ForbiddenError, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.message(),
            *message,
            "case {case:?}: expected the fixed sentence {message:?}, got {:?}",
            answer.message()
        );
        assert!(
            !answer.body.to_string().contains(network_id.as_str()),
            "case {case:?}: the refusal must not disclose the id it was asked about, got {}",
            answer.body
        );
    }

    // The control: an id this store CAN place as a network of this account is reported.
    let answer = call(&router, "GET", &format!("/networks/{kept}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case \"a network this store holds\": expected 200, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["networkId"].as_str(),
        Some(kept.as_str()),
        "case \"a network this store holds\": expected the network back, got {}",
        answer.body
    );
}
