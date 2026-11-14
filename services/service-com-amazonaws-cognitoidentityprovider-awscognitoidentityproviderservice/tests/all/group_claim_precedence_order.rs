// SPDX-License-Identifier: CC0-1.0

//! The ORDER a pool's group claim lists a user's groups in.
//!
//! RULE: the `cognito:groups` claim of the tokens a sign-in issues lists the
//! groups the stored user belongs to ordered by ascending `Precedence`, with a
//! group that states none behind every group that states one, and groups tied at
//! one precedence in the stable stored order. `CreateGroup`'s own `Precedence`
//! documentation is where that order comes from (this service's model): "Zero is
//! the highest precedence value. Groups with lower `Precedence` values take
//! precedence over groups with higher or null `Precedence` values", and the claim
//! is what a reader reads that precedence out of — "Amazon Cognito ID tokens have
//! a `cognito:preferred_role` claim that presents the highest-precedence group
//! that a user belongs to. Both ID and access tokens also contain a
//! `cognito:groups` claim" (`CreateGroup.RoleArn`).
//!
//! The rows differ along what the rule generalises over: the COUNT of groups, the
//! precedence values themselves (including the documented `0` minimum and the
//! documented `2^31-1` maximum), whether a group states one at all, whether two
//! groups tie, and the order the memberships were ADDED in — which is never what
//! decides the claim.
//!
//! Every group, membership and token here is real: they come from `CreateGroup`,
//! `AdminAddUserToGroup` and `AdminInitiateAuth`, and the claim is read out of the
//! issued tokens themselves.

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

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The documented maximum of `CreateGroup`'s `Precedence`: "The maximum
/// `Precedence` value is `2^31-1`" (this service's model).
const MAX_PRECEDENCE: i64 = 2_147_483_647;

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

/// A base64url JWS segment's bytes (RFC 7515 §2), decoded by the one JOSE codec
/// this workspace holds (`service_common::jwt`).
fn decode_base64url(segment: &str) -> Vec<u8> {
    service_common::jwt::decode_base64url(segment)
        .unwrap_or_else(|| panic!("{segment:?} must be base64url"))
}

/// The `cognito:groups` claim of the compact-serialized JWS `token`, or `None`
/// when it carries none.
fn groups_claim(token: &str) -> Option<Vec<String>> {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a JWS compact serialization has three segments, got {token}"
    );
    let claims: Value = serde_json::from_slice(&decode_base64url(segments[1]))
        .unwrap_or_else(|error| panic!("the claims must be JSON ({error}): {token}"));
    let claim = claims.get("cognito:groups")?.clone();
    Some(
        claim
            .as_array()
            .unwrap_or_else(|| panic!("cognito:groups must be a list, got {claim}"))
            .iter()
            .map(|name| {
                name.as_str()
                    .unwrap_or_else(|| panic!("a group name must be a string, got {name}"))
                    .to_string()
            })
            .collect(),
    )
}

/// A pool with one app client that admits `ADMIN_USER_PASSWORD_AUTH`, returned as
/// (`UserPoolId`, `ClientId`).
async fn pool_with_client(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let pool = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{name}-client"),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with a permanent password so it can sign in.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
}

/// Sign `username` in and return the `AuthenticationResult`.
async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> Value {
    let result = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    result["AuthenticationResult"].clone()
}

/// One group of a row: the name it is created under and the `Precedence` it
/// states, `None` for the documented default of none at all.
struct GroupSpec {
    name: &'static str,
    precedence: Option<i64>,
}

/// One row of the table: the user that signs in, the groups the pool holds for
/// it — added in the row's own order, which the claim never follows — and the
/// order the claim must list them in.
struct OrderCase {
    case: &'static str,
    username: &'static str,
    groups: Vec<GroupSpec>,
    expected: Vec<&'static str>,
}

