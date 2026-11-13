// SPDX-License-Identifier: CC0-1.0

//! `AdminResetUserPassword` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.AdminResetUserPassword`).
//!
//! The rule under test: an administrative password reset BEGINS a password reset of the
//! stored user the request names — it puts that user in `RESET_REQUIRED`, so no password
//! already stored signs it in, and sends it one password-reset code, addressed to the
//! destination the pool's account-recovery configuration prioritises among the user's
//! VERIFIED values, recorded as the single code outstanding for a reset of that user; a
//! pool whose configuration offers that user no such destination begins no reset and
//! stores nothing. The code it sends is the one that completes the reset, for as long as
//! a sent code stays acceptable, and the reset settles the user back into `CONFIRMED`
//! with the password the completion chose.
//!
//! The code itself is delivered out of band, so no response carries it: a test learns the
//! delivered code the way a mailbox would — from the emulator's own state.
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

/// A password satisfying a default pool's password policy, so a created user can sign in
/// before anything resets it.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// Another password satisfying it, for a completed reset to settle on.
const RESET_PASSWORD: &str = "Res3tPassw0rd!";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service —
    /// `AdminResetUserPassword` among them.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service operations
    /// that complete a reset are called.
    Anonymous,
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
/// that admits the sign-in flows these tests use, and return both minted ids.
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

/// Create `username` with `attributes` and a permanent password, which is what makes it
/// a `CONFIRMED` user that signs in, and return its generated `sub`.
async fn admin_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> String {
    let attributes: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    let created = ok(
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
    created["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string()
}

/// Begin an administrative reset of `username` and return its status and body.
async fn admin_reset(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> (u16, Value) {
    dispatch(
        router,
        Addressing::Signed,
        "AdminResetUserPassword",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await
}

/// The `UserStatus` `AdminGetUser` reports for `username`.
async fn stored_status(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> String {
    let read_back = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    read_back["UserStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("AdminGetUser must report a UserStatus, got {read_back}"))
        .to_string()
}

/// The code outstanding for a password reset of `username`, read out of the emulator's
/// own state: the real service delivers a reset code out of band, so no response carries
/// it.
fn outstanding_reset_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
) -> Option<state::VerificationCode> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(&state::CodePurpose::PasswordReset))
        .cloned()
}

/// Backdate the outstanding reset code of `username` by `age`, so the validity window is
/// read against the clock rather than waited out.
fn backdate_reset_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    age: Duration,
) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let code = guard
        .pool_mut(user_pool_id)
        .and_then(|pool| pool.user_mut(username))
        .and_then(|user| {
            user.outstanding_codes
                .get_mut(&state::CodePurpose::PasswordReset)
        })
        .expect("a reset code is outstanding");
    code.issued_at = SystemTime::now() - age;
}

/// Complete a reset of `username` with `code`, choosing `password`.
async fn confirm_reset(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    code: &str,
    password: &str,
) -> (u16, Value) {
    dispatch(
        router,
        Addressing::Anonymous,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "ConfirmationCode": code,
            "Password": password,
        }),
    )
    .await
}

/// Sign `username` in with `password` through the admin password flow.
async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
    password: &str,
) -> (u16, Value) {
    dispatch(
        router,
        Addressing::Signed,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": password },
        }),
    )
    .await
}

/// A code of the same shape the service did NOT send: every digit shifted by one, so no
/// character of it matches.
fn wrong_code(code: &str) -> String {
    code.chars()
        .map(|digit| char::from_digit((digit.to_digit(10).unwrap_or(0) + 1) % 10, 10).unwrap_or('0'))
        .collect()
}

