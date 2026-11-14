// SPDX-License-Identifier: CC0-1.0

//! What ONE user-pool Lambda trigger function can tell apart about the flow that invoked it,
//! which is the `triggerSource` of the event it was handed.
//!
//! RULE under test: a pool's `LambdaConfig` member names "a collection of user pool Lambda
//! triggers. Amazon Cognito invokes triggers at several possible stages of user pool
//! operations. Triggers can modify the outcome of the operations that invoked them"
//! (`LambdaConfigType`, this service's model) — and where ONE member is reached from SEVERAL
//! stages, the event's `triggerSource` is what names the stage that reached it, so a single
//! function serving all of them branches on that member and the operation then answers the
//! caller with what that branch produced. The two members whose stages this suite drives:
//!
//! * `UserMigration` "can create user profiles when users sign in or attempt to reset their
//!   password with credentials that don't exist yet" (`LambdaConfigType$UserMigration`), so a
//!   sign-in of an unknown username is handed `UserMigration_Authentication` and a password
//!   reset of one `UserMigration_ForgotPassword` — and only the sign-in's event carries the
//!   password the stage presented;
//! * the pre-token-generation member is reached by every generation of a sign-in's tokens, so
//!   an initial authentication is handed `TokenGeneration_Authentication`, the refresh of
//!   that same session `TokenGeneration_RefreshTokens` and the new-password challenge that
//!   completes a sign-in `TokenGeneration_NewPasswordChallenge` — and the claims the branch
//!   asked for are the ones the issued tokens carry.
//!
//! So each suite here asserts the rule over the SET of stages one function serves: the
//! distinction is observable to the caller (in the migrated user that reads back, and in the
//! claims of the issued tokens), and every stage reaches a terminal answer rather than
//! leaving the caller unable to tell "trigger not invoked" from "call still pending".
//!
//! Both suites run a REAL deployment package: Cognito and Lambda are mounted on one engine,
//! as the assembled emulator mounts them, and the pool's trigger names a `python3.12`
//! function whose handler reads `event["triggerSource"]` and answers from the branch it takes.
//! Nothing here asserts a value baked into the service: the pool id, the client id, the
//! function ARN and the users all come back from the state the setup calls created.

use std::collections::{BTreeMap, BTreeSet};
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

/// A password satisfying a default pool's password policy, which is the one every sign-in
/// below presents.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The password the new-password challenge chooses, distinct from the one above.
const CHOSEN_PASSWORD: &str = "Ch0sen!Passw0rd";

/// The migrate-user source of a sign-in, as Amazon Cognito documents the trigger's sources.
const MIGRATION_BY_AUTHENTICATION: &str = "UserMigration_Authentication";

/// The migrate-user source of a password reset.
const MIGRATION_BY_FORGOT_PASSWORD: &str = "UserMigration_ForgotPassword";

/// The standard attribute the handler writes the migrate-user branch it took into, so a
/// later read of the migrated user shows which stage created it.
const BRANCH_ATTRIBUTE: &str = "nickname";

/// The standard attribute the handler writes whether the event carried a password into.
const PASSWORD_ATTRIBUTE: &str = "given_name";

/// What the handler writes into [`PASSWORD_ATTRIBUTE`] for an event that carried the
/// password the stage presented.
const WITH_PASSWORD: &str = "with-password";

/// And for one that carried none.
const WITHOUT_PASSWORD: &str = "without-password";

/// The claim the handler writes the token-generation source it was handed into.
const SOURCE_CLAIM: &str = "trigger_source";

/// The claim it writes the event version into beside it.
const VERSION_CLAIM: &str = "trigger_version";

/// The event version the second trigger version asks for: the major number of `V2_0`
/// (`PreTokenGenerationVersionConfigType$LambdaVersion`, this service's model).
const SECOND_EVENT_VERSION: &str = "2";

/// How long a wait for a function to report `Active`, or for the warm-up invocation to
/// answer, is given before it is a failure. Generous: the span is the emulator's to choose
/// and nothing here tests it.
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
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
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

