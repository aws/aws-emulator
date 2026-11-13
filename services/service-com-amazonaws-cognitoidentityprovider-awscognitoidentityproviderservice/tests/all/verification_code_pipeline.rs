// SPDX-License-Identifier: CC0-1.0

//! The verification-code pipeline over the wire: the operations that SEND a code and
//! the operations that CONSUME one.
//!
//! The rule under test: an operation Amazon Cognito documents as sending a code mints
//! one, records it as the single code outstanding for that user and purpose — so a
//! later send supersedes the earlier code — and reports the destination it went to
//! masked, with the medium that destination is reached over; an operation documented
//! as consuming a code accepts ONLY the outstanding code, refuses any other value
//! presented against a live code with `CodeMismatchException` and every check made
//! where no live code is outstanding with `ExpiredCodeException`, and spends the code
//! it accepts so it cannot be used twice.
//!
//! The code itself is delivered out of band, so no response carries it: a test learns
//! the delivered code the way a mailbox would — from the emulator's own state.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// Another password satisfying it, for a reset to settle on.
const RESET_PASSWORD: &str = "Res3tPassw0rd!";

/// A password a default pool's policy refuses (too short, no symbol).
const WEAK_PASSWORD: &str = "short";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service
    /// operations are called.
    Anonymous,
}

/// Which code one table row presents to the operation that consumes it.
#[derive(Clone, Copy, Debug)]
enum Presented {
    /// A value of the right shape that the service never sent.
    Wrong,
    /// The code the FIRST send delivered, which a later send superseded.
    Superseded,
    /// The code outstanding now.
    Outstanding,
}

/// The shared engine every router in one test mounts.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`.
fn router(engine: &Arc<engine::InMemoryEngine>, authenticating: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(authenticating);
    router
}

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return the
/// status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    addressing: Addressing,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = match addressing {
        Addressing::Signed => ENDPOINT_HOST,
        Addressing::Anonymous => EMULATOR_HOST,
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if matches!(addressing, Addressing::Signed) {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// Call `operation` signed, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Signed, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Call `operation` unsigned, requiring success, and return its response body.
async fn ok_anonymous(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Anonymous, operation, body).await;
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

/// `base` with every member of `extra` added, so one helper can create pools whose
/// configuration differs row by row.
fn merged(base: Value, extra: &Value) -> Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    if let Some(extra) = extra.as_object() {
        for (name, value) in extra {
            object.insert(name.clone(), value.clone());
        }
    }
    Value::Object(object)
}

/// Create a pool with `extra_pool` merged into its configuration, plus an app client
/// that admits every flow these tests sign in with, and return both minted ids.
async fn pool_with_client(
    router: &MultiServiceRouter,
    pool_name: &str,
    extra_pool: &Value,
) -> (String, String) {
    let created = ok(
        router,
        "CreateUserPool",
        &merged(json!({ "PoolName": pool_name }), extra_pool),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "app",
            "ExplicitAuthFlows": [
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_USER_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
            ],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with `attributes` and a permanent password, which is what makes
/// it a `CONFIRMED` user an access token can be issued for.
async fn admin_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) {
    let attributes: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": attributes,
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
}

/// Sign `username` in with `password` through the admin password flow and return the
/// access token of that sign-in.
async fn access_token(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
    password: &str,
) -> String {
    let signed_in = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": password },
        }),
    )
    .await;
    signed_in["AuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("a sign-in issues an access token")
        .to_string()
}

/// The code outstanding for `purpose`, read out of the emulator's own state: the real
/// service delivers a code out of band, so no response carries it.
fn outstanding_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    purpose: &state::CodePurpose,
) -> Option<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(purpose))
        .map(|code| code.code.clone())
}

/// Backdate the code outstanding for `purpose` by `age`, so the validity window is
/// read against the clock rather than waited out.
fn backdate_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    purpose: &state::CodePurpose,
    age: Duration,
) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let code = guard
        .pool_mut(user_pool_id)
        .and_then(|pool| pool.user_mut(username))
        .and_then(|user| user.outstanding_codes.get_mut(purpose))
        .expect("a code is outstanding for this purpose");
    code.issued_at = SystemTime::now() - age;
}

/// The one-time password a challenge session delivered, read out of the emulator's
/// own state for the same reason.
fn session_code(engine: &Arc<engine::InMemoryEngine>, session: &str) -> Option<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .auth_sessions
        .get(session)
        .and_then(|stored| stored.code.as_ref())
        .map(|code| code.code.clone())
}

/// Close the validity window of the challenge session `session` names.
fn expire_session(engine: &Arc<engine::InMemoryEngine>, session: &str) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let stored = guard
        .auth_sessions
        .get_mut(session)
        .expect("the challenge session is stored");
    stored.expires_at = SystemTime::now() - Duration::from_secs(1);
}