/// RULE: the reset message goes to the VERIFIED attribute the pool's
/// `AccountRecoverySetting` prioritises, and a pool whose configuration offers the user
/// no such destination begins no reset at all — over a table whose rows differ along the
/// pool's configuration and which of the user's values are verified.
#[tokio::test]
async fn an_admin_reset_follows_the_pools_account_recovery_setting() {
    let engine = engine();
    let setup = router(&engine, false);

    // The `RecoveryMechanisms` names and the `admin_only` option are
    // `RecoveryOptionNameType` values
    // (`smithy_query(query="shape",shape="RecoveryOptionNameType")`).
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
    let admin_only = json!({
        "AccountRecoverySetting": {
            "RecoveryMechanisms": [{ "Name": "admin_only", "Priority": 1 }]
        }
    });
    // An SMS reset code leaves the pool through the Amazon SNS caller its
    // `SmsConfiguration` names, so a pool that stores none has no SMS delivery
    // path and the phone mechanism cannot be the one a code goes over
    // (`SmsConfigurationType` in this service's model). Every row below that
    // expects a text message therefore configures that caller.
    let sends_sms =
        json!({ "SmsConfiguration": { "SnsCallerArn": "arn:aws:iam::000000000000:role/sms-caller" } });

    // (case, pool configuration, the user's attributes, expected status, the attribute
    //  the code goes to or the expected error code)
    type Row = (
        &'static str,
        Value,
        Vec<(&'static str, &'static str)>,
        u16,
        &'static str,
    );
    let rows: Vec<Row> = vec![
        (
            "a pool that configures no recovery, with both values verified",
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
            merged(verified_phone_first.clone(), &sends_sms),
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
            // SMS delivery path: the phone mechanism cannot deliver, so the
            // verified email behind it is the destination.
            "verified_phone_number first and verified, on a pool that sends no SMS",
            verified_phone_first.clone(),
            vec![
                ("email", "i@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100009"),
                ("phone_number_verified", "true"),
            ],
            200,
            "email",
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
            "a pool with no email at all, falling back to the verified number",
            sends_sms.clone(),
            vec![
                ("phone_number", "+15550100005"),
                ("phone_number_verified", "true"),
            ],
            200,
            "phone_number",
        ),
        (
            // The same user on a pool that cannot text: nothing qualifies at all.
            "a verified number alone, on a pool that sends no SMS",
            json!({}),
            vec![
                ("phone_number", "+15550100010"),
                ("phone_number_verified", "true"),
            ],
            400,
            "InvalidParameterException",
        ),
        (
            "a pool that reserves account recovery to administrators",
            admin_only,
            vec![("email", "f@example.com"), ("email_verified", "true")],
            400,
            "NotAuthorizedException",
        ),
        (
            "a user whose only address is unverified",
            json!({}),
            vec![("email", "g@example.com")],
            400,
            "InvalidParameterException",
        ),
        (
            "a user with no address at all",
            json!({}),
            vec![("name", "Hana")],
            400,
            "InvalidParameterException",
        ),
    ];

    for (index, (case, configuration, attributes, expected_status, expected)) in
        rows.iter().enumerate()
    {
        let (user_pool_id, _client_id) =
            pool_with_client(&setup, &format!("recovery{index}"), configuration).await;
        let username = format!("recovery{index}");
        admin_user(&setup, &user_pool_id, &username, attributes).await;
        let status_before = stored_status(&setup, &user_pool_id, &username).await;

        let (status, payload) = admin_reset(&setup, &user_pool_id, &username).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );

        if *expected_status == 200 {
            assert_eq!(
                payload,
                Value::Object(serde_json::Map::new()),
                "case {case:?}: this operation models no output member, got {payload}"
            );
            let destination = attributes
                .iter()
                .find(|(name, _)| name == expected)
                .map(|(_, value)| *value)
                .unwrap_or_default();
            let code = outstanding_reset_code(&engine, &user_pool_id, &username)
                .unwrap_or_else(|| panic!("case {case:?}: a reset must leave a code outstanding"));
            assert_eq!(
                (code.attribute.as_str(), code.destination.as_str()),
                (*expected, destination),
                "case {case:?}: expected the code sent to {expected} at {destination}, got {code:?}"
            );
            let actual = stored_status(&setup, &user_pool_id, &username).await;
            assert_eq!(
                actual,
                state::UserStatus::ResetRequired.as_str(),
                "case {case:?}: expected the reset user in RESET_REQUIRED, got {actual:?}"
            );
        } else {
            assert_eq!(
                error_code(&payload),
                *expected,
                "case {case:?}: expected error code {expected}, got {payload}"
            );
            assert!(
                outstanding_reset_code(&engine, &user_pool_id, &username).is_none(),
                "case {case:?}: a refused reset sends no code"
            );
            let actual = stored_status(&setup, &user_pool_id, &username).await;
            assert_eq!(
                actual, status_before,
                "case {case:?}: a refused reset leaves the stored status at {status_before:?}, \
                 got {actual:?}"
            );
        }
    }
}

