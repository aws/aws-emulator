// SPDX-License-Identifier: CC0-1.0

//! How `GetNetwork` refuses a `networkId` it cannot serve, driven over HTTP against one
//! running emulator.
//!
//! The rule under test: this API resolves ownership of the `networkId` path label
//! before it validates the label or reports anything about the network behind it, so a
//! request it cannot serve from a network of this account is refused in the
//! authorization class with a fixed sentence that names neither the member nor the id —
//! never as a validation failure and never as a statement that the resource was not
//! found. Three classes of id, every one of them refused that way:
//!
//! * a label outside `NetworkId` (`@length` 8, `@pattern ^[0-9]{8}$`, from
//!   `smithy_query(query="shape",shape="NetworkId")`) is denied before the model's own
//!   constraints are reached — `ForbiddenError` (`@httpError(403)`);
//! * a network this account owned and deleted — `ForbiddenError` (403) with the
//!   service's "gone" sentence, which is the terminal answer a client polling for the
//!   end of an initiated deletion waits for;
//! * a well-formed label this store cannot place at all — `ForbiddenError`
//!   (`@httpError(403)`), disclosing nothing about whether such a network exists.
//!
//! All three are the PERMISSION class: the caller is authenticated, so what the answer
//! states is that these credentials may not be shown the resource. The authentication
//! class (`UnauthorizedError`, `@httpError(401)`) is for a request whose credentials are
//! missing or invalid, which no id in the path can make true.
//!
//! Every case goes through the real multi-service router — route resolution, restJson1
//! deserialization, modelled-constraint validation, the handler and response
//! serialization — so what it asserts is the wire behaviour a client sees.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_wickr_wickradminapi::state::{
    ACCESS_DENIED_MESSAGE, NETWORK_GONE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

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

/// A fresh emulator holding only this service, over its own engine, so one test's
/// networks are invisible to another's.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is not the subject here; the refusal path is.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_wickr_wickradminapi::EmulatedService::new(
        engine,
    ));
    router
}

/// One request/response round trip.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
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

/// Assert `answer` is the denial this API gives for an id the caller cannot be shown to
/// own: the expected status and error shape, the expected fixed sentence, and nothing in
/// the body naming the id that was asked for.
fn assert_denied(case: &str, answer: &Answer, status: u16, shape: &str, message: &str, asked: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected status {status}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.wire_code(),
        shape,
        "case {case:?}: expected the modelled {shape}, got {}",
        answer.body
    );
    assert_eq!(
        answer.error_type, shape,
        "case {case:?}: expected x-amzn-errortype {shape}, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case:?}: expected the fixed message {message:?}, got {:?}",
        answer.message()
    );
    assert!(
        !answer.body.to_string().contains(asked),
        "case {case:?}: the refusal must not disclose the id it was asked about, got {}",
        answer.body
    );
}

#[tokio::test]
async fn a_network_id_outside_the_modelled_shape_is_denied() {
    let router = emulator();
    // `NetworkId` is `@length(8, 8)` with `@pattern ^[0-9]{8}$`: one character under
    // and one over the length, eight characters that are not digits at all, a digit
    // string with one non-digit at either end, and one with a separator inside.
    let cases: &[(&str, &str)] = &[
        ("one under the length", "1234567"),
        ("far under the length", "1"),
        ("one over the length", "123456789"),
        ("eight non-digits", "abcdefgh"),
        ("last character not a digit", "1234567a"),
        ("first character not a digit", "a1234567"),
        ("a separator inside", "1234-678"),
    ];
    for (case, network_id) in cases {
        let answer = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_denied(
            case,
            &answer,
            403,
            "ForbiddenError",
            ACCESS_DENIED_MESSAGE,
            network_id,
        );
    }
}