/// The handler the trigger function runs: ONE function serving every stage below, which
/// branches on the `triggerSource` of the event it was handed.
///
/// For a migrate-user event it answers with a profile that RECORDS the branch it took and
/// whether that event carried a password, so a later read of the migrated user shows both.
/// For a token-generation event it asks for claims that record the branch and the event
/// version, so the issued tokens carry both. So every distinction the suites assert is one
/// this Python code drew from `triggerSource` alone.
const HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    source = event["triggerSource"]
    response = event.get("response") or {}
    if source.startswith("UserMigration_"):
        presented = "with-password" if "password" in event["request"] else "without-password"
        response["userAttributes"] = {
            "email": event["userName"] + "@example.test",
            "email_verified": "true",
            "nickname": source,
            "given_name": presented,
        }
        if source == "UserMigration_Authentication":
            response["finalUserStatus"] = "CONFIRMED"
        else:
            response["finalUserStatus"] = "RESET_REQUIRED"
    elif source.startswith("TokenGeneration_"):
        claims = {"trigger_source": source, "trigger_version": event["version"]}
        response["claimsAndScopeOverrideDetails"] = {
            "idTokenGeneration": {"claimsToAddOrOverride": claims},
            "accessTokenGeneration": {"claimsToAddOrOverride": claims},
        }
    event["response"] = response
    return event
"#;

/// Create the trigger function, wait until Lambda reports it `Active`, warm it with one
/// direct invocation, and return the ARN Lambda minted for it.
///
/// The warm-up is also the proof that the package RUNS and BRANCHES: its answer is the
/// profile this handler builds for a migrate-user source, so a suite that later reads a
/// branch out of state or out of a token is reading the return value of code that had
/// already run once.
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
        &json!({
            "triggerSource": MIGRATION_BY_AUTHENTICATION,
            "version": "1",
            "userName": "warm",
            "request": { "password": GOOD_PASSWORD },
            "response": {},
        }),
    )
    .await;
    assert_eq!(
        (
            status,
            answered["response"]["userAttributes"][BRANCH_ATTRIBUTE].clone()
        ),
        (200, json!(MIGRATION_BY_AUTHENTICATION)),
        "the trigger function's own package must run and branch on the source it was handed, \
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

/// An `InitiateAuth` body for the self-service password flow.
fn password_sign_in(client_id: &str, username: &str, password: &str) -> Value {
    json!({
        "ClientId": client_id,
        "AuthFlow": "USER_PASSWORD_AUTH",
        "AuthParameters": { "USERNAME": username, "PASSWORD": password },
    })
}

/// Which stage asks a pool to migrate a username it does not hold.
#[derive(Clone, Copy, Debug)]
enum Migrating {
    /// A self-service password sign-in, by `InitiateAuth`.
    SignIn,
    /// An administrative password sign-in, by `AdminInitiateAuth`.
    AdminSignIn,
    /// A self-service password reset, by `ForgotPassword`.
    Reset,
}

impl Migrating {
    /// The operation this stage calls, and its request body.
    fn request(self, user_pool_id: &str, client_id: &str, username: &str) -> (&'static str, Value) {
        match self {
            Self::SignIn => (
                "InitiateAuth",
                password_sign_in(client_id, username, GOOD_PASSWORD),
            ),
            Self::AdminSignIn => (
                "AdminInitiateAuth",
                json!({
                    "UserPoolId": user_pool_id,
                    "ClientId": client_id,
                    "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
                }),
            ),
            Self::Reset => (
                "ForgotPassword",
                json!({ "ClientId": client_id, "Username": username }),
            ),
        }
    }

    /// Whether this stage presents a password, which is the member the event carries it in:
    /// "Amazon Cognito doesn't send this value in a request for the
    /// `UserMigration_ForgotPassword` trigger source" (the migrate-user event's `password`).
    fn presented(self) -> &'static str {
        match self {
            Self::SignIn | Self::AdminSignIn => WITH_PASSWORD,
            Self::Reset => WITHOUT_PASSWORD,
        }
    }
}

/// The user attributes of an `AdminGetUser` answer, by name.
fn attributes(user: &Value) -> BTreeMap<String, String> {
    user["UserAttributes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|attribute| {
            Some((
                attribute["Name"].as_str()?.to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            ))
        })
        .collect()
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