/// RULE: the reset blocks every sign-in with the password the user had — each flow
/// reporting it as the one `PasswordResetRequiredException` sentence this service reports
/// that state with — and the code it sent is what completes it, once, settling the user
/// back into `CONFIRMED` with the password the completion chose. Rows differ along the
/// sign-in flow presented and along the code presented.
#[tokio::test]
async fn the_reset_blocks_sign_in_until_its_own_code_completes_it() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) = pool_with_client(&setup, "reset-flow", &json!({})).await;
    let username = "ingrid";
    admin_user(
        &setup,
        &user_pool_id,
        username,
        &[("email", "ingrid@example.com"), ("email_verified", "true")],
    )
    .await;

    // Before the reset the stored password signs the user in.
    let (status, payload) = sign_in(&setup, &user_pool_id, &client_id, username, GOOD_PASSWORD).await;
    assert_eq!(
        status, 200,
        "case \"before the reset\": the stored password signs the user in, got {status} with \
         {payload}"
    );
    let refresh_token = payload["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the sign-in issues a refresh token, got {payload}"))
        .to_string();

    let (status, payload) = admin_reset(&setup, &user_pool_id, username).await;
    assert_eq!(
        status, 200,
        "the reset must succeed, got {status} with {payload}"
    );
    let code = outstanding_reset_code(&engine, &user_pool_id, username)
        .expect("the reset leaves a code outstanding")
        .code;

    // Every sign-in flow now reports the state the reset put the user in, in the one
    // sentence this service reports it with — rows differ along the flow presented.
    let rows: Vec<(&str, Addressing, &str, Value)> = vec![
        (
            "the admin password flow",
            Addressing::Signed,
            "AdminInitiateAuth",
            json!({
                "UserPoolId": user_pool_id.clone(),
                "ClientId": client_id.clone(),
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "the client-side password flow",
            Addressing::Anonymous,
            "InitiateAuth",
            json!({
                "ClientId": client_id.clone(),
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "a refresh token the user held before the reset",
            Addressing::Anonymous,
            "InitiateAuth",
            json!({
                "ClientId": client_id.clone(),
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": refresh_token },
            }),
        ),
    ];
    for (case, addressing, operation, body) in rows {
        // A signed call goes to the router that does not verify signatures (this suite
        // writes no real signature); an unsigned one to the authenticating router, which
        // is what makes it an anonymous call to the emulator's own endpoint.
        let router = match addressing {
            Addressing::Signed => &setup,
            Addressing::Anonymous => &emulator,
        };
        let (status, payload) = dispatch(router, addressing, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "PasswordResetRequiredException"),
            "case {case:?}: expected a 400 PasswordResetRequiredException, got {status} with \
             {payload}"
        );
        // The sentence Amazon Cognito words this refusal with, written out here rather
        // than taken from the emulator, so the wording itself is pinned and not merely
        // compared with itself.
        assert_eq!(
            payload["message"],
            json!("Password reset required for the user"),
            "case {case:?}: expected the message Amazon Cognito words this refusal with, \
             got {payload}"
        );
    }

    // A value the service never sent completes nothing, and leaves the reset standing.
    let (status, payload) = confirm_reset(
        &emulator,
        &client_id,
        username,
        &wrong_code(&code),
        RESET_PASSWORD,
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "CodeMismatchException"),
        "case \"a code the reset never sent\": expected a 400 CodeMismatchException, got {status} \
         with {payload}"
    );
    let actual = stored_status(&setup, &user_pool_id, username).await;
    assert_eq!(
        actual,
        state::UserStatus::ResetRequired.as_str(),
        "case \"a code the reset never sent\": the user stays in RESET_REQUIRED, got {actual:?}"
    );

    // The code the reset sent completes it.
    let (status, payload) =
        confirm_reset(&emulator, &client_id, username, &code, RESET_PASSWORD).await;
    assert_eq!(
        status, 200,
        "case \"the code the reset sent\": expected HTTP 200, got {status} with {payload}"
    );
    let actual = stored_status(&setup, &user_pool_id, username).await;
    assert_eq!(
        actual,
        state::UserStatus::Confirmed.as_str(),
        "case \"the code the reset sent\": expected the user back in CONFIRMED, got {actual:?}"
    );

    // ROUND TRIP: the password the completion chose is the one that signs in now, and the
    // one the reset deactivated no longer does.
    let (status, payload) =
        sign_in(&setup, &user_pool_id, &client_id, username, RESET_PASSWORD).await;
    assert_eq!(
        status, 200,
        "case \"the password the reset settled on\": the user signs in, got {status} with {payload}"
    );
    let (status, payload) = sign_in(&setup, &user_pool_id, &client_id, username, GOOD_PASSWORD).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "NotAuthorizedException"),
        "case \"the password the reset replaced\": expected a 400 NotAuthorizedException, got \
         {status} with {payload}"
    );

    // The code is spent, so nothing is live to compare a second presentation against.
    let (status, payload) =
        confirm_reset(&emulator, &client_id, username, &code, GOOD_PASSWORD).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ExpiredCodeException"),
        "case \"the same code a second time\": expected a 400 ExpiredCodeException, got {status} \
         with {payload}"
    );
}

