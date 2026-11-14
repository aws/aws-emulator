// SPDX-License-Identifier: CC0-1.0

//! What a SUCCESSFUL authentication response looks like on the wire, for every
//! operation of this service that completes or continues a sign-in.
//!
//! Three rules are under test.
//!
//! * `ChallengeParameters` IS ALWAYS THERE. `InitiateAuth`, `AdminInitiateAuth`,
//!   `RespondToAuthChallenge` and `AdminRespondToAuthChallenge` each model
//!   `ChallengeParameters` as a `ChallengeParametersType` map, and a successful
//!   response carries it whether or not a challenge is outstanding: the
//!   parameters of the challenge the caller must answer, or an EMPTY map once
//!   authentication is complete. A caller therefore reads the member
//!   unconditionally. The table spans every way those four operations complete a
//!   sign-in (two password flows, the legacy admin flow, both refresh-token
//!   aliases, an answered password challenge, an answered one-time-password
//!   challenge) and every way they leave one outstanding.
//! * A CHALLENGE REPORTS THE USER'S OWN ATTRIBUTES. The `userAttributes`
//!   parameter of a `NEW_PASSWORD_REQUIRED` challenge carries the values the
//!   stored user already has, and not the `sub` the pool mints for every user:
//!   `sub` is immutable and is not an attribute any challenge response can
//!   supply, which is why it is never among the `requiredAttributes` either. The
//!   rows differ along how many attributes the user holds and of which kind,
//!   starting with a user whose only attribute is that minted `sub`.
//! * A REFRESH TOKEN IS AN OPAQUE CREDENTIAL, and the key of a newly seen device
//!   is UUID-shaped. The access and id tokens beside it are signed statements a
//!   verifier reads; the refresh token states nothing its holder can read, while
//!   still being the credential that refreshes the sign-in.
//!
//! Each table row names the case it asserts.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The region the suite's calls are signed for, which is the region the pools they
/// create live in.
const REGION: &str = "us-east-1";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The temporary password an administrator-created user signs in with once, which is
/// what leaves it owing a `NEW_PASSWORD_REQUIRED` challenge.
const TEMPORARY_PASSWORD: &str = "Temp0rary!2026";

/// The challenge an administrator-created user has to answer.
const NEW_PASSWORD_REQUIRED: &str = "NEW_PASSWORD_REQUIRED";

/// The one-time-password challenge of the choice-based flow.
const EMAIL_OTP: &str = "EMAIL_OTP";

/// The lengths of the five hyphen-separated groups of a canonical UUID
/// (RFC 9562 §4: 8-4-4-4-12 hexadecimal characters).
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// One emulator with just this service mounted, beside the engine its state lives in.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        )
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
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// A created pool and one of its app clients.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Create a pool carrying `pool_members` and one app client carrying `client_members`,
/// and return both ids. The client allows every flow this suite drives.
async fn pool_with_client(
    router: &MultiServiceRouter,
    pool_name: &str,
    pool_members: &Value,
    client_members: &Value,
) -> Fixture {
    let mut pool_body = Map::new();
    pool_body.insert("PoolName".to_string(), json!(pool_name));
    for (name, value) in pool_members.as_object().expect("an object of members") {
        pool_body.insert(name.clone(), value.clone());
    }
    let created = ok(router, "CreateUserPool", &Value::Object(pool_body)).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();

    let mut client_body = Map::new();
    client_body.insert("UserPoolId".to_string(), json!(user_pool_id));
    client_body.insert("ClientName".to_string(), json!("auth-client"));
    client_body.insert(
        "ExplicitAuthFlows".to_string(),
        json!([
            "ALLOW_USER_PASSWORD_AUTH",
            "ALLOW_ADMIN_USER_PASSWORD_AUTH",
            "ALLOW_REFRESH_TOKEN_AUTH",
            "ALLOW_USER_AUTH",
        ]),
    );
    for (name, value) in client_members.as_object().expect("an object of members") {
        client_body.insert(name.clone(), value.clone());
    }
    let client = ok(
        router,
        "CreateUserPoolClient",
        &Value::Object(client_body),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// Create `username` with `attributes` and a PERMANENT password, so it is `CONFIRMED`
/// and signs straight in, and return its `sub`.
async fn confirmed_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> String {
    let sub = created_user(router, user_pool_id, username, attributes, None).await;
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
    sub
}

/// Create `username` with `attributes` and `temporary_password`, and return the `sub`
/// the pool minted for it.
async fn created_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
    temporary_password: Option<&str>,
) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Username".to_string(), json!(username));
    if !attributes.is_empty() {
        body.insert(
            "UserAttributes".to_string(),
            Value::Array(
                attributes
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            ),
        );
    }
    if let Some(password) = temporary_password {
        body.insert("TemporaryPassword".to_string(), json!(password));
    }
    let created = ok(router, "AdminCreateUser", &Value::Object(body)).await;
    created["User"]["Attributes"]
        .as_array()
        .expect("the created user's attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("the pool mints a sub for every user")
        .to_string()
}

/// A `ChallengeParameters` map as a response carried it, asserting that the member is
/// PRESENT and a map wherever this suite reads one.
fn challenge_parameters(case: &str, payload: &Value) -> Map<String, Value> {
    let member = payload.get("ChallengeParameters").unwrap_or_else(|| {
        panic!(
            "case {case:?}: a successful authentication response always carries \
             ChallengeParameters, got {payload}"
        )
    });
    member
        .as_object()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ChallengeParameters is a map, got {member}")
        })
        .clone()
}