#[tokio::test]
async fn a_well_formed_network_id_that_names_no_network_is_denied() {
    let router = emulator();
    // Well formed per `NetworkId` and never minted by this store: the low end of the
    // 8-digit space, the high end, one just below the first id a store mints, and one
    // in the middle. A network this store DID mint is readable, so the denial is not
    // simply "every id is denied".
    let owned = create_network(&router, "owned-net").await;
    let cases: &[(&str, &str)] = &[
        ("the low end of the id space", "00000001"),
        ("all zeroes", "00000000"),
        ("just below the first minted id", "09999999"),
        ("the middle of the id space", "54321098"),
        ("the high end of the id space", "99999999"),
    ];
    for (case, network_id) in cases {
        assert_ne!(
            *network_id, owned,
            "case {case:?}: the unknown id must not be the one this store minted"
        );
        let answer = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_denied(
            case,
            &answer,
            403,
            "ForbiddenError",
            ACCESS_DENIED_MESSAGE,
            network_id,
        );
    }

    let answer = call(&router, "GET", &format!("/networks/{owned}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case \"a network this store minted\": expected 200, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["networkId"].as_str(),
        Some(owned.as_str()),
        "case \"a network this store minted\": expected the network back, got {}",
        answer.body
    );
}

#[tokio::test]
async fn a_deleted_network_is_denied_as_gone_and_its_neighbours_are_not() {
    let router = emulator();
    // Several networks, so what the refusal answers is the id that was deleted and not
    // "the store is empty": one deleted, one left alone, and one deleted after the
    // first to show the answer is per id rather than per store.
    let first = create_network(&router, "first-net").await;
    let kept = create_network(&router, "kept-net").await;
    let second = create_network(&router, "second-net").await;

    for (case, network_id) in [("the first deleted", &first), ("the second deleted", &second)] {
        let answer = call(&router, "DELETE", &format!("/networks/{network_id}"), None).await;
        assert!(
            (200..300).contains(&answer.status),
            "case {case:?}: DeleteNetwork expected a success, got {} with {}",
            answer.status,
            answer.body
        );

        // Deletion is initiated rather than finished, so a client reaches the terminal
        // answer by asking again until it stops changing — the loop is here to drive
        // the same poll a client waiting out a deletion makes, and against this store
        // the first answer is already the terminal one, so it converges without
        // iterating. The deadline is generous: it bounds the loop, it is not the thing
        // under test, and the assertion after it is the check.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut answer = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        while answer.status != 403 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
            answer = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        }
        assert_denied(
            case,
            &answer,
            403,
            "ForbiddenError",
            NETWORK_GONE_MESSAGE,
            network_id,
        );
    }

    let answer = call(&router, "GET", &format!("/networks/{kept}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case \"the network left alone\": expected 200, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["networkName"].as_str(),
        Some("kept-net"),
        "case \"the network left alone\": expected its own record, got {}",
        answer.body
    );
}

#[tokio::test]
async fn the_three_refusal_classes_are_told_apart() {
    // One table over all three classes at once, so the statuses, shapes and sentences
    // cannot drift apart: the answer depends only on which class the id falls in.
    let router = emulator();
    let deleted = create_network(&router, "doomed-net").await;
    let answer = call(&router, "DELETE", &format!("/networks/{deleted}"), None).await;
    assert!(
        (200..300).contains(&answer.status),
        "DeleteNetwork: expected a success, got {} with {}",
        answer.status,
        answer.body
    );

    let cases: Vec<(&str, String, u16, &str, &str)> = vec![
        (
            "outside the modelled shape",
            "12345".to_string(),
            403,
            "ForbiddenError",
            ACCESS_DENIED_MESSAGE,
        ),
        (
            "owned and deleted",
            deleted.clone(),
            403,
            "ForbiddenError",
            NETWORK_GONE_MESSAGE,
        ),
        (
            "well formed and unplaceable",
            "87654321".to_string(),
            403,
            "ForbiddenError",
            ACCESS_DENIED_MESSAGE,
        ),
    ];
    for (case, network_id, status, shape, message) in cases {
        let answer = call(&router, "GET", &format!("/networks/{network_id}"), None).await;
        assert_denied(case, &answer, status, shape, message, &network_id);
        assert_ne!(
            answer.wire_code(),
            "ResourceNotFoundError",
            "case {case:?}: a read this API denies is never a not-found report, got {}",
            answer.body
        );
        assert_ne!(
            answer.wire_code(),
            "ValidationError",
            "case {case:?}: a read this API denies is never a validation failure, got {}",
            answer.body
        );
        assert_ne!(
            answer.wire_code(),
            "UnauthorizedError",
            "case {case:?}: a read this API denies is never an authentication failure, got {}",
            answer.body
        );
    }
}