/// A code of the same shape the service did NOT send: every digit of `code` shifted
/// by one, so no character of it matches.
fn wrong_code(code: &str) -> String {
    code.chars()
        .map(|digit| {
            char::from_digit((digit.to_digit(10).unwrap_or(0) + 1) % 10, 10).unwrap_or('0')
        })
        .collect()
}

/// The attributes `AdminGetUser` reports for `username`, as `(name, value)` pairs.
async fn stored_attributes(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> Vec<(String, String)> {
    let read_back = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    read_back["UserAttributes"]
        .as_array()
        .expect("AdminGetUser reports the stored attributes")
        .iter()
        .map(|attribute| {
            (
                attribute["Name"].as_str().unwrap_or_default().to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `UserStatus` `AdminGetUser` reports for `username`.
async fn stored_status(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> String {
    let read_back = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    read_back["UserStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert that `details` reports a delivery of `attribute` to `destination`.
fn assert_delivery(case: &str, details: &Value, attribute: &str, destination: &str) {
    let medium = state::delivery_medium(attribute).unwrap_or_default();
    assert_eq!(
        details["DeliveryMedium"],
        json!(medium),
        "case {case:?}: expected DeliveryMedium {medium}, got {details}"
    );
    assert_eq!(
        details["AttributeName"],
        json!(attribute),
        "case {case:?}: expected AttributeName {attribute}, got {details}"
    );
    assert_eq!(
        details["Destination"],
        json!(state::masked_destination(destination)),
        "case {case:?}: expected the stored value reported masked, got {details}"
    );
}

/// RULE: `SignUp` sends the confirmation code, `ResendConfirmationCode` SUPERSEDES it,
/// and `ConfirmSignUp` accepts only the code outstanding now — once. Rows differ along
/// which code is presented.
#[tokio::test]
async fn a_sign_up_confirmation_code_is_single_use_and_a_resend_supersedes_it() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) = pool_with_client(
        &setup,
        "sign-up-codes",
        &json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    let purpose = state::CodePurpose::SignUpConfirmation;

    // (case, which code is presented, expected status, expected error code)
    let rows: Vec<(&str, Presented, u16, &str)> = vec![
        (
            "a value of the right shape the service never sent",
            Presented::Wrong,
            400,
            "CodeMismatchException",
        ),
        (
            "the code the sign-up sent, after a resend superseded it",
            Presented::Superseded,
            400,
            "CodeMismatchException",
        ),
        (
            "the code outstanding now",
            Presented::Outstanding,
            200,
            "",
        ),
    ];

    for (index, (case, presented, expected_status, expected_error)) in rows.iter().enumerate() {
        let username = format!("signup{index}");
        let address = format!("{username}@example.com");
        // Registering the user sends the first code, to the attribute the POOL
        // auto-verifies.
        let signed_up = ok_anonymous(
            &emulator,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "Password": GOOD_PASSWORD,
                "UserAttributes": [{ "Name": "email", "Value": address }],
            }),
        )
        .await;
        assert_delivery(case, &signed_up["CodeDeliveryDetails"], "email", &address);
        let first = outstanding_code(&engine, &user_pool_id, &username, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the sign-up must leave a code outstanding"));

        // Resending supersedes it: a second code for the same purpose replaces the
        // first, and the delivery reported is the same stored destination.
        let resent = ok_anonymous(
            &emulator,
            "ResendConfirmationCode",
            &json!({ "ClientId": client_id, "Username": username }),
        )
        .await;
        assert_delivery(case, &resent["CodeDeliveryDetails"], "email", &address);
        let outstanding = outstanding_code(&engine, &user_pool_id, &username, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the resend must leave a code outstanding"));
        assert_ne!(
            outstanding, first,
            "case {case:?}: a resend must mint a fresh code, got {outstanding:?} twice"
        );

        let code = match presented {
            Presented::Wrong => wrong_code(&outstanding),
            Presented::Superseded => first.clone(),
            Presented::Outstanding => outstanding.clone(),
        };
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ConfirmSignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "ConfirmationCode": code,
            }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        if !expected_error.is_empty() {
            assert_eq!(
                error_code(&payload),
                *expected_error,
                "case {case:?}: expected error code {expected_error}, got {payload}"
            );
        }

        // A refused confirmation changes nothing; an accepted one confirms the account
        // AND the attribute the code was sent to.
        let status_now = stored_status(&setup, &user_pool_id, &username).await;
        let attributes = stored_attributes(&setup, &user_pool_id, &username).await;
        let confirmed = matches!(presented, Presented::Outstanding);
        assert_eq!(
            status_now,
            if confirmed { "CONFIRMED" } else { "UNCONFIRMED" },
            "case {case:?}: expected the stored status to follow the outcome, got {status_now}"
        );
        assert_eq!(
            attributes.contains(&("email_verified".to_string(), "true".to_string())),
            confirmed,
            "case {case:?}: the verified flag follows the outcome, got {attributes:?}"
        );

        if confirmed {
            // The code is spent: presenting it again finds nothing outstanding.
            let (replay_status, replay) = dispatch(
                &emulator,
                Addressing::Anonymous,
                "ConfirmSignUp",
                &json!({
                    "ClientId": client_id,
                    "Username": username,
                    "ConfirmationCode": code,
                }),
            )
            .await;
            assert_eq!(
                replay_status, 400,
                "case {case:?}: a confirmed account refuses a second confirmation, got \
                 {replay_status} with {replay}"
            );
            assert_eq!(
                error_code(&replay),
                "NotAuthorizedException",
                "case {case:?}: expected NotAuthorizedException, got {replay}"
            );
            assert!(
                outstanding_code(&engine, &user_pool_id, &username, &purpose).is_none(),
                "case {case:?}: an accepted code is spent, so none stays outstanding"
            );
        }
    }
}

/// RULE: `ForgotPassword` sends the reset code, a second call SUPERSEDES it, and
/// `ConfirmForgotPassword` accepts only the code outstanding now — once. Rows differ
/// along which code is presented.
#[tokio::test]
async fn a_password_reset_code_is_single_use_and_a_reissue_supersedes_it() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) = pool_with_client(&setup, "reset-codes", &json!({})).await;
    let purpose = state::CodePurpose::PasswordReset;

    // (case, which code is presented, expected status, expected error code)
    let rows: Vec<(&str, Presented, u16, &str)> = vec![
        (
            "a value of the right shape the service never sent",
            Presented::Wrong,
            400,
            "CodeMismatchException",
        ),
        (
            "the code the first request sent, after a reissue superseded it",
            Presented::Superseded,
            400,
            "CodeMismatchException",
        ),
        (
            "the code outstanding now",
            Presented::Outstanding,
            200,
            "",
        ),
    ];

    for (index, (case, presented, expected_status, expected_error)) in rows.iter().enumerate() {
        let username = format!("reset{index}");
        let address = format!("{username}@example.net");
        // A reset message goes to a VERIFIED address, so the user is created with one.
        admin_user(
            &setup,
            &user_pool_id,
            &username,
            &[("email", &address), ("email_verified", "true")],
        )
        .await;

        let first_sent = ok_anonymous(
            &emulator,
            "ForgotPassword",
            &json!({ "ClientId": client_id, "Username": username }),
        )
        .await;
        assert_delivery(case, &first_sent["CodeDeliveryDetails"], "email", &address);
        let first = outstanding_code(&engine, &user_pool_id, &username, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the request must leave a code outstanding"));

        ok_anonymous(
            &emulator,
            "ForgotPassword",
            &json!({ "ClientId": client_id, "Username": username }),
        )
        .await;
        let outstanding = outstanding_code(&engine, &user_pool_id, &username, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the reissue must leave a code outstanding"));
        assert_ne!(
            outstanding, first,
            "case {case:?}: a reissue must mint a fresh code, got {outstanding:?} twice"
        );

        let code = match presented {
            Presented::Wrong => wrong_code(&outstanding),
            Presented::Superseded => first.clone(),
            Presented::Outstanding => outstanding.clone(),
        };
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ConfirmForgotPassword",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "ConfirmationCode": code,
                "Password": RESET_PASSWORD,
            }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        if !expected_error.is_empty() {
            assert_eq!(
                error_code(&payload),
                *expected_error,
                "case {case:?}: expected error code {expected_error}, got {payload}"
            );
        }

        // ROUND TRIP: only an accepted reset changes the password the pool stores, read
        // back by signing in with it.
        let accepted = matches!(presented, Presented::Outstanding);
        let expected_password = if accepted { RESET_PASSWORD } else { GOOD_PASSWORD };
        let token = access_token(
            &setup,
            &user_pool_id,
            &client_id,
            &username,
            expected_password,
        )
        .await;
        assert!(
            !token.is_empty(),
            "case {case:?}: the stored password must be {expected_password:?} after the reset"
        );

        if accepted {
            // The code is spent, so no live code is left to compare against: the
            // refusal is the one that asks for a code to be requested.
            let (replay_status, replay) = dispatch(
                &emulator,
                Addressing::Anonymous,
                "ConfirmForgotPassword",
                &json!({
                    "ClientId": client_id,
                    "Username": username,
                    "ConfirmationCode": code,
                    "Password": GOOD_PASSWORD,
                }),
            )
            .await;
            assert_eq!(
                error_code(&replay),
                "ExpiredCodeException",
                "case {case:?}: a spent code is refused, got {replay_status} with {replay}"
            );
        }
    }

    // A code the pool accepts but a password its policy refuses leaves the reset
    // OUTSTANDING: the refusal wrote nothing, so the same code still completes it.
    let username = "policy";
    let address = "policy@example.net";
    admin_user(
        &setup,
        &user_pool_id,
        username,
        &[("email", address), ("email_verified", "true")],
    )
    .await;
    ok_anonymous(
        &emulator,
        "ForgotPassword",
        &json!({ "ClientId": client_id, "Username": username }),
    )
    .await;
    let code = outstanding_code(&engine, &user_pool_id, username, &purpose)
        .expect("the request leaves a code outstanding");
    let (status, payload) = dispatch(
        &emulator,
        Addressing::Anonymous,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": code,
            "Password": WEAK_PASSWORD,
        }),
    )
    .await;
    assert_eq!(
        error_code(&payload),
        "InvalidPasswordException",
        "case \"a password the policy refuses\": expected InvalidPasswordException, got \
         {status} with {payload}"
    );
    let (status, payload) = dispatch(
        &emulator,
        Addressing::Anonymous,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": code,
            "Password": RESET_PASSWORD,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the same code after a refused password\": expected HTTP 200, got {status} \
         with {payload}"
    );
}

/// RULE: an attribute's code confirms THAT attribute of THAT user and nothing else —
/// over a table whose rows differ along the attribute the code was sent for, the
/// attribute it is presented for, and whether the value was held pending.
#[tokio::test]
async fn an_attribute_code_confirms_only_the_attribute_it_was_sent_for() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) =
        pool_with_client(&setup, "attribute-codes", &json!({})).await;

    // (case, attribute the code is sent for, attribute it is presented for, whether
    //  the outstanding code is presented, expected status, expected error code)
    let rows: Vec<(&str, &str, &str, bool, u16, &str)> = vec![
        (
            "the email code, presented for email",
            "email",
            "email",
            true,
            200,
            "",
        ),
        (
            "the phone_number code, presented for phone_number",
            "phone_number",
            "phone_number",
            true,
            200,
            "",
        ),
        (
            // No code is live for the attribute it is presented FOR, so there is
            // nothing to compare it against.
            "the email code, presented for phone_number",
            "email",
            "phone_number",
            true,
            400,
            "ExpiredCodeException",
        ),
        (
            "a value the service never sent, presented for email",
            "email",
            "email",
            false,
            400,
            "CodeMismatchException",
        ),
    ];

    for (index, (case, sent_for, presented_for, present_outstanding, expected_status, expected_error)) in
        rows.iter().enumerate()
    {
        let username = format!("attribute{index}");
        let address = format!("{username}@example.org");
        let phone = format!("+1555010{index:04}");
        admin_user(
            &setup,
            &user_pool_id,
            &username,
            &[("email", &address), ("phone_number", &phone)],
        )
        .await;
        let token = access_token(&setup, &user_pool_id, &client_id, &username, GOOD_PASSWORD).await;

        let sent = ok_anonymous(
            &emulator,
            "GetUserAttributeVerificationCode",
            &json!({ "AccessToken": token, "AttributeName": sent_for }),
        )
        .await;
        let destination = if *sent_for == "email" { &address } else { &phone };
        assert_delivery(case, &sent["CodeDeliveryDetails"], sent_for, destination);

        let purpose = state::CodePurpose::AttributeVerification((*sent_for).to_string());
        let outstanding = outstanding_code(&engine, &user_pool_id, &username, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the send must leave a code outstanding"));
        let code = if *present_outstanding {
            outstanding.clone()
        } else {
            wrong_code(&outstanding)
        };

        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "VerifyUserAttribute",
            &json!({ "AccessToken": token, "AttributeName": presented_for, "Code": code }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        if !expected_error.is_empty() {
            assert_eq!(
                error_code(&payload),
                *expected_error,
                "case {case:?}: expected error code {expected_error}, got {payload}"
            );
        }

        // ROUND TRIP: only an accepted code confirms the attribute, read back through
        // the user's own profile.
        let accepted = *expected_status == 200;
        let attributes = stored_attributes(&setup, &user_pool_id, &username).await;
        let flag = state::verified_flag(presented_for).unwrap_or_default();
        assert_eq!(
            attributes.contains(&(flag.to_string(), "true".to_string())),
            accepted,
            "case {case:?}: {flag} follows the outcome, got {attributes:?}"
        );
        if accepted {
            // The code is spent, so no live code is left to compare against: the
            // refusal is the one that asks for a code to be requested.
            let (_, replay) = dispatch(
                &emulator,
                Addressing::Anonymous,
                "VerifyUserAttribute",
                &json!({ "AccessToken": token, "AttributeName": presented_for, "Code": code }),
            )
            .await;
            assert_eq!(
                error_code(&replay),
                "ExpiredCodeException",
                "case {case:?}: a spent code is refused, got {replay}"
            );
        }
    }

    // An update the pool holds PENDING until the user confirms it sends a code for the
    // pending value, and accepting that code is what promotes it.
    let (pending_pool, pending_client) = pool_with_client(
        &setup,
        "pending-email",
        &json!({
            "AutoVerifiedAttributes": ["email"],
            "UserAttributeUpdateSettings": {
                "AttributesRequireVerificationBeforeUpdate": ["email"]
            },
        }),
    )
    .await;
    admin_user(
        &setup,
        &pending_pool,
        "mover",
        &[("email", "old@example.com"), ("email_verified", "true")],
    )
    .await;
    let token = access_token(&setup, &pending_pool, &pending_client, "mover", GOOD_PASSWORD).await;
    let updated = ok_anonymous(
        &emulator,
        "UpdateUserAttributes",
        &json!({
            "AccessToken": token,
            "UserAttributes": [{ "Name": "email", "Value": "new@example.com" }],
        }),
    )
    .await;
    let deliveries = updated["CodeDeliveryDetailsList"]
        .as_array()
        .expect("the update reports one delivery per code it sent");
    assert_eq!(
        deliveries.len(),
        1,
        "case \"a pending email update\": expected one delivery, got {updated}"
    );
    assert_delivery(
        "a pending email update",
        &deliveries[0],
        "email",
        "new@example.com",
    );
    let purpose = state::CodePurpose::AttributeVerification("email".to_string());
    let code = outstanding_code(&engine, &pending_pool, "mover", &purpose)
        .expect("a reported delivery is a code that was minted and stored");
    let attributes = stored_attributes(&setup, &pending_pool, "mover").await;
    assert!(
        attributes.contains(&("email".to_string(), "old@example.com".to_string())),
        "case \"a pending email update\": the stored value stands until the code is \
         accepted, got {attributes:?}"
    );
    let (status, payload) = dispatch(
        &emulator,
        Addressing::Anonymous,
        "VerifyUserAttribute",
        &json!({ "AccessToken": token, "AttributeName": "email", "Code": code }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a pending email update\": the delivered code must be accepted, got \
         {status} with {payload}"
    );
    let attributes = stored_attributes(&setup, &pending_pool, "mover").await;
    assert!(
        attributes.contains(&("email".to_string(), "new@example.com".to_string()))
            && attributes.contains(&("email_verified".to_string(), "true".to_string())),
        "case \"a pending email update\": the confirmed value becomes the user's, got \
         {attributes:?}"
    );
}

/// RULE: a code past its validity window is refused `ExpiredCodeException` while one
/// inside it is accepted — over a table whose rows differ along the purpose the code
/// was sent for and how old it is, including the window's own bound.
#[tokio::test]
async fn a_code_past_its_validity_window_is_refused_as_expired() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) = pool_with_client(
        &setup,
        "code-expiry",
        &json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    let window = state::VERIFICATION_CODE_VALIDITY;

    // (case, how old the outstanding code is made, expected status, expected error)
    let ages: Vec<(&str, Duration, u16, &str)> = vec![
        ("just minted", Duration::from_secs(0), 200, ""),
        (
            "one second inside the window",
            window - Duration::from_secs(1),
            200,
            "",
        ),
        (
            "exactly at the window's end",
            window,
            400,
            "ExpiredCodeException",
        ),
        (
            "one second past the window",
            window + Duration::from_secs(1),
            400,
            "ExpiredCodeException",
        ),
    ];

    for (index, (case, age, expected_status, expected_error)) in ages.iter().enumerate() {
        // The sign-up confirmation code, aged by the clock rather than by waiting.
        let username = format!("expiry{index}");
        let address = format!("{username}@example.com");
        ok_anonymous(
            &emulator,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "Password": GOOD_PASSWORD,
                "UserAttributes": [{ "Name": "email", "Value": address }],
            }),
        )
        .await;
        let purpose = state::CodePurpose::SignUpConfirmation;
        let code = outstanding_code(&engine, &user_pool_id, &username, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the sign-up must leave a code outstanding"));
        backdate_code(&engine, &user_pool_id, &username, &purpose, *age);
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ConfirmSignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "ConfirmationCode": code,
            }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?} (ConfirmSignUp): expected HTTP {expected_status}, got {status} \
             with {payload}"
        );
        if !expected_error.is_empty() {
            assert_eq!(
                error_code(&payload),
                *expected_error,
                "case {case:?} (ConfirmSignUp): expected {expected_error}, got {payload}"
            );
        }

        // The password-reset code, over the same window.
        let reset_user = format!("expiryreset{index}");
        let reset_address = format!("{reset_user}@example.com");
        admin_user(
            &setup,
            &user_pool_id,
            &reset_user,
            &[("email", &reset_address), ("email_verified", "true")],
        )
        .await;
        ok_anonymous(
            &emulator,
            "ForgotPassword",
            &json!({ "ClientId": client_id, "Username": reset_user }),
        )
        .await;
        let purpose = state::CodePurpose::PasswordReset;
        let code = outstanding_code(&engine, &user_pool_id, &reset_user, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the request must leave a code outstanding"));
        backdate_code(&engine, &user_pool_id, &reset_user, &purpose, *age);
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ConfirmForgotPassword",
            &json!({
                "ClientId": client_id,
                "Username": reset_user,
                "ConfirmationCode": code,
                "Password": RESET_PASSWORD,
            }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?} (ConfirmForgotPassword): expected HTTP {expected_status}, got \
             {status} with {payload}"
        );
        if !expected_error.is_empty() {
            assert_eq!(
                error_code(&payload),
                *expected_error,
                "case {case:?} (ConfirmForgotPassword): expected {expected_error}, got \
                 {payload}"
            );
        }

        // The attribute verification code, over the same window.
        let attribute_user = format!("expiryattribute{index}");
        let attribute_address = format!("{attribute_user}@example.com");
        admin_user(
            &setup,
            &user_pool_id,
            &attribute_user,
            &[("email", &attribute_address)],
        )
        .await;
        let token = access_token(
            &setup,
            &user_pool_id,
            &client_id,
            &attribute_user,
            GOOD_PASSWORD,
        )
        .await;
        ok_anonymous(
            &emulator,
            "GetUserAttributeVerificationCode",
            &json!({ "AccessToken": token, "AttributeName": "email" }),
        )
        .await;
        let purpose = state::CodePurpose::AttributeVerification("email".to_string());
        let code = outstanding_code(&engine, &user_pool_id, &attribute_user, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the send must leave a code outstanding"));
        backdate_code(&engine, &user_pool_id, &attribute_user, &purpose, *age);
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "VerifyUserAttribute",
            &json!({ "AccessToken": token, "AttributeName": "email", "Code": code }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?} (VerifyUserAttribute): expected HTTP {expected_status}, got \
             {status} with {payload}"
        );
        if !expected_error.is_empty() {
            assert_eq!(
                error_code(&payload),
                *expected_error,
                "case {case:?} (VerifyUserAttribute): expected {expected_error}, got \
                 {payload}"
            );
        }
    }
}

