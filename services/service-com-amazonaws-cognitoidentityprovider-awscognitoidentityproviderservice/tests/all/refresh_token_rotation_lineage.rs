// SPDX-License-Identifier: CC0-1.0

//! Refresh-token ROTATION: which refresh token of a sign-in authorizes a refresh,
//! how a rotation that has invalidated one is reported, and which configurations
//! an app client may hold.
//!
//! Three rules are under test.
//!
//! * ONE TOKEN AT A TIME. A rotating app client "issues new ID, access, and
//!   refresh tokens when users renew their sessions with refresh tokens"
//!   (`RefreshTokenRotationType`) and the submitted token "is invalidated after
//!   the request is complete and after an optional grace period"
//!   (`GetTokensFromRefreshToken.RefreshToken`), the grace period being the window
//!   in which "the original refresh token that you're rotating out can remain
//!   valid ... This allows for client-side retries"
//!   (`RefreshTokenRotationType.RetryGracePeriodSeconds`) — so a refresh made with
//!   a token an earlier rotation already rotated out is a RETRY of that rotation:
//!   the token the first attempt issued is one nobody holds, the retry issues the
//!   one that takes its place, and the abandoned successor is superseded. A
//!   refresh presenting a token rotation has invalidated — the submitted one past
//!   its grace period, or a successor a retry superseded — is refused
//!   `RefreshTokenReuseException`, which "is throw when your application requests
//!   token refresh with a refresh token that has been invalidated by
//!   refresh-token rotation" (this service's model).
//! * THE LEGACY FLOW IS WITHDRAWN. `InitiateAuth`'s `REFRESH_TOKEN_AUTH` /
//!   `REFRESH_TOKEN` flow "receive[s] new ID and access tokens"
//!   (`InitiateAuth.AuthFlow`) and leaves the submitted refresh token in force,
//!   which is not what a rotating client does with one, so on such a client the
//!   flow answers `UnsupportedOperationException` — "thrown when you attempt to
//!   perform an operation that isn't enabled for the user pool client".
//! * THE CONFIGURATION IS STORED. Every `RefreshTokenRotation` the model admits —
//!   its `@required` `Feature` in either state, with or without a
//!   `RetryGracePeriodSeconds` inside the `@range 0..60` — is a configuration an
//!   app client holds: `CreateUserPoolClient` and `UpdateUserPoolClient` store it
//!   and a later `DescribeUserPoolClient` reports it back unchanged, so what a
//!   client rotates with is read from stored state. The configurations the model
//!   does NOT admit are refused by its own constraints, which `zz_probe2.rs` pins.
//!
//! Nothing here waits for a grace period to run out: the rows that need an
//! already-invalidated token use the `0` the member documents as "the grace period
//! is disabled and a successful request immediately invalidates the submitted
//! refresh token".
//!
//! Each row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The user every row signs in.
const USERNAME: &str = "rotating";

/// The operation that renews a session on a rotating app client.
const REFRESH: &str = "GetTokensFromRefreshToken";

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

/// A pool whose user can sign in, and one app client configured with `rotation`,
/// returned as (`UserPoolId`, `ClientId`).
async fn pool_with_client(router: &MultiServiceRouter, rotation: Option<Value>) -> (String, String) {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({ "PoolName": "rotation-pool" }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!("rotation-client"));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!(["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]),
    );
    if let Some(rotation) = rotation {
        body.insert("RefreshTokenRotation".to_string(), rotation);
    }
    let client = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    (user_pool_id, client_id)
}

/// Sign the suite's user in and return the refresh token the sign-in issued.
async fn sign_in(router: &MultiServiceRouter, client_id: &str) -> String {
    let result = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    result["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .expect("an issued refresh token")
        .to_string()
}

/// Renew the session `refresh_token` belongs to, and return the status, the body
/// and the refresh token the renewal issued (`None` when it issued none).
async fn refresh(
    router: &MultiServiceRouter,
    client_id: &str,
    refresh_token: &str,
) -> (u16, Value, Option<String>) {
    let (status, payload) = call(
        router,
        REFRESH,
        &json!({ "ClientId": client_id, "RefreshToken": refresh_token }),
    )
    .await;
    let issued = payload["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .map(str::to_string);
    (status, payload, issued)
}

/// The grace period a row configures, in seconds: `0` is the published minimum,
/// which the member documents as disabling the grace period so that "a
/// successful request immediately invalidates the submitted refresh token".
const NO_GRACE_PERIOD: i64 = 0;

#[tokio::test]
async fn a_rotated_out_refresh_token_is_refused_as_reuse() {
    // Both ways of asking for no grace period at all: the member left out, and
    // the published minimum stated.
    for rotation in [
        json!({ "Feature": "ENABLED" }),
        json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": NO_GRACE_PERIOD }),
    ] {
        let case = format!("rotation {rotation}");
        let router = router();
        let (_user_pool_id, client_id) = pool_with_client(&router, Some(rotation.clone())).await;
        let first = sign_in(&router, &client_id).await;

        // The rotation issues the token that replaces the submitted one.
        let (status, payload, second) = refresh(&router, &client_id, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first refresh must be served, got {status} with {payload}"
        );
        let second = second.unwrap_or_else(|| {
            panic!("case {case:?}: a rotating client must issue a new refresh token, got {payload}")
        });
        assert_ne!(
            second, first,
            "case {case:?}: the issued refresh token must not be the submitted one"
        );

        // With no grace period the submitted token is invalidated by the request
        // that rotated it out, so presenting it again is the reuse the modelled
        // exception is for.
        let (status, payload, _) = refresh(&router, &client_id, &first).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "RefreshTokenReuseException"),
            "case {case:?}: a rotated-out refresh token must be refused as reuse, got {status} \
             with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            REUSE_MESSAGE,
            "case {case:?}: the reuse refusal carries its own sentence, got {payload}"
        );

        // The refusal invalidated nothing: the token the rotation issued is still
        // the one in force, and rotates in its turn.
        let (status, payload, third) = refresh(&router, &client_id, &second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the token the rotation issued must refresh, got {status} with {payload}"
        );
        assert!(
            third.is_some_and(|third| third != second),
            "case {case:?}: the second rotation must issue a token of its own, got {payload}"
        );

        // And it was itself invalidated by the request it authorized.
        let (status, payload, _) = refresh(&router, &client_id, &second).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "RefreshTokenReuseException"),
            "case {case:?}: the token that authorized the second rotation is rotated out too, \
             got {status} with {payload}"
        );
    }
}