/// The `kind` token of an `AuthenticationResult`, which every generation below must mint.
fn token<'payload>(case: &str, payload: &'payload Value, member: &str, kind: &str) -> &'payload str {
    payload["AuthenticationResult"][member]
        .as_str()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the generation to mint an {kind} token, got {payload}")
        })
}

#[tokio::test]
async fn one_migrate_user_function_tells_a_sign_in_from_a_password_reset_by_the_trigger_source() {
    let router = router();
    let function = trigger_function(&router, "trigger-source-migrate-user").await;
    let (user_pool_id, client_id) = pool_with_client(
        &router,
        "migration-sources",
        json!({ "UserMigration": function }),
    )
    .await;

    // (case, the stage that asks, the source its event must carry, the status the branch
    // that source takes asks the migrated user be created in)
    let rows: [(&str, Migrating, &str, &str); 3] = [
        (
            "a self-service password sign-in through InitiateAuth",
            Migrating::SignIn,
            MIGRATION_BY_AUTHENTICATION,
            "CONFIRMED",
        ),
        (
            "an administrative password sign-in through AdminInitiateAuth",
            Migrating::AdminSignIn,
            MIGRATION_BY_AUTHENTICATION,
            "CONFIRMED",
        ),
        (
            "a self-service password reset through ForgotPassword",
            Migrating::Reset,
            MIGRATION_BY_FORGOT_PASSWORD,
            "RESET_REQUIRED",
        ),
    ];

    let mut observed: Vec<(&str, String)> = Vec::new();
    for (index, (case, stage, expected_source, expected_status)) in rows.into_iter().enumerate() {
        // One username per row, none of which the pool holds: the function is the pool's
        // authority on whether each exists.
        let username = format!("legacy{index}");
        let (operation, body) = stage.request(&user_pool_id, &client_id, &username);
        let (status, payload) = cognito(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to reach a terminal answer, got {status} with \
             {payload}"
        );
        // The answer the stage itself owes, which is what says the flow completed rather
        // than merely invoking something.
        match stage {
            Migrating::SignIn | Migrating::AdminSignIn => {
                token(case, &payload, "AccessToken", "access");
            }
            Migrating::Reset => {
                let destination = payload["CodeDeliveryDetails"]["Destination"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    !destination.is_empty(),
                    "case {case:?}: expected the migrated reset to report a code delivery, got \
                     {payload}"
                );
            }
        }

        // What the branch left behind, read back through this service's own reader.
        let user = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        let read = attributes(&user);
        let branch = read.get(BRANCH_ATTRIBUTE).cloned().unwrap_or_default();
        assert_eq!(
            branch, expected_source,
            "case {case:?}: expected the function to have branched on {expected_source:?}, got \
             {user}"
        );
        let presented = read.get(PASSWORD_ATTRIBUTE).map(String::as_str);
        assert_eq!(
            presented,
            Some(stage.presented()),
            "case {case:?}: expected the event to carry the presented password only for a \
             sign-in, got {user}"
        );
        assert_eq!(
            user["UserStatus"].as_str(),
            Some(expected_status),
            "case {case:?}: expected the status this branch asked for, got {user}"
        );
        observed.push((case, branch));
    }

    // The whole point of the member: the stages are told apart by it alone, so the sources
    // the function saw across these flows are exactly the two Amazon Cognito documents for
    // this trigger — neither one value for every stage nor a value of another trigger.
    let distinct: BTreeSet<&str> = observed.iter().map(|(_, source)| source.as_str()).collect();
    assert_eq!(
        distinct,
        BTreeSet::from([MIGRATION_BY_AUTHENTICATION, MIGRATION_BY_FORGOT_PASSWORD]),
        "expected the two documented migrate-user sources across these flows, got {observed:?}"
    );
}