/// RULE: the `EMAIL_OTP` challenge delivers a one-time password that signs the user in
/// exactly once, and only while its session lasts — rows differ along which code is
/// presented and whether the session's window is still open.
#[tokio::test]
async fn an_email_one_time_password_signs_the_user_in_once() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) = pool_with_client(
        &setup,
        "email-otp",
        &json!({
            "Policies": {
                "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "EMAIL_OTP"] }
            },
        }),
    )
    .await;
    admin_user(
        &setup,
        &user_pool_id,
        "otp",
        &[("email", "otp@example.com"), ("email_verified", "true")],
    )
    .await;

    /// Start the challenge and return its session token.
    async fn start(router: &MultiServiceRouter, client_id: &str) -> (String, Value) {
        let started = ok_anonymous(
            router,
            "InitiateAuth",
            &json!({
                "ClientId": client_id,
                "AuthFlow": "USER_AUTH",
                "AuthParameters": { "USERNAME": "otp", "PREFERRED_CHALLENGE": "EMAIL_OTP" },
            }),
        )
        .await;
        let session = started["Session"]
            .as_str()
            .expect("a challenge carries a session")
            .to_string();
        (session, started)
    }

    let (session, started) = start(&emulator, &client_id).await;
    assert_eq!(
        started["ChallengeName"],
        json!("EMAIL_OTP"),
        "the preferred challenge is the one started, got {started}"
    );
    assert!(
        started["AuthenticationResult"].is_null(),
        "a challenge carries no tokens, got {started}"
    );
    assert_eq!(
        started["ChallengeParameters"]["CODE_DELIVERY_DESTINATION"],
        json!(state::masked_destination("otp@example.com")),
        "the challenge reports the stored address masked, got {started}"
    );
    assert_eq!(
        started["ChallengeParameters"]["CODE_DELIVERY_DELIVERY_MEDIUM"],
        json!("EMAIL"),
        "the challenge reports the medium the code travelled over, got {started}"
    );
    let first = session_code(&engine, &session).expect("the challenge delivers a code");
    assert!(
        first.len() == 8 && first.chars().all(|digit| digit.is_ascii_digit()),
        "an email one-time password is eight decimal digits, got {first:?}"
    );
    assert!(
        !started.to_string().contains(&first),
        "no response member carries the code itself, got {started}"
    );

    // A code the challenge did not deliver is refused, and the session stands so the
    // right code can still answer it.
    let (status, payload) = dispatch(
        &emulator,
        Addressing::Anonymous,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": session,
            "ChallengeResponses": { "USERNAME": "otp", "EMAIL_OTP_CODE": wrong_code(&first) },
        }),
    )
    .await;
    assert_eq!(
        error_code(&payload),
        "CodeMismatchException",
        "case \"a code the challenge did not deliver\": expected CodeMismatchException, \
         got {status} with {payload}"
    );

    // A second challenge gets its own code, and the first one no longer answers it.
    let (reissued_session, _) = start(&emulator, &client_id).await;
    let reissued = session_code(&engine, &reissued_session).expect("the challenge delivers a code");
    assert_ne!(
        reissued, first,
        "case \"a reissued challenge\": each challenge mints its own code"
    );
    let (_, payload) = dispatch(
        &emulator,
        Addressing::Anonymous,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": reissued_session,
            "ChallengeResponses": { "USERNAME": "otp", "EMAIL_OTP_CODE": first },
        }),
    )
    .await;
    assert_eq!(
        error_code(&payload),
        "CodeMismatchException",
        "case \"another challenge's code\": expected CodeMismatchException, got {payload}"
    );

    // The delivered code completes the sign-in, and the session is spent with it.
    let signed_in = ok_anonymous(
        &emulator,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": reissued_session,
            "ChallengeResponses": { "USERNAME": "otp", "EMAIL_OTP_CODE": reissued },
        }),
    )
    .await;
    assert!(
        signed_in["AuthenticationResult"]["AccessToken"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "case \"the delivered code\": the sign-in must issue tokens, got {signed_in}"
    );
    let (_, replay) = dispatch(
        &emulator,
        Addressing::Anonymous,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": reissued_session,
            "ChallengeResponses": { "USERNAME": "otp", "EMAIL_OTP_CODE": reissued },
        }),
    )
    .await;
    assert_eq!(
        error_code(&replay),
        "NotAuthorizedException",
        "case \"the same code again\": an answered session is spent, got {replay}"
    );

    // A challenge whose window has closed is refused as an expired CODE: that window is
    // what bounds the one-time password.
    let (expiring_session, _) = start(&emulator, &client_id).await;
    let expiring = session_code(&engine, &expiring_session).expect("the challenge delivers a code");
    expire_session(&engine, &expiring_session);
    let (status, payload) = dispatch(
        &emulator,
        Addressing::Anonymous,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": expiring_session,
            "ChallengeResponses": { "USERNAME": "otp", "EMAIL_OTP_CODE": expiring },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a session whose window closed\": expected HTTP 400, got {status} with \
         {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ExpiredCodeException",
        "case \"a session whose window closed\": expected ExpiredCodeException, got \
         {payload}"
    );
}

