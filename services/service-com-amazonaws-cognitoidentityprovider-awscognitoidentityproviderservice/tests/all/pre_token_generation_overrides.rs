// SPDX-License-Identifier: CC0-1.0

//! What a pool's pre-token-generation Lambda trigger changes about the tokens the sign-in
//! that fired it answers with.
//!
//! The rule under test: a trigger "can modify the outcome of the operations that invoked
//! them" (`LambdaConfigType`, this service's model), and the outcome a pre-token-generation
//! trigger precedes is the token triple itself — so the claims, the scopes and the group
//! configuration the function answered with are the ones the issued tokens carry, not
//! decoration the minting may ignore. The answer states them under the member of the
//! trigger contract it was written for: `response.claimsOverrideDetails` customises the
//! identity token, and `response.claimsAndScopeOverrideDetails` customises both tokens
//! (`idTokenGeneration`, `accessTokenGeneration`) and the access token's scopes. A
//! `groupOverrideDetails` REPLACES the group configuration the event reported — the groups,
//! the roles and the preferred role — while a claim Amazon Cognito reserves is never the
//! function's to rewrite.
//!
//! Both suites run a REAL deployment package: Cognito and Lambda are mounted on one engine,
//! as the assembled emulator mounts them, and the pool's trigger names a `python3.12`
//! function whose handler answers with the override document the call's `ClientMetadata`
//! asked it for. So the override every assertion reads back out of a decoded token travelled
//! out to the function as an event and came back as that function's own return value.
//!
//! The first suite is the table of contracts and members: one row per thing the rule
//! generalises over — which contract member the answer uses, which token it customises,
//! whether the groups are replaced, emptied or left alone, whether a scope is added or
//! suppressed, and a reserved claim the answer tries to write. The second is the set of
//! operations: every operation of this service that mints a sign-in's tokens applies what
//! the trigger answered for ITS generation.
//!
//! Nothing here asserts a value baked into the service: the pool id, the client id, the
//! function ARN, the stored group and the user all come back from the state the setup calls
//! created.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The local name of Cognito's Smithy shape, which is the `X-Amz-Target` prefix.
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// Where Lambda's `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying Lambda's `Role` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the trigger function's package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The password the `NEW_PASSWORD_REQUIRED` row chooses.
const CHOSEN_PASSWORD: &str = "Ch0sen!Passw0rd";

/// The group every user below is a member of, which is what the tokens would carry if the
/// trigger changed nothing.
const STORED_GROUP: &str = "stored-group";

/// The role that group grants, which is what the identity token's `cognito:roles` and
/// `cognito:preferred_role` would carry unchanged.
const STORED_ROLE: &str = "arn:aws:iam::000000000000:role/stored-group";

/// Another role, for the row whose answer replaces the group configuration wholesale.
const REPLACEMENT_ROLE: &str = "arn:aws:iam::000000000000:role/replacement";

/// The one scope an access token of a sign-in is granted, which the rows that change the
/// access token's scopes add to or take out of.
const GRANTED_SCOPE: &str = "aws.cognito.signin.user.admin";

/// How long a wait for a function to report `Active`, or for the warm-up invocation to
/// answer, is given before it is a failure. Generous: the span is the emulator's to choose
/// and no row here tests it.
const BUDGET: Duration = Duration::from_secs(60);

/// How long a poll waits before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Cognito and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts them, so
/// the trigger dispatch's in-process invocation reaches this very Lambda.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Cognito call: `awsJson1_1`, addressed by `X-Amz-Target`.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{COGNITO_TARGET_PREFIX}.{operation}"),
        )
        .header("authorization", authorization("cognito-idp"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    answer(router.dispatch(request).await)
}

/// One REST call to Lambda.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("the test request is well formed");
    answer(router.dispatch(request).await)
}