/// The JSON document `token` hands its holder, or `None` when it hands none: the claim
/// segment of a compact-serialized JWS, else the whole value read as one base64url run.
/// Decoded with this crate's own codec (`state::base64_url_decode`), the one the service
/// encodes such a run with.
fn readable_document(token: &str) -> Option<Value> {
    let segments: Vec<&str> = token.split('.').collect();
    let candidate = if segments.len() == 3 { segments[1] } else { token };
    serde_json::from_slice(&state::base64_url_decode(candidate)?).ok()
}

/// Whether `value` satisfies the modelled `TokenModelType` pattern
/// `^[A-Za-z0-9-_=.]+$` (this service's model, the target of every token member of
/// `AuthenticationResultType`).
fn is_modelled_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=' | '.'))
}

/// Whether `value` has the shape of a canonical UUID: [`UUID_GROUP_LENGTHS`] groups of
/// lowercase hexadecimal, hyphen-separated.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == UUID_GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_LENGTHS)
            .all(|(group, length)| {
                group.len() == length
                    && group
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
}

/// The one-time password a challenge session delivered, read out of the emulator's own
/// state because nothing in the response carries it.
fn session_code(engine: &Arc<engine::InMemoryEngine>, session: &str) -> String {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .auth_sessions
        .get(session)
        .and_then(|stored| stored.code.as_ref())
        .map(|code| code.code.clone())
        .expect("the challenge delivered a code")
}

/// RULE: every successful response of the four authentication operations carries a
/// `ChallengeParameters` map — EMPTY when authentication completed, and the outstanding
/// challenge's parameters when one is pending.
#[tokio::test]
async fn every_authentication_success_carries_a_challenge_parameter_map() {
    let (router, engine) = router_and_engine();
    let fixture = pool_with_client(
        &router,
        "completed-pool",
        &json!({
            "Policies": {
                "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "EMAIL_OTP"] }
            },
        }),
        &json!({}),
    )
    .await;
    confirmed_user(&router, &fixture.user_pool_id, "alice", &[]).await;
    confirmed_user(
        &router,
        &fixture.user_pool_id,
        "otp",
        &[("email", "otp@example.com"), ("email_verified", "true")],
    )
    .await;
    created_user(
        &router,
        &fixture.user_pool_id,
        "newbie",
        &[],
        Some(TEMPORARY_PASSWORD),
    )
    .await;

    // A password sign-in, whose refresh token the refresh rows below present back.
    let signed_in = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let refresh_token = signed_in["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .expect("a sign-in issues a refresh token")
        .to_string();

    // The password challenge of the administrator-created user, answered by one row.
    let challenged = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "newbie", "PASSWORD": TEMPORARY_PASSWORD },
        }),
    )
    .await;
    let password_session = challenged["Session"]
        .as_str()
        .expect("a challenge carries a session")
        .to_string();

    // The one-time-password challenge, answered by another row.
    let started = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "USER_AUTH",
            "AuthParameters": { "USERNAME": "otp", "PREFERRED_CHALLENGE": EMAIL_OTP },
        }),
    )
    .await;
    let otp_session = started["Session"]
        .as_str()
        .expect("a challenge carries a session")
        .to_string();
    let code = session_code(&engine, &otp_session);

    // (case, operation, request body) — every way these operations complete a sign-in.
    let completed = [
        (
            "InitiateAuth with the client-side password flow",
            "InitiateAuth",
            json!({
                "ClientId": fixture.client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "AdminInitiateAuth with the admin password flow",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": fixture.user_pool_id,
                "ClientId": fixture.client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "AdminInitiateAuth with the legacy admin flow",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": fixture.user_pool_id,
                "ClientId": fixture.client_id,
                "AuthFlow": "ADMIN_NO_SRP_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "InitiateAuth with the refresh-token flow",
            "InitiateAuth",
            json!({
                "ClientId": fixture.client_id,
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": refresh_token },
            }),
        ),
        (
            "InitiateAuth with the refresh-token flow's alias",
            "InitiateAuth",
            json!({
                "ClientId": fixture.client_id,
                "AuthFlow": "REFRESH_TOKEN",
                "AuthParameters": { "REFRESH_TOKEN": refresh_token },
            }),
        ),
        (
            "AdminRespondToAuthChallenge answering the password challenge",
            "AdminRespondToAuthChallenge",
            json!({
                "UserPoolId": fixture.user_pool_id,
                "ClientId": fixture.client_id,
                "ChallengeName": NEW_PASSWORD_REQUIRED,
                "Session": password_session,
                "ChallengeResponses": {
                    "USERNAME": "newbie",
                    "NEW_PASSWORD": GOOD_PASSWORD,
                },
            }),
        ),
        (
            "RespondToAuthChallenge answering the one-time password",
            "RespondToAuthChallenge",
            json!({
                "ClientId": fixture.client_id,
                "ChallengeName": EMAIL_OTP,
                "Session": otp_session,
                "ChallengeResponses": { "USERNAME": "otp", "EMAIL_OTP_CODE": code },
            }),
        ),
    ];

    for (case, operation, body) in &completed {
        let payload = ok(&router, operation, body).await;
        let parameters = challenge_parameters(case, &payload);
        assert!(
            parameters.is_empty(),
            "case {case:?}: a completed sign-in leaves no challenge outstanding, so the \
             map is empty, got {parameters:?}"
        );
        assert!(
            payload["AuthenticationResult"]["AccessToken"]
                .as_str()
                .is_some_and(|token| !token.is_empty()),
            "case {case:?}: a completed sign-in returns its tokens, got {payload}"
        );
        assert!(
            payload["ChallengeName"].is_null() && payload["Session"].is_null(),
            "case {case:?}: a completed sign-in carries no next challenge, got {payload}"
        );
    }

    // The same member on the responses that leave a challenge OUTSTANDING: present, and
    // carrying that challenge's own parameters.
    let outstanding = [
        (
            "AdminInitiateAuth starting the password challenge",
            &challenged,
            NEW_PASSWORD_REQUIRED,
            "USER_ID_FOR_SRP",
        ),
        (
            "InitiateAuth starting the one-time-password challenge",
            &started,
            EMAIL_OTP,
            "CODE_DELIVERY_DESTINATION",
        ),
    ];
    for (case, payload, challenge_name, expected_key) in outstanding {
        let parameters = challenge_parameters(case, payload);
        assert_eq!(
            payload["ChallengeName"],
            json!(challenge_name),
            "case {case:?}: expected the {challenge_name} challenge, got {payload}"
        );
        assert!(
            parameters.contains_key(expected_key),
            "case {case:?}: an outstanding challenge reports its own parameters, \
             expected {expected_key}, got {parameters:?}"
        );
    }
}

/// One row of the attribute table: the attributes the challenged user is created with,
/// and the `userAttributes` object the challenge must report for it.
struct AttributeRow {
    case: &'static str,
    username: &'static str,
    attributes: &'static [(&'static str, &'static str)],
}

/// RULE: a `NEW_PASSWORD_REQUIRED` challenge reports the user's OWN stored attributes
/// under `userAttributes` and never the `sub` the pool mints — on the admin flow and on
/// the client-side flow alike.
#[tokio::test]
async fn a_challenge_reports_the_users_attributes_without_the_pool_minted_sub() {
    let rows = [
        AttributeRow {
            case: "a user whose only attribute is the minted sub",
            username: "plain",
            attributes: &[],
        },
        AttributeRow {
            case: "a user with one standard attribute",
            username: "emailed",
            attributes: &[("email", "emailed@example.test")],
        },
        AttributeRow {
            case: "a user with several standard attributes",
            username: "detailed",
            attributes: &[
                ("email", "detailed@example.test"),
                ("email_verified", "true"),
                ("given_name", "Dee"),
                ("phone_number", "+15550100"),
            ],
        },
    ];

    let (router, _engine) = router_and_engine();
    let fixture = pool_with_client(&router, "challenge-pool", &json!({}), &json!({})).await;

    for row in &rows {
        let sub = created_user(
            &router,
            &fixture.user_pool_id,
            row.username,
            row.attributes,
            Some(TEMPORARY_PASSWORD),
        )
        .await;
        let expected: Value = Value::Object(
            row.attributes
                .iter()
                .map(|(name, value)| ((*name).to_string(), json!(value)))
                .collect(),
        );

        // Both operations start this challenge, and both must report it the same way.
        let starts = [
            (
                "AdminInitiateAuth",
                json!({
                    "UserPoolId": fixture.user_pool_id,
                    "ClientId": fixture.client_id,
                    "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                    "AuthParameters": {
                        "USERNAME": row.username,
                        "PASSWORD": TEMPORARY_PASSWORD,
                    },
                }),
            ),
            (
                "InitiateAuth",
                json!({
                    "ClientId": fixture.client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": {
                        "USERNAME": row.username,
                        "PASSWORD": TEMPORARY_PASSWORD,
                    },
                }),
            ),
        ];
        for (operation, body) in &starts {
            let case = format!("{} / {operation}", row.case);
            let payload = ok(&router, operation, body).await;
            let parameters = challenge_parameters(&case, &payload);
            let reported = parameters.get("userAttributes").unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a {NEW_PASSWORD_REQUIRED} challenge reports \
                     userAttributes, got {parameters:?}"
                )
            });
            let reported: Value = serde_json::from_str(
                reported
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: userAttributes is a string \
                                               carrying a JSON object, got {reported}")),
            )
            .unwrap_or_else(|error| {
                panic!("case {case:?}: userAttributes must be JSON ({error}): {reported}")
            });
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the user's own attributes {expected}, got {reported}"
            );
            assert!(
                !payload.to_string().contains(sub.as_str()),
                "case {case:?}: no challenge parameter carries the minted sub, got {payload}"
            );
            // The attributes the challenge ASKS for never include it either.
            let required = parameters
                .get("requiredAttributes")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the challenge reports requiredAttributes, \
                            got {parameters:?}")
                });
            assert!(
                !required.contains("sub"),
                "case {case:?}: the pool mints sub, so no challenge asks for it, \
                 got {required}"
            );
        }
    }
}