/// RULE: confirming a sign-up makes the confirmed value an ALIAS of that user, and an
/// alias names ONE user of the pool — so a value another user already holds is refused
/// unless the request asks for the alias to be migrated.
#[tokio::test]
async fn confirming_a_sign_up_migrates_an_alias_only_when_the_request_asks() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) = pool_with_client(
        &setup,
        "alias-codes",
        &json!({
            "AliasAttributes": ["email"],
            "AutoVerifiedAttributes": ["email"],
        }),
    )
    .await;
    let purpose = state::CodePurpose::SignUpConfirmation;
    let shared = "shared@example.com";
    admin_user(
        &setup,
        &user_pool_id,
        "holder",
        &[("email", shared), ("email_verified", "true")],
    )
    .await;

    // (case, whether the request asks for the migration, expected status, expected
    //  error code)
    let rows: Vec<(&str, bool, u16, &str)> = vec![
        ("without ForceAliasCreation", false, 400, "AliasExistsException"),
        ("with ForceAliasCreation", true, 200, ""),
    ];

    for (index, (case, force, expected_status, expected_error)) in rows.iter().enumerate() {
        let username = format!("claimant{index}");
        ok_anonymous(
            &emulator,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "Password": GOOD_PASSWORD,
                "UserAttributes": [{ "Name": "email", "Value": shared }],
            }),
        )
        .await;
        let code = outstanding_code(&engine, &user_pool_id, &username, &purpose)
            .unwrap_or_else(|| panic!("case {case:?}: the sign-up must leave a code outstanding"));
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ConfirmSignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "ConfirmationCode": code,
                "ForceAliasCreation": force,
            }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        if !expected_error.is_empty() {
            assert_eq!(
                error_code(&payload),
                *expected_error,
                "case {case:?}: expected error code {expected_error}, got {payload}"
            );
            // The refusal wrote nothing: the code still stands and so does the holder's
            // alias.
            assert!(
                outstanding_code(&engine, &user_pool_id, &username, &purpose).is_some(),
                "case {case:?}: a refused confirmation does not spend the code"
            );
        }
        let holder = stored_attributes(&setup, &user_pool_id, "holder").await;
        assert_eq!(
            holder.contains(&("email_verified".to_string(), "true".to_string())),
            !*force,
            "case {case:?}: the previous holder keeps its alias unless it is migrated, \
             got {holder:?}"
        );
    }
}