/// RULE: the code the reset sends completes it only inside the validity window a sent
/// code has — over rows differing along the code's age, including the window's own bound
/// and one past it. The window is this service's own
/// [`state::VERIFICATION_CODE_VALIDITY`], so the rows are written against it rather than
/// against a number.
#[tokio::test]
async fn the_code_a_reset_sends_completes_it_only_inside_its_validity_window() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let (user_pool_id, client_id) = pool_with_client(&setup, "reset-code-ttl", &json!({})).await;

    const ONE_SECOND: Duration = Duration::from_secs(1);
    const ONE_WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);
    let window = state::VERIFICATION_CODE_VALIDITY;

    // (case, how old the code is when it is presented, accepted)
    let rows: Vec<(&str, Duration, bool)> = vec![
        ("a code presented the moment it was sent", Duration::ZERO, true),
        ("a code one second inside the window", window - ONE_SECOND, true),
        ("a code exactly at the window's bound", window, false),
        ("a code one second past the window", window + ONE_SECOND, false),
        ("a code a week past the window", window + ONE_WEEK, false),
    ];

    for (index, (case, age, accepted)) in rows.iter().enumerate() {
        let username = format!("ttl{index}");
        let address = format!("{username}@example.net");
        admin_user(
            &setup,
            &user_pool_id,
            &username,
            &[("email", &address), ("email_verified", "true")],
        )
        .await;
        let (status, payload) = admin_reset(&setup, &user_pool_id, &username).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reset must succeed, got {status} with {payload}"
        );
        let code = outstanding_reset_code(&engine, &user_pool_id, &username)
            .unwrap_or_else(|| panic!("case {case:?}: the reset leaves a code outstanding"))
            .code;
        backdate_reset_code(&engine, &user_pool_id, &username, *age);

        let (status, payload) =
            confirm_reset(&emulator, &client_id, &username, &code, RESET_PASSWORD).await;
        let expected_status = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        if !*accepted {
            assert_eq!(
                error_code(&payload),
                "ExpiredCodeException",
                "case {case:?}: expected ExpiredCodeException, got {payload}"
            );
        }
        let expected_user_status = if *accepted {
            state::UserStatus::Confirmed
        } else {
            state::UserStatus::ResetRequired
        };
        let actual = stored_status(&setup, &user_pool_id, &username).await;
        assert_eq!(
            actual,
            expected_user_status.as_str(),
            "case {case:?}: expected the user in {}, got {actual:?}",
            expected_user_status.as_str()
        );
    }
}