/// The sentence a refresh token refresh-token rotation has already invalidated is
/// refused with.
const REUSE_MESSAGE: &str = "Refresh token reuse detected";

#[tokio::test]
async fn a_retry_inside_the_grace_period_supersedes_the_token_it_replaces() {
    // The rows span the rest of the member's published range, the maximum
    // included, so the submitted token is certainly still inside its retry window
    // when the row replays it.
    for grace in [30, 60] {
        let case = format!("RetryGracePeriodSeconds {grace}");
        let router = router();
        let (_user_pool_id, client_id) = pool_with_client(
            &router,
            Some(json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": grace })),
        )
        .await;
        let first = sign_in(&router, &client_id).await;

        let (status, payload, second) = refresh(&router, &client_id, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first refresh must be served, got {status} with {payload}"
        );
        let second = second.unwrap_or_else(|| {
            panic!("case {case:?}: the first rotation must issue a refresh token, got {payload}")
        });

        // The retry the grace period exists for: the SUBMITTED token is still
        // accepted, and the token it issues replaces the one the first rotation
        // issued.
        let (status, payload, retried) = refresh(&router, &client_id, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a retry inside the grace period must be served, got {status} with \
             {payload}"
        );
        let retried = retried.unwrap_or_else(|| {
            panic!("case {case:?}: the retry must issue a refresh token, got {payload}")
        });
        assert_ne!(
            retried, second,
            "case {case:?}: the retry must issue a token of its own"
        );

        let (status, payload, _) = refresh(&router, &client_id, &second).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "RefreshTokenReuseException"),
            "case {case:?}: the token the retry superseded must be refused as reuse, got {status} \
             with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            REUSE_MESSAGE,
            "case {case:?}: the reuse refusal carries its own sentence, got {payload}"
        );

        let (status, payload, _) = refresh(&router, &client_id, &retried).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the token the retry issued is the one in force, got {status} with \
             {payload}"
        );
    }
}

#[tokio::test]
async fn the_legacy_refresh_flow_is_not_available_on_a_rotating_client() {
    // (the rotation each row configures, whether the legacy flow is served)
    let rows = [
        (None, true),
        (Some(json!({ "Feature": "DISABLED" })), true),
        (Some(json!({ "Feature": "ENABLED" })), false),
        (
            Some(json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": 60 })),
            false,
        ),
    ];

    for (rotation, served) in rows {
        let case = format!("rotation {rotation:?}");
        let router = router();
        let (_user_pool_id, client_id) = pool_with_client(&router, rotation.clone()).await;
        let refresh_token = sign_in(&router, &client_id).await;

        for auth_flow in ["REFRESH_TOKEN_AUTH", "REFRESH_TOKEN"] {
            let (status, payload) = call(
                &router,
                "InitiateAuth",
                &json!({
                    "ClientId": client_id,
                    "AuthFlow": auth_flow,
                    "AuthParameters": { "REFRESH_TOKEN": refresh_token },
                }),
            )
            .await;
            if served {
                assert_eq!(
                    status, 200,
                    "case {case:?} / {auth_flow}: a client that does not rotate serves the flow, \
                     got {status} with {payload}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&payload), error_message(&payload)),
                    (
                        400,
                        "UnsupportedOperationException",
                        "This API does not support refresh token rotation"
                    ),
                    "case {case:?} / {auth_flow}: a rotating client withdraws the flow, got \
                     {status} with {payload}"
                );
            }
        }
    }
}

/// Every `RefreshTokenRotation` the model admits is a configuration an app client
/// HOLDS: the create and the update store it and a later describe reports it
/// back unchanged, so what a client rotates with is read from stored state rather
/// than from the request that configured it. The configurations the model does NOT
/// admit are refused by its own constraints, which `zz_probe2.rs` pins.
#[tokio::test]
async fn every_rotation_configuration_the_model_admits_is_stored_and_read_back() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, None).await;

    // The rows span the two members: the feature in each of its two states, the
    // grace period absent, at its published minimum and at a value in between.
    let rows = [
        json!({ "Feature": "DISABLED" }),
        json!({ "Feature": "DISABLED", "RetryGracePeriodSeconds": 30 }),
        json!({ "Feature": "ENABLED" }),
        json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": 0 }),
        json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": 60 }),
    ];

    for rotation in rows {
        let case = format!("rotation {rotation}");
        let created = ok(
            &router,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientName": "configured-client",
                "RefreshTokenRotation": rotation,
            }),
        )
        .await;
        assert_eq!(
            created["UserPoolClient"]["RefreshTokenRotation"], rotation,
            "case {case:?}: the create answers the configuration it stored, got {created}"
        );

        ok(
            &router,
            "UpdateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "RefreshTokenRotation": rotation,
            }),
        )
        .await;
        let described = ok(
            &router,
            "DescribeUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
        )
        .await;
        assert_eq!(
            described["UserPoolClient"]["RefreshTokenRotation"], rotation,
            "case {case:?}: a describe reports the configuration the update stored, got {described}"
        );
    }
}