/// The status and JSON body of a dispatched response.
fn answer(response: http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A Cognito call that must succeed, with its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = cognito(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// A .zip deployment package holding `source` as the module the handler names.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The handler the trigger function runs: it answers with the override document the call's
/// `ClientMetadata` asked for, under the `response` half of the event it was handed.
///
/// So every override a row asserts is one this Python code returned: the test states what to
/// override in the call it makes, and the function is what puts it in the answer.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    metadata = event["request"].get("clientMetadata") or {}
    asked = metadata.get("overrides")
    if asked:
        event["response"] = json.loads(asked)
    return event
"#;

/// Create the trigger function, wait until Lambda reports it `Active`, warm it with one
/// direct invocation, and return the ARN Lambda minted for it.
///
/// The warm-up is also the proof that the package RUNS: its answer is the document this
/// handler builds, so a row that later reads an override out of a token is reading the
/// return value of code that had already run once.
async fn trigger_function(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction must answer 201, got {status} with {created}"
    );
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction must mint an ARN, got {created}"))
        .to_string();

    // An invocation only reaches a function Lambda reports as active, so the condition is
    // waited FOR rather than waited out.
    let deadline = Instant::now() + BUDGET;
    loop {
        let (_, read) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        if read["State"].as_str() == Some("Active") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the trigger function {name:?} never became Active: {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }

    let (status, answered) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &json!({ "request": { "clientMetadata": { "overrides": "{\"warm\": true}" } } }),
    )
    .await;
    assert_eq!(
        (status, answered["response"].clone()),
        (200, json!({ "warm": true })),
        "the trigger function's own package must run and answer with what it was asked for, \
         got {status} with {answered}"
    );
    arn
}

/// Create a pool whose `LambdaConfig` is `lambda_config`, with an app client admitting every
/// sign-in flow these suites drive, and return their minted ids.
async fn pool_with_client(
    router: &MultiServiceRouter,
    name: &str,
    lambda_config: Value,
) -> (String, String) {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": name,
            // The tier a pool that configures the second trigger version runs on.
            "UserPoolTier": "ESSENTIALS",
            "LambdaConfig": lambda_config,
        }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{name}-app"),
            "ExplicitAuthFlows": [
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
            ],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {client}"))
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with a permanent password and put it in the pool's stored group, which
/// is the group configuration a trigger that overrides nothing leaves in the tokens.
async fn grouped_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "MessageAction": "SUPPRESS",
            "UserAttributes": [{ "Name": "email", "Value": format!("{username}@example.test") }],
        }),
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
    ok(
        router,
        "CreateGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "GroupName": STORED_GROUP,
            "RoleArn": STORED_ROLE,
            "Precedence": 1,
        }),
    )
    .await;
    ok(
        router,
        "AdminAddUserToGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "GroupName": STORED_GROUP,
        }),
    )
    .await;
}

/// The `ClientMetadata` that asks the handler to answer with `response`.
fn asking_for(response: &Value) -> Value {
    json!({ "overrides": response.to_string() })
}

/// The decoded claims of a JWT this service issued: the payload segment, read the way every
/// verifier of a Cognito token reads it.
fn claims(case: &str, kind: &str, token: &str) -> Value {
    let payload = token
        .split('.')
        .nth(1)
        .unwrap_or_else(|| panic!("case {case:?}: the {kind} token must have a payload segment"));
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the {kind} token's payload must be base64url: {error}")
        });
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!("case {case:?}: the {kind} token's payload must be JSON: {error}")
    })
}

/// What one claim of a minted token must be.
#[derive(Debug, Clone)]
enum Expect {
    /// The claim carries exactly this value.
    Is(Value),
    /// The claim is not in the token at all.
    Absent,
    /// The claim is in the token, whatever the minting computed for it.
    Present,
    /// The claim is in the token and is NOT this value — what a reserved claim an answer
    /// tried to write must be.
    Other(Value),
}

/// Assert the `kind` token's `claims` against what the row expects of them.
fn assert_claims(case: &str, kind: &str, claims: &Value, expected: &[(&str, Expect)]) {
    for (claim, expect) in expected {
        let got = claims.get(*claim).cloned();
        match expect {
            Expect::Is(want) => assert_eq!(
                got.as_ref(),
                Some(want),
                "case {case:?}: expected the {kind} token's {claim} to be {want}, got {got:?}"
            ),
            Expect::Absent => assert!(
                got.is_none(),
                "case {case:?}: expected the {kind} token to carry no {claim}, got {got:?}"
            ),
            Expect::Present => assert!(
                got.is_some(),
                "case {case:?}: expected the {kind} token to carry {claim}, got nothing"
            ),
            Expect::Other(refused) => {
                assert!(
                    got.is_some(),
                    "case {case:?}: expected the {kind} token to carry {claim}, got nothing"
                );
                assert_ne!(
                    got.as_ref(),
                    Some(refused),
                    "case {case:?}: expected the {kind} token's {claim} not to be the value the \
                     answer asked for, got {got:?}"
                );
            }
        }
    }
}