#[tokio::test]
async fn one_pre_token_generation_function_tells_a_refresh_from_a_sign_in_by_the_trigger_source() {
    let router = router();
    let function = trigger_function(&router, "trigger-source-pre-token").await;
    let (user_pool_id, client_id) = pool_with_client(
        &router,
        "generation-sources",
        json!({
            "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": function },
        }),
    )
    .await;

    // The user that signs in, with a password of its own.
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "signer",
            "MessageAction": "SUPPRESS",
        }),
    )
    .await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "signer",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    // And the user an administrator created with a TEMPORARY password, which owes a password
    // change: its sign-in answers the challenge whose response mints the last generation
    // below.
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "challenged",
            "MessageAction": "SUPPRESS",
            "TemporaryPassword": GOOD_PASSWORD,
        }),
    )
    .await;

    // The first sign-in, whose refresh token the refreshing generations exchange.
    let signed_in = ok(
        &router,
        "InitiateAuth",
        &password_sign_in(&client_id, "signer", GOOD_PASSWORD),
    )
    .await;
    let refresh_token = signed_in["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first sign-in must mint a refresh token, got {signed_in}"))
        .to_string();

    let challenge = ok(
        &router,
        "InitiateAuth",
        &password_sign_in(&client_id, "challenged", GOOD_PASSWORD),
    )
    .await;
    assert_eq!(
        challenge["ChallengeName"].as_str(),
        Some("NEW_PASSWORD_REQUIRED"),
        "a user that owes a password change must be challenged, got {challenge}"
    );
    let session = challenge["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("the challenge must carry a session, got {challenge}"))
        .to_string();

    // (case, the operation, its body, the source the generation's event must carry). The
    // first row's generation is the sign-in already made above, whose answer is read back
    // rather than made again.
    let rows: [(&str, &str, Value, &str); 4] = [
        (
            "a password sign-in through InitiateAuth",
            "",
            Value::Null,
            "TokenGeneration_Authentication",
        ),
        (
            "a refresh of that same session through InitiateAuth",
            "InitiateAuth",
            json!({
                "ClientId": client_id,
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": refresh_token },
            }),
            "TokenGeneration_RefreshTokens",
        ),
        (
            "a refresh of that same session through GetTokensFromRefreshToken",
            "GetTokensFromRefreshToken",
            json!({ "ClientId": client_id, "RefreshToken": refresh_token }),
            "TokenGeneration_RefreshTokens",
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
            }),
            "TokenGeneration_NewPasswordChallenge",
        ),
    ];

    let mut observed: Vec<(&str, String)> = Vec::new();
    for (case, operation, body, expected_source) in rows {
        let payload = if operation.is_empty() {
            signed_in.clone()
        } else {
            let (status, payload) = cognito(&router, operation, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {operation} to reach a terminal answer, got {status} \
                 with {payload}"
            );
            payload
        };

        // The claims the branch asked for are in BOTH tokens the second contract customises,
        // and the ACCESS token is the one a resource server reads them from.
        for (member, kind) in [("AccessToken", "access"), ("IdToken", "identity")] {
            let decoded = claims(case, kind, token(case, &payload, member, kind));
            assert_eq!(
                decoded.get(SOURCE_CLAIM).and_then(Value::as_str),
                Some(expected_source),
                "case {case:?}: expected the {kind} token to carry the source the function \
                 branched on, got {decoded}"
            );
            assert_eq!(
                decoded.get(VERSION_CLAIM).and_then(Value::as_str),
                Some(SECOND_EVENT_VERSION),
                "case {case:?}: expected the {kind} token to carry the configured trigger \
                 version, got {decoded}"
            );
            if kind == "access" {
                observed.push((case, decoded[SOURCE_CLAIM].as_str().unwrap_or_default().to_string()));
            }
        }
    }

    // A refresh is distinguishable from the authentication it refreshes, and the challenge
    // that completes a sign-in from both: the sources across these generations are exactly
    // the three Amazon Cognito documents for the stages they are.
    let distinct: BTreeSet<&str> = observed.iter().map(|(_, source)| source.as_str()).collect();
    assert_eq!(
        distinct,
        BTreeSet::from([
            "TokenGeneration_Authentication",
            "TokenGeneration_NewPasswordChallenge",
            "TokenGeneration_RefreshTokens",
        ]),
        "expected one documented source per generation across these flows, got {observed:?}"
    );
}