/// RULE: a password-reset message goes to the VERIFIED attribute the pool's
/// `AccountRecoverySetting` prioritises, and a pool that reserves resets to
/// administrators sends none — over a table whose rows differ along the pool's
/// configuration and which of the user's values are verified.
#[tokio::test]
async fn forgot_password_follows_the_pools_account_recovery_setting() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let verified_email_first = json!({
        "AccountRecoverySetting": {
            "RecoveryMechanisms": [
                { "Name": "verified_email", "Priority": 1 },
                { "Name": "verified_phone_number", "Priority": 2 },
            ]
        }
    });
    let verified_phone_first = json!({
        "AccountRecoverySetting": {
            "RecoveryMechanisms": [
                { "Name": "verified_phone_number", "Priority": 1 },
                { "Name": "verified_email", "Priority": 2 },
            ]
        }
    });
    // The same ranking on a pool that can actually SEND a text message: an SMS
    // reset code leaves through the Amazon SNS caller the pool's
    // `SmsConfiguration` names, so the phone mechanism qualifies only where the
    // pool stores one.
    let verified_phone_first_over_sms = merged(
        verified_phone_first.clone(),
        &json!({
            "SmsConfiguration": { "SnsCallerArn": "arn:aws:iam::000000000000:role/sms-caller" }
        }),
    );
    let admin_only = json!({
        "AccountRecoverySetting": {
            "RecoveryMechanisms": [{ "Name": "admin_only", "Priority": 1 }]
        }
    });

    // (case, pool configuration, the user's attributes, expected status, expected
    //  attribute the code goes to or the expected error code)
    type Row = (&'static str, Value, Vec<(&'static str, &'static str)>, u16, &'static str);
    let rows: Vec<Row> = vec![
        (
            "a pool that configures no recovery, with a verified email",
            json!({}),
            vec![
                ("email", "a@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100001"),
                ("phone_number_verified", "true"),
            ],
            200,
            "email",
        ),
        (
            "verified_email at priority 1, with both verified",
            verified_email_first,
            vec![
                ("email", "b@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100002"),
                ("phone_number_verified", "true"),
            ],
            200,
            "email",
        ),
        (
            "verified_phone_number at priority 1, with both verified",
            verified_phone_first_over_sms,
            vec![
                ("email", "c@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100003"),
                ("phone_number_verified", "true"),
            ],
            200,
            "phone_number",
        ),
        (
            // The same ranking and the same verified number, on a pool with no
            // SMS delivery path at all: the phone mechanism cannot deliver, so
            // the pool falls through to the verified email behind it.
            "verified_phone_number first and verified, on a pool that sends no SMS",
            verified_phone_first.clone(),
            vec![
                ("email", "h@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100008"),
                ("phone_number_verified", "true"),
            ],
            200,
            "email",
        ),
        (
            // And with the verified number as the user's ONLY address, nothing
            // qualifies at all.
            "a verified number alone, on a pool that sends no SMS",
            verified_phone_first.clone(),
            vec![
                ("phone_number", "+15550100009"),
                ("phone_number_verified", "true"),
            ],
            400,
            "InvalidParameterException",
        ),
        (
            "verified_phone_number first, but only the email is verified",
            verified_phone_first,
            vec![
                ("email", "d@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100004"),
            ],
            200,
            "email",
        ),
        (
            "a pool that reserves resets to administrators",
            admin_only,
            vec![("email", "e@example.com"), ("email_verified", "true")],
            400,
            "NotAuthorizedException",
        ),
        (
            "a user whose only address is unverified",
            json!({}),
            vec![("email", "f@example.com")],
            400,
            "InvalidParameterException",
        ),
        (
            "a user with no address at all",
            json!({}),
            vec![("name", "Gina")],
            400,
            "InvalidParameterException",
        ),
    ];

    for (index, (case, configuration, attributes, expected_status, expected)) in
        rows.iter().enumerate()
    {
        let (user_pool_id, client_id) =
            pool_with_client(&setup, &format!("recovery{index}"), configuration).await;
        let username = format!("recovery{index}");
        admin_user(&setup, &user_pool_id, &username, attributes).await;

        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ForgotPassword",
            &json!({ "ClientId": client_id, "Username": username }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        if *expected_status == 200 {
            let destination = attributes
                .iter()
                .find(|(name, _)| name == expected)
                .map(|(_, value)| *value)
                .unwrap_or_default();
            assert_delivery(case, &payload["CodeDeliveryDetails"], expected, destination);
            // The reported delivery is a code that was really minted: it is the one the
            // reset is completed with.
            assert!(
                outstanding_code(
                    &engine,
                    &user_pool_id,
                    &username,
                    &state::CodePurpose::PasswordReset
                )
                .is_some(),
                "case {case:?}: a reported delivery leaves a code outstanding"
            );
        } else {
            assert_eq!(
                error_code(&payload),
                *expected,
                "case {case:?}: expected error code {expected}, got {payload}"
            );
            assert!(
                outstanding_code(
                    &engine,
                    &user_pool_id,
                    &username,
                    &state::CodePurpose::PasswordReset
                )
                .is_none(),
                "case {case:?}: a refused request sends no code"
            );
        }
    }
}