/// RULE: the refresh token every completed sign-in issues is an OPAQUE credential — it
/// satisfies the modelled token pattern, hands its holder no readable document, and
/// carries nothing of the sign-in it belongs to — while the access and id tokens beside
/// it are the signed statements a verifier reads. It is nonetheless the credential that
/// refreshes the sign-in.
#[tokio::test]
async fn an_issued_refresh_token_is_an_opaque_credential() {
    let (router, engine) = router_and_engine();
    let fixture = pool_with_client(
        &router,
        "opaque-pool",
        &json!({
            "Policies": {
                "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "EMAIL_OTP"] }
            },
        }),
        &json!({
            "RefreshTokenRotation": { "Feature": "ENABLED", "RetryGracePeriodSeconds": 30 },
        }),
    )
    .await;
    let alice_sub = confirmed_user(&router, &fixture.user_pool_id, "alice", &[]).await;
    confirmed_user(
        &router,
        &fixture.user_pool_id,
        "otp",
        &[("email", "otp@example.com"), ("email_verified", "true")],
    )
    .await;
    created_user(
        &router,
        &fixture.user_pool_id,
        "newbie",
        &[],
        Some(TEMPORARY_PASSWORD),
    )
    .await;

    let mut results: Vec<(String, Value)> = Vec::new();

    // Each operation that MINTS a refresh token mints one here.
    for (case, operation, body) in [
        (
            "InitiateAuth with the client-side password flow",
            "InitiateAuth",
            json!({
                "ClientId": fixture.client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "AdminInitiateAuth with the admin password flow",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": fixture.user_pool_id,
                "ClientId": fixture.client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
    ] {
        let payload = ok(&router, operation, &body).await;
        results.push((case.to_string(), payload["AuthenticationResult"].clone()));
    }

    // The two challenge responses, each completing a sign-in of its own.
    let challenged = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "newbie", "PASSWORD": TEMPORARY_PASSWORD },
        }),
    )
    .await;
    let answered = ok(
        &router,
        "AdminRespondToAuthChallenge",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "ChallengeName": NEW_PASSWORD_REQUIRED,
            "Session": challenged["Session"].as_str().expect("a challenge session"),
            "ChallengeResponses": { "USERNAME": "newbie", "NEW_PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    results.push((
        "AdminRespondToAuthChallenge answering the password challenge".to_string(),
        answered["AuthenticationResult"].clone(),
    ));

    let started = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "USER_AUTH",
            "AuthParameters": { "USERNAME": "otp", "PREFERRED_CHALLENGE": EMAIL_OTP },
        }),
    )
    .await;
    let otp_session = started["Session"].as_str().expect("a challenge session");
    let code = session_code(&engine, otp_session);
    let answered = ok(
        &router,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": fixture.client_id,
            "ChallengeName": EMAIL_OTP,
            "Session": otp_session,
            "ChallengeResponses": { "USERNAME": "otp", "EMAIL_OTP_CODE": code },
        }),
    )
    .await;
    results.push((
        "RespondToAuthChallenge answering the one-time password".to_string(),
        answered["AuthenticationResult"].clone(),
    ));

    // A rotation mints one too, for the token exchange.
    let rotating = results[0].1["RefreshToken"]
        .as_str()
        .expect("a sign-in issues a refresh token")
        .to_string();
    let rotated = ok(
        &router,
        "GetTokensFromRefreshToken",
        &json!({ "ClientId": fixture.client_id, "RefreshToken": rotating }),
    )
    .await;
    results.push((
        "GetTokensFromRefreshToken rotating the submitted token".to_string(),
        rotated["AuthenticationResult"].clone(),
    ));

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, result) in &results {
        let refresh = result["RefreshToken"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: this answer must issue a refresh token, got {result}")
        });
        assert!(
            is_modelled_token(refresh),
            "case {case:?}: a refresh token satisfies TokenModelType, got {refresh}"
        );
        assert!(
            readable_document(refresh).is_none(),
            "case {case:?}: a refresh token hands its holder no readable document, \
             got {:?}",
            readable_document(refresh)
        );
        for secret in [
            alice_sub.as_str(),
            fixture.client_id.as_str(),
            fixture.user_pool_id.as_str(),
            "alice",
            "newbie",
        ] {
            assert!(
                !refresh.contains(secret),
                "case {case:?}: a refresh token states nothing of its sign-in, \
                 found {secret:?} in {refresh}"
            );
        }
        assert!(
            minted.insert(refresh.to_string()),
            "case {case:?}: every sign-in mints its own refresh credential, \
             {refresh} repeated"
        );

        // The two tokens beside it ARE readable statements, so this is not a suite that
        // would pass on any three opaque strings.
        for member in ["AccessToken", "IdToken"] {
            let token = result[member].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: this answer must issue {member}, got {result}")
            });
            let claims = readable_document(token).unwrap_or_else(|| {
                panic!("case {case:?} / {member}: a signed token carries claims, got {token}")
            });
            assert!(
                claims["iss"].as_str().is_some_and(|iss| !iss.is_empty()),
                "case {case:?} / {member}: the claims name the issuing pool, got {claims}"
            );
        }
    }

    // An opaque credential is still a credential: the refresh token of the first
    // sign-in exchanges for a fresh pair. This app client ROTATES its refresh
    // tokens, so the renewal is the operation that rotates one — the
    // `InitiateAuth` refresh flow "isn't enabled for the user pool client"
    // (`UnsupportedOperationException` in this service's model) where rotation is
    // switched on.
    let refreshed = ok(
        &router,
        "GetTokensFromRefreshToken",
        &json!({
            "ClientId": fixture.client_id,
            "RefreshToken": results[1].1["RefreshToken"],
        }),
    )
    .await;
    assert!(
        refreshed["AuthenticationResult"]["AccessToken"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "an opaque refresh token still refreshes its sign-in, got {refreshed}"
    );
}