/// One row of the contract table.
struct Row {
    case: &'static str,
    /// The `LambdaConfig` the pool configures the trigger with, built around the function's
    /// ARN.
    lambda_config: Value,
    /// The `response` half the function answers with.
    answered: Value,
    /// What the identity token this generation mints must carry.
    id_token: Vec<(&'static str, Expect)>,
    /// What the access token beside it must carry.
    access_token: Vec<(&'static str, Expect)>,
}

#[tokio::test]
async fn a_pre_token_generation_answer_decides_the_minted_tokens_claims() {
    let router = router();
    let function = trigger_function(&router, "pre-token-overrides").await;

    let rows: Vec<Row> = vec![
        Row {
            case: "the second contract overriding the groups",
            lambda_config: json!({
                "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": function },
            }),
            answered: json!({
                "claimsAndScopeOverrideDetails": {
                    "groupOverrideDetails": {
                        "groupsToOverride": ["override-one", "override-two"],
                    },
                },
            }),
            id_token: vec![
                (
                    "cognito:groups",
                    Expect::Is(json!(["override-one", "override-two"])),
                ),
                // The replacement names no role, so the claims the stored group's role put
                // in the token go with it.
                ("cognito:roles", Expect::Absent),
                ("cognito:preferred_role", Expect::Absent),
            ],
            access_token: vec![(
                "cognito:groups",
                Expect::Is(json!(["override-one", "override-two"])),
            )],
        },
        Row {
            case: "the second contract replacing the whole group configuration",
            lambda_config: json!({
                "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": function },
            }),
            answered: json!({
                "claimsAndScopeOverrideDetails": {
                    "groupOverrideDetails": {
                        "groupsToOverride": ["replacement"],
                        "iamRolesToOverride": [REPLACEMENT_ROLE],
                        "preferredRole": REPLACEMENT_ROLE,
                    },
                },
            }),
            id_token: vec![
                ("cognito:groups", Expect::Is(json!(["replacement"]))),
                ("cognito:roles", Expect::Is(json!([REPLACEMENT_ROLE]))),
                (
                    "cognito:preferred_role",
                    Expect::Is(json!(REPLACEMENT_ROLE)),
                ),
            ],
            access_token: vec![("cognito:groups", Expect::Is(json!(["replacement"])))],
        },
        Row {
            case: "the second contract answering an empty group object, which suppresses them",
            lambda_config: json!({
                "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": function },
            }),
            answered: json!({
                "claimsAndScopeOverrideDetails": { "groupOverrideDetails": {} },
            }),
            id_token: vec![
                ("cognito:groups", Expect::Absent),
                ("cognito:roles", Expect::Absent),
                ("cognito:preferred_role", Expect::Absent),
            ],
            access_token: vec![("cognito:groups", Expect::Absent)],
        },
        Row {
            case: "the second contract customising each token's own claims and the scopes",
            lambda_config: json!({
                "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": function },
            }),
            answered: json!({
                "claimsAndScopeOverrideDetails": {
                    "idTokenGeneration": {
                        "claimsToAddOrOverride": { "tenant": "acme", "seats": 3 },
                        "claimsToSuppress": ["email"],
                    },
                    "accessTokenGeneration": {
                        "claimsToAddOrOverride": { "tier": "gold" },
                        "scopesToAdd": ["store/read"],
                        "scopesToSuppress": [GRANTED_SCOPE],
                    },
                },
            }),
            id_token: vec![
                ("tenant", Expect::Is(json!("acme"))),
                ("seats", Expect::Is(json!(3))),
                ("email", Expect::Absent),
                // The answer named no group member, so the user's stored membership stands.
                ("cognito:groups", Expect::Is(json!([STORED_GROUP]))),
                // What the answer asked of the OTHER token is not in this one.
                ("tier", Expect::Absent),
            ],
            access_token: vec![
                ("tier", Expect::Is(json!("gold"))),
                ("scope", Expect::Is(json!("store/read"))),
                ("tenant", Expect::Absent),
                ("cognito:groups", Expect::Is(json!([STORED_GROUP]))),
            ],
        },
        Row {
            case: "the first contract, which customises the identity token alone",
            lambda_config: json!({
                "PreTokenGenerationConfig": { "LambdaVersion": "V1_0", "LambdaArn": function },
            }),
            answered: json!({
                "claimsOverrideDetails": {
                    "claimsToAddOrOverride": { "tenant": "legacy" },
                    "claimsToSuppress": ["email"],
                    "groupOverrideDetails": { "groupsToOverride": ["legacy-group"] },
                },
            }),
            id_token: vec![
                ("tenant", Expect::Is(json!("legacy"))),
                ("email", Expect::Absent),
                ("cognito:groups", Expect::Is(json!(["legacy-group"]))),
            ],
            access_token: vec![
                ("tenant", Expect::Absent),
                ("cognito:groups", Expect::Is(json!(["legacy-group"]))),
                ("scope", Expect::Is(json!(GRANTED_SCOPE))),
            ],
        },
        Row {
            case: "the legacy trigger member, whose answer names reserved claims",
            lambda_config: json!({ "PreTokenGeneration": function }),
            answered: json!({
                "claimsOverrideDetails": {
                    "claimsToAddOrOverride": {
                        "sub": "not-this-users-id",
                        "cognito:groups": ["sneaked-in"],
                        "nickname": "kept",
                    },
                    "claimsToSuppress": ["exp", "aud", "token_use"],
                },
            }),
            id_token: vec![
                ("sub", Expect::Other(json!("not-this-users-id"))),
                ("cognito:groups", Expect::Is(json!([STORED_GROUP]))),
                ("exp", Expect::Present),
                ("aud", Expect::Present),
                ("token_use", Expect::Is(json!("id"))),
                // A claim that is NOT reserved, in the same answer, is still applied.
                ("nickname", Expect::Is(json!("kept"))),
            ],
            access_token: vec![("token_use", Expect::Is(json!("access")))],
        },
        Row {
            case: "the second contract suppressing every scope the grant authorized",
            lambda_config: json!({
                "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": function },
            }),
            answered: json!({
                "claimsAndScopeOverrideDetails": {
                    "accessTokenGeneration": { "scopesToSuppress": [GRANTED_SCOPE] },
                },
            }),
            id_token: vec![("cognito:groups", Expect::Is(json!([STORED_GROUP])))],
            // Authorized for no scope at all, so the claim that states the scopes it IS
            // authorized for is not there.
            access_token: vec![("scope", Expect::Absent)],
        },
        Row {
            case: "a function that answers with no override at all",
            lambda_config: json!({
                "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": function },
            }),
            answered: json!({}),
            id_token: vec![
                ("cognito:groups", Expect::Is(json!([STORED_GROUP]))),
                ("cognito:roles", Expect::Is(json!([STORED_ROLE]))),
                ("cognito:preferred_role", Expect::Is(json!(STORED_ROLE))),
                ("email", Expect::Present),
            ],
            access_token: vec![
                ("cognito:groups", Expect::Is(json!([STORED_GROUP]))),
                ("scope", Expect::Is(json!(GRANTED_SCOPE))),
            ],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let (user_pool_id, client_id) =
            pool_with_client(&router, &format!("overridden{index}"), row.lambda_config).await;
        let username = format!("signer{index}");
        grouped_user(&router, &user_pool_id, &username).await;

        let (status, payload) = cognito(
            &router,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
                "ClientMetadata": asking_for(&row.answered),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the sign-in must issue tokens, got {status} with {payload}",
            row.case
        );
        let result = &payload["AuthenticationResult"];
        let id_token = result["IdToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected an IdToken, got {payload}", row.case));
        let access_token = result["AccessToken"].as_str().unwrap_or_else(|| {
            panic!("case {:?}: expected an AccessToken, got {payload}", row.case)
        });

        assert_claims(
            row.case,
            "identity",
            &claims(row.case, "identity", id_token),
            &row.id_token,
        );
        assert_claims(
            row.case,
            "access",
            &claims(row.case, "access", access_token),
            &row.access_token,
        );
    }
}

