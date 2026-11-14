// SPDX-License-Identifier: CC0-1.0

//! Wire probes of the app-client rotation surface: the modelled constraints of
//! `RefreshTokenRotation`.
//!
//! `RefreshTokenRotationType` carries a `@required` `Feature` of the enum
//! `FeatureType` (`ENABLED` / `DISABLED`) and a `RetryGracePeriodSeconds` of
//! `@range 0..60`
//! (`smithy_query(query="shape",shape="RefreshTokenRotationType")`), so a request
//! that omits the feature, names a value outside its set, or asks for a grace
//! period outside that range is refused by the modelled constraints before
//! `CreateUserPoolClient`'s handler runs. The rule this suite exists for is the
//! consequence: a refused create stores no app client at all, so the pool still
//! lists exactly the clients it held.
//!
//! The refusal's envelope (`N validation error detected: …`) and its clause (the
//! member and the constraint it missed) are asserted; the rendering of the
//! rejected VALUE inside that envelope belongs to the framework's constraint
//! checker, which every service shares, so it is not pinned here. What a
//! configuration the model DOES admit is worth — stored by the create and the
//! update, reported back by the describe, and deciding what a refresh rotates —
//! is asserted in `refresh_token_rotation_lineage.rs`, beside the rotation
//! behaviour it belongs to.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// The `ClientId`s the pool `user_pool_id` holds app clients under.
async fn client_ids(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListUserPoolClients",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    listed["UserPoolClients"]
        .as_array()
        .map(|clients| {
            clients
                .iter()
                .filter_map(|client| client["ClientId"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_rotation_outside_the_modelled_constraints_is_refused_and_stores_nothing() {
    let router = router();
    let pool = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "rotation-constraint-pool" }),
    )
    .await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    // The app client the refusals have to leave the pool holding.
    ok(
        &router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": "held-client" }),
    )
    .await;
    let before = client_ids(&router, &user_pool_id).await;

    // One row per modelled constraint of the structure: the required member, the
    // enum's value set (in the value's own case, so a near miss is a miss), and
    // both ends of the integer range.
    let rows = [
        (
            "no Feature at all, which the model requires",
            json!({ "RetryGracePeriodSeconds": 30 }),
            "refreshTokenRotation.feature",
        ),
        (
            "a Feature outside the modelled value set",
            json!({ "Feature": "ROTATING" }),
            "refreshTokenRotation.feature",
        ),
        (
            "a Feature in the wrong case",
            json!({ "Feature": "enabled" }),
            "refreshTokenRotation.feature",
        ),
        (
            "a grace period one past the published maximum",
            json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": 61 }),
            "refreshTokenRotation.retryGracePeriodSeconds",
        ),
        (
            "a grace period one below the published minimum",
            json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": -1 }),
            "refreshTokenRotation.retryGracePeriodSeconds",
        ),
    ];

    for (case, rotation, member) in rows {
        let (status, payload) = call(
            &router,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientName": "constraint-client",
                "RefreshTokenRotation": rotation,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {case:?}: the refusal keeps the validation envelope, got {message:?}"
        );
        assert!(
            message.contains(&format!("at '{member}' failed to satisfy constraint:")),
            "case {case:?}: the refusal names the member it missed, got {message:?}"
        );

        // Refused before anything was written: the pool still holds exactly the
        // app clients it held.
        let after = client_ids(&router, &user_pool_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must store no app client, got {after:?}"
        );
    }
}