fn order_cases() -> Vec<OrderCase> {
    vec![
        OrderCase {
            case: "one group, which states a precedence",
            username: "single",
            groups: vec![GroupSpec {
                name: "single-only",
                precedence: Some(7),
            }],
            expected: vec!["single-only"],
        },
        OrderCase {
            // Added worst precedence first, so insertion order and precedence
            // order disagree in every position.
            case: "three groups added in descending precedence order",
            username: "three",
            groups: vec![
                GroupSpec {
                    name: "three-hi",
                    precedence: Some(10),
                },
                GroupSpec {
                    name: "three-lo",
                    precedence: Some(1),
                },
                GroupSpec {
                    name: "three-mid",
                    precedence: Some(5),
                },
            ],
            expected: vec!["three-lo", "three-mid", "three-hi"],
        },
        OrderCase {
            // The two documented ends of the range, added worst first.
            case: "the documented precedence bounds 0 and 2^31-1",
            username: "bounds",
            groups: vec![
                GroupSpec {
                    name: "bounds-max",
                    precedence: Some(MAX_PRECEDENCE),
                },
                GroupSpec {
                    name: "bounds-zero",
                    precedence: Some(0),
                },
            ],
            expected: vec!["bounds-zero", "bounds-max"],
        },
        OrderCase {
            // "Groups with lower Precedence values take precedence over groups
            // with higher or NULL Precedence values", so the group that states
            // none goes last however early it was added.
            case: "a group stating no precedence beside two that do",
            username: "unstated",
            groups: vec![
                GroupSpec {
                    name: "unstated-none",
                    precedence: None,
                },
                GroupSpec {
                    name: "unstated-high",
                    precedence: Some(42),
                },
                GroupSpec {
                    name: "unstated-low",
                    precedence: Some(2),
                },
            ],
            expected: vec!["unstated-low", "unstated-high", "unstated-none"],
        },
        OrderCase {
            // "Two groups can have the same Precedence value. If this happens,
            // neither group takes precedence over the other": the tie keeps the
            // stored order, which is by name, so the claim is stable.
            case: "two groups tied at one precedence beside a better one",
            username: "tied",
            groups: vec![
                GroupSpec {
                    name: "tied-b",
                    precedence: Some(9),
                },
                GroupSpec {
                    name: "tied-a",
                    precedence: Some(9),
                },
                GroupSpec {
                    name: "tied-best",
                    precedence: Some(3),
                },
            ],
            expected: vec!["tied-best", "tied-a", "tied-b"],
        },
        OrderCase {
            case: "every group stating no precedence at all",
            username: "none",
            groups: vec![
                GroupSpec {
                    name: "none-second",
                    precedence: None,
                },
                GroupSpec {
                    name: "none-first",
                    precedence: None,
                },
            ],
            expected: vec!["none-first", "none-second"],
        },
    ]
}

#[tokio::test]
async fn the_group_claim_is_ordered_by_ascending_precedence() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "precedence-pool").await;

    for row in order_cases() {
        let case = row.case;
        user(&router, &user_pool_id, row.username).await;
        for group in &row.groups {
            let mut request = json!({
                "UserPoolId": user_pool_id,
                "GroupName": group.name,
            });
            if let Some(precedence) = group.precedence {
                request["Precedence"] = json!(precedence);
            }
            ok(&router, "CreateGroup", &request).await;
            ok(
                &router,
                "AdminAddUserToGroup",
                &json!({
                    "UserPoolId": user_pool_id,
                    "Username": row.username,
                    "GroupName": group.name,
                }),
            )
            .await;
        }

        let expected: Vec<String> = row.expected.iter().map(|name| (*name).to_string()).collect();
        // The claim is carried by the access and the identity token, and both
        // read the same stored precedences, so neither may order them its own
        // way. Two sign-ins are read, so the order is the rule's and not one
        // request's.
        for attempt in ["the first sign-in", "a second sign-in"] {
            let result = sign_in(&router, &user_pool_id, &client_id, row.username).await;
            for member in ["AccessToken", "IdToken"] {
                let token = result[member].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: {member} must be issued, got {result}")
                });
                let actual = groups_claim(token);
                assert_eq!(
                    actual.as_deref(),
                    Some(expected.as_slice()),
                    "case {case:?} / {member} / {attempt}: the group claim must be ordered by \
                     ascending Precedence as {expected:?}, got {actual:?}"
                );
            }
        }
    }
}