/// RULE: a reset sends ONE code, so a second reset supersedes the first one's code; and
/// it lands on the one user of the one pool the request names, that user resolving by its
/// username or by its `sub`.
#[tokio::test]
async fn a_reset_supersedes_the_earlier_code_and_lands_on_one_user_of_one_pool() {
    let engine = engine();
    let setup = router(&engine, false);
    let (first_pool, _first_client) = pool_with_client(&setup, "first-reset-pool", &json!({})).await;
    let (second_pool, _second_client) =
        pool_with_client(&setup, "second-reset-pool", &json!({})).await;

    // The same usernames exist in both pools, each with a verified address.
    let mut subs = Vec::new();
    for user_pool_id in [&first_pool, &second_pool] {
        for username in ["alice", "bob"] {
            let address = format!("{username}@example.org");
            let sub = admin_user(
                &setup,
                user_pool_id,
                username,
                &[("email", &address), ("email_verified", "true")],
            )
            .await;
            subs.push((user_pool_id.clone(), username, sub));
        }
    }

    // Two resets of the same user: the second mints a fresh code that replaces the first.
    let (status, payload) = admin_reset(&setup, &first_pool, "alice").await;
    assert_eq!(
        status, 200,
        "the first reset must succeed, got {status} with {payload}"
    );
    let first = outstanding_reset_code(&engine, &first_pool, "alice")
        .expect("the first reset leaves a code outstanding")
        .code;
    let (status, payload) = admin_reset(&setup, &first_pool, "alice").await;
    assert_eq!(
        status, 200,
        "the second reset must succeed, got {status} with {payload}"
    );
    let second = outstanding_reset_code(&engine, &first_pool, "alice")
        .expect("the second reset leaves a code outstanding")
        .code;
    assert_ne!(
        second, first,
        "case \"a second reset of the same user\": expected a fresh code, got {second:?} twice"
    );

    // Only the named user of the named pool was reset.
    for (case, user_pool_id, username, expected) in [
        (
            "the reset user of the pool it was reset in",
            &first_pool,
            "alice",
            state::UserStatus::ResetRequired,
        ),
        (
            "another user of the same pool",
            &first_pool,
            "bob",
            state::UserStatus::Confirmed,
        ),
        (
            "the same username in another pool",
            &second_pool,
            "alice",
            state::UserStatus::Confirmed,
        ),
    ] {
        let actual = stored_status(&setup, user_pool_id, username).await;
        assert_eq!(
            actual,
            expected.as_str(),
            "case {case:?}: expected {}, got {actual:?}",
            expected.as_str()
        );
        let code = outstanding_reset_code(&engine, user_pool_id, username);
        assert_eq!(
            code.is_some(),
            matches!(expected, state::UserStatus::ResetRequired),
            "case {case:?}: only the reset user holds a reset code, got {code:?}"
        );
    }

    // `Username` "must be the sub of a local user or the username", so a reset named by
    // the `sub` lands on the same stored user.
    let (user_pool_id, username, sub) = subs
        .iter()
        .find(|(pool, username, _)| pool == &second_pool && *username == "bob")
        .expect("the second pool holds bob")
        .clone();
    let (status, payload) = admin_reset(&setup, &user_pool_id, &sub).await;
    assert_eq!(
        status, 200,
        "case \"a reset named by the user's sub\": expected HTTP 200, got {status} with {payload}"
    );
    let actual = stored_status(&setup, &user_pool_id, username).await;
    assert_eq!(
        actual,
        state::UserStatus::ResetRequired.as_str(),
        "case \"a reset named by the user's sub\": expected RESET_REQUIRED, got {actual:?}"
    );
    assert!(
        outstanding_reset_code(&engine, &user_pool_id, username).is_some(),
        "case \"a reset named by the user's sub\": the stored user holds the reset code"
    );
}