#[tokio::test]
async fn every_token_minting_operation_applies_what_its_trigger_answered() {
    let router = router();
    let function = trigger_function(&router, "pre-token-every-generation").await;
    let (user_pool_id, client_id) = pool_with_client(
        &router,
        "every-generation",
        json!({
            "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": function },
        }),
    )
    .await;
    grouped_user(&router, &user_pool_id, "refreshed").await;

    // A user an administrator created with a temporary password owes a password change, so
    // its sign-in answers the `NEW_PASSWORD_REQUIRED` challenge whose response mints the
    // tokens of the last generation below.
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "challenged",
            "TemporaryPassword": GOOD_PASSWORD,
        }),
    )
    .await;

    /// The group list one generation's answer overrides with: a list of its own, so the
    /// token a generation answers with can only carry the override of ITS own invocation.
    fn overriding(group: &str) -> Value {
        json!({
            "claimsAndScopeOverrideDetails": {
                "groupOverrideDetails": { "groupsToOverride": [group] },
            },
        })
    }

    // A first sign-in, through the self-service password flow, hands back the refresh token
    // the two refreshing generations exchange — and is itself the first generation asserted.
    let signed_in = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "refreshed", "PASSWORD": GOOD_PASSWORD },
            "ClientMetadata": asking_for(&overriding("by-initiate-auth")),
        }),
    )
    .await;
    let refresh_token = signed_in["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first sign-in must mint a refresh token, got {signed_in}"))
        .to_string();
    let challenged = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "challenged", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let session = challenged["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("the challenge must carry a session, got {challenged}"))
        .to_string();

    // (case, the operation, its body, the group its answer overrides with)
    let calls = [
        (
            "a self-service password sign-in through InitiateAuth",
            "",
            Value::Null,
            "by-initiate-auth",
        ),
        (
            "an administrative sign-in through AdminInitiateAuth",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "refreshed", "PASSWORD": GOOD_PASSWORD },
                "ClientMetadata": asking_for(&overriding("by-admin-initiate-auth")),
            }),
            "by-admin-initiate-auth",
        ),
        (
            "a refresh through InitiateAuth",
            "InitiateAuth",
            json!({
                "ClientId": client_id,
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": refresh_token },
                "ClientMetadata": asking_for(&overriding("by-refresh-token-auth")),
            }),
            "by-refresh-token-auth",
        ),
        (
            "a refresh through GetTokensFromRefreshToken",
            "GetTokensFromRefreshToken",
            json!({
                "ClientId": client_id,
                "RefreshToken": refresh_token,
                "ClientMetadata": asking_for(&overriding("by-get-tokens")),
            }),
            "by-get-tokens",
        ),
        (
            "a new-password challenge through AdminRespondToAuthChallenge",
            "AdminRespondToAuthChallenge",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "ChallengeName": "NEW_PASSWORD_REQUIRED",
                "Session": session,
                "ChallengeResponses": {
                    "USERNAME": "challenged",
                    "NEW_PASSWORD": CHOSEN_PASSWORD,
                },
                "ClientMetadata": asking_for(&overriding("by-challenge-response")),
            }),
            "by-challenge-response",
        ),
    ];

    for (case, operation, body, group) in calls {
        // The first row's generation is the sign-in already made above, whose answer is
        // read back rather than made again: its refresh token is what the rows after it
        // exchange.
        let payload = if operation.is_empty() {
            signed_in.clone()
        } else {
            let (status, payload) = cognito(&router, operation, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the generation must issue tokens, got {status} with {payload}"
            );
            payload
        };
        let id_token = payload["AuthenticationResult"]["IdToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an IdToken, got {payload}"));
        assert_claims(
            case,
            "identity",
            &claims(case, "identity", id_token),
            &[("cognito:groups", Expect::Is(json!([group])))],
        );
    }

    // The OTHER minting path of GetTokensFromRefreshToken: on an app client whose refresh
    // token rotates, the refresh mints a fresh refresh token beside the pair — "this
    // operation issues a new refresh token and invalidates the original refresh token ...
    // when refresh token rotation is enabled" — and that path applies the same answer.
    let case = "a rotating refresh through GetTokensFromRefreshToken";
    let rotating = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "rotating-app",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
            "RefreshTokenRotation": { "Feature": "ENABLED", "RetryGracePeriodSeconds": 0 },
        }),
    )
    .await;
    let rotating_client = rotating["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a client id, got {rotating}"))
        .to_string();
    let first = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": rotating_client,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "refreshed", "PASSWORD": GOOD_PASSWORD },
            "ClientMetadata": asking_for(&overriding("by-rotating-sign-in")),
        }),
    )
    .await;
    let rotating_refresh = first["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a refresh token, got {first}"))
        .to_string();
    let rotated = ok(
        &router,
        "GetTokensFromRefreshToken",
        &json!({
            "ClientId": rotating_client,
            "RefreshToken": rotating_refresh,
            "ClientMetadata": asking_for(&overriding("by-rotation")),
        }),
    )
    .await;
    let minted = rotated["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !minted.is_empty() && minted != rotating_refresh,
        "case {case:?}: a rotating refresh must mint a refresh token of its own, got {rotated}"
    );
    let id_token = rotated["AuthenticationResult"]["IdToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an IdToken, got {rotated}"));
    assert_claims(
        case,
        "identity",
        &claims(case, "identity", id_token),
        &[("cognito:groups", Expect::Is(json!(["by-rotation"])))],
    );
}