/// RULE: the `DeviceGroupKey` a pool reports in `NewDeviceMetadata` is UUID-shaped, is
/// the same for every device of that pool, and differs between pools — on the admin
/// sign-in and the client-side one alike.
#[tokio::test]
async fn a_new_device_metadata_carries_a_uuid_shaped_device_group_key() {
    // Device remembering is activated by the PRESENCE of `DeviceConfiguration`, so the
    // rows span its property combinations, the empty structure included.
    let configurations = [
        (
            "a pool that only challenges a new device",
            json!({ "ChallengeRequiredOnNewDevice": true }),
        ),
        (
            "a pool that remembers on the user's prompt",
            json!({ "DeviceOnlyRememberedOnUserPrompt": true }),
        ),
        ("a pool that configures no property", json!({})),
    ];

    let (router, _engine) = router_and_engine();
    let mut per_pool: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (case, device_configuration) in &configurations {
        let fixture = pool_with_client(
            &router,
            "device-pool",
            &json!({ "DeviceConfiguration": device_configuration }),
            &json!({}),
        )
        .await;
        confirmed_user(&router, &fixture.user_pool_id, "alice", &[]).await;

        let mut keys: BTreeSet<String> = BTreeSet::new();
        // Both sign-ins report the device of a pool that remembers devices.
        for (operation, body) in [
            (
                "InitiateAuth",
                json!({
                    "ClientId": fixture.client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
                }),
            ),
            (
                "AdminInitiateAuth",
                json!({
                    "UserPoolId": fixture.user_pool_id,
                    "ClientId": fixture.client_id,
                    "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
                }),
            ),
        ] {
            let case = format!("{case} / {operation}");
            let payload = ok(&router, operation, &body).await;
            let metadata = &payload["AuthenticationResult"]["NewDeviceMetadata"];
            let group_key = metadata["DeviceGroupKey"].as_str().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a sign-in from an unrecognized device reports a \
                     DeviceGroupKey, got {payload}"
                )
            });
            assert!(
                is_uuid_shaped(group_key),
                "case {case:?}: expected a UUID-shaped DeviceGroupKey, got {group_key:?}"
            );
            keys.insert(group_key.to_string());
        }
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: the key belongs to the POOL, so every device of it reports \
             the same one, got {keys:?}"
        );
        per_pool.push(((*case).to_string(), keys));
    }

    for (case, keys) in &per_pool {
        for (other_case, other) in &per_pool {
            if case == other_case {
                continue;
            }
            assert!(
                keys.is_disjoint(other),
                "case {case:?}: each pool mints its own device group key, but {other_case:?} \
                 reports {other:?} too"
            );
        }
    }
}