/// RULE: a reset of a pool, a user or a deactivated user the store cannot serve is
/// refused and writes nothing, and a request outside a modelled constraint is a 400 —
/// never the scaffold's 501.
#[tokio::test]
async fn a_reset_the_store_cannot_serve_is_refused_and_writes_nothing() {
    let engine = engine();
    let setup = router(&engine, false);
    let (user_pool_id, _client_id) = pool_with_client(&setup, "refusal-pool", &json!({})).await;
    admin_user(
        &setup,
        &user_pool_id,
        "jane",
        &[("email", "jane@example.com"), ("email_verified", "true")],
    )
    .await;
    admin_user(
        &setup,
        &user_pool_id,
        "kim",
        &[("email", "kim@example.com"), ("email_verified", "true")],
    )
    .await;
    ok(
        &setup,
        "AdminDisableUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "kim" }),
    )
    .await;

    for (case, body, expected) in [
        (
            "an absent user pool",
            json!({ "UserPoolId": "us-east-1_absent", "Username": "jane" }),
            (400, "ResourceNotFoundException"),
        ),
        (
            "a user the pool does not hold",
            json!({ "UserPoolId": user_pool_id.clone(), "Username": "absent" }),
            (400, "UserNotFoundException"),
        ),
        (
            "a deactivated user",
            json!({ "UserPoolId": user_pool_id.clone(), "Username": "kim" }),
            (400, "NotAuthorizedException"),
        ),
    ] {
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminResetUserPassword",
            &body,
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {case:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // Nothing above was written: neither user owes a reset and neither holds a code.
    for username in ["jane", "kim"] {
        let actual = stored_status(&setup, &user_pool_id, username).await;
        assert_eq!(
            actual,
            state::UserStatus::Confirmed.as_str(),
            "case {username:?}: a refused reset writes nothing, got {actual:?}"
        );
        assert!(
            outstanding_reset_code(&engine, &user_pool_id, username).is_none(),
            "case {username:?}: a refused reset sends no code"
        );
    }

    // One value past each published bound of the input
    // (`smithy_query(query="operation",operation="AdminResetUserPassword")`): `Username`
    // length 1..128 and pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, and `UserPoolId` length
    // 1..55 and pattern `^[\w-]+_[0-9a-zA-Z]+$`.
    for (case, body) in [
        (
            "an empty username, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id.clone(), "Username": "" }),
        ),
        (
            "a username of 129 characters, past the length maximum 128",
            json!({ "UserPoolId": user_pool_id.clone(), "Username": "u".repeat(129) }),
        ),
        (
            "a username holding a space, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id.clone(), "Username": "two words" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "Username": "jane" }),
        ),
        (
            "a user pool id of 56 characters, past the length maximum 55",
            json!({
                "UserPoolId": format!("us-east-1_{}", "a".repeat(46)),
                "Username": "jane",
            }),
        ),
        (
            "a request missing the required Username",
            json!({ "UserPoolId": user_pool_id.clone() }),
        ),
    ] {
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminResetUserPassword",
            &body,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside a modelled constraint is a 400, got {status} with \
             {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {case:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // A username at each modelled bound of `UsernameType`, and a non-ASCII one, are
    // served: the constraint check refuses what is OUTSIDE the bound, not the bound.
    for (case, username) in [
        ("a username at the length minimum 1", "u".to_string()),
        ("a username at the length maximum 128", "u".repeat(128)),
        ("a non-ASCII username", "ユーザー①".to_string()),
    ] {
        let address = format!("{}@example.com", username.len());
        admin_user(
            &setup,
            &user_pool_id,
            &username,
            &[("email", &address), ("email_verified", "true")],
        )
        .await;
        let (status, payload) = admin_reset(&setup, &user_pool_id, &username).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {payload}"
        );
        assert!(
            outstanding_reset_code(&engine, &user_pool_id, &username).is_some(),
            "case {case:?}: the reset leaves a code outstanding"
        );
    }
}
