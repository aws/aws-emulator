// SPDX-License-Identifier: CC0-1.0

//! Setting a user's time-based one-time password (TOTP) authenticator up over the
//! wire, driven exactly as an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.AssociateSoftwareToken` /
//! `.VerifySoftwareToken`).
//!
//! The rule under test: **a software-token call is authorized by exactly ONE of
//! the two credentials the request may carry — "you can authorize an
//! `AssociateSoftwareToken` request with either the user's access token, or a
//! session string from a challenge response that you received from Amazon
//! Cognito" — `AssociateSoftwareToken` mints a fresh private key for the user that
//! credential names, STORES it as the key that user's authenticator is
//! provisioned with and reports it as `SecretCode`, and `VerifySoftwareToken`
//! registers the authenticator exactly when the `UserCode` presented is one that
//! stored key emits.** A call the session flow authorized is handed a session that
//! carries the setup forward; one an access token authorized is handed none.
//!
//! What a successful setup records is read back through `GetUser` and
//! `GetUserAuthFactors` — the registration is what lets `SetUserMFAPreference`
//! activate `SOFTWARE_TOKEN_MFA` at all, since a user "must register a TOTP
//! authenticator before they set this as their preferred MFA method"
//! (`SetUserMFAPreference.SoftwareTokenMfaSettings` in this service's model) — so
//! each row asserts what a later caller sees rather than the answer of the call
//! that wrote it.
//!
//! The suite computes the one-time passwords it presents itself, from RFC 6238's
//! definition over the key the association answered with — the arithmetic the
//! authenticator app that was provisioned with that key does — so a wrong
//! algorithm in the service cannot be the reason a row passes.
//!
//! The pools, app clients, users, access tokens and challenge sessions are all
//! real: they come from `CreateUserPool`, `SetUserPoolMfaConfig`,
//! `CreateUserPoolClient`, `AdminCreateUser`, `AdminSetUserPassword`,
//! `InitiateAuth` and `GlobalSignOut`. The one session no operation can hand out —
//! one whose validity window has already closed — is planted in this service's
//! state.
//!
//! Each table row names the case it asserts and every assertion reports the actual
//! value.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;

/// A request's SigV4 credential scope is what routes an administrative call to
/// this service. The signature is never checked here: the suite turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The emulator's own endpoint: an address, so an unsigned request addressed to it
/// names no service in its `Host` header and `X-Amz-Target` is the only routing
/// signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The `UserMFASettingList` value TOTP MFA is reported under: "the possible values
/// in this list are `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`" (this
/// service's model).
const SOFTWARE_TOKEN_MFA: &str = "SOFTWARE_TOKEN_MFA";

/// The `AuthFactorType` value a registered software token offers
/// (`smithy_query(query="shape", shape="AuthFactorType")`).
const SOFTWARE_TOKEN: &str = "SOFTWARE_TOKEN";

/// The `ChallengeNameType` value a user that must set a factor up is answered
/// with.
const MFA_SETUP: &str = "MFA_SETUP";

/// The modelled bounds of a `SecretCode`
/// (`smithy_query(query="operation",operation="AssociateSoftwareToken")`: length
/// 16.., pattern `^[A-Za-z0-9]+$`), which the key an association reports has to
/// satisfy.
const SECRET_CODE_MIN_LENGTH: usize = 16;

/// The modelled bounds of a `Session` (`SessionType`: length 20..4096).
const SESSION_LENGTH: (usize, usize) = (20, 4096);

/// How long one time step of a TOTP one-time password lasts: RFC 6238's
/// recommended default, which is what an authenticator provisioned with a shared
/// secret alone counts in.
const TIME_STEP: Duration = Duration::from_secs(30);

/// How many digits one one-time password carries: `SoftwareTokenMFAUserCodeType`
/// is length 6..6 of `^[0-9]+$` (this service's model).
const CODE_DIGITS: usize = 6;

/// The emulator under test and the engine its state is reached through.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

/// One emulator with just this service mounted.
fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    Emulator { engine, router }
}

/// Dispatch `operation` with `body` and return its status and JSON response body.
/// A `signed` call carries a credential scope, as the SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a signed-in user's client calls the self-service half of
/// this API.
async fn dispatch(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
    signed: bool,
) -> (u16, Value) {
    let host = if signed { ENDPOINT_HOST } else { EMULATOR_HOST };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        );
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
    let response = emulator
        .router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
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

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(emulator, operation, body, true).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Call the unsigned self-service `operation`, requiring success, and return its
/// response body.
async fn ok_self_service(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(emulator, operation, body, false).await;
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

/// Create a pool named `name`, with `extra` merged into the request, and return its
/// minted id.
async fn pool(emulator: &Emulator, name: &str, extra: Value) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body.insert(name.clone(), value.clone());
        }
    }
    let created = ok(emulator, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client of `user_pool_id` that allows the password sign-in flows,
/// with `extra` merged in, and return its minted id.
async fn app_client(emulator: &Emulator, user_pool_id: &str, name: &str, extra: Value) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(name));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!([
            "ALLOW_USER_PASSWORD_AUTH",
            "ALLOW_ADMIN_USER_PASSWORD_AUTH",
            "ALLOW_REFRESH_TOKEN_AUTH"
        ]),
    );
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body.insert(name.clone(), value.clone());
        }
    }
    let created = ok(emulator, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` in `user_pool_id` with a permanent password.
async fn user(emulator: &Emulator, user_pool_id: &str, username: &str) {
    ok(
        emulator,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        emulator,
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

/// Sign `username` in through `client_id` and return the whole sign-in response.
async fn sign_in(emulator: &Emulator, client_id: &str, username: &str) -> Value {
    let (status, payload) = dispatch(
        emulator,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "signing {username} in must be served, got {status} with {payload}"
    );
    payload
}

/// A pool with one app client and one signed-in user, whose sign-in COMPLETED.
struct SignedIn {
    user_pool_id: String,
    username: String,
    access_token: String,
    id_token: String,
}

/// A pool whose MFA is optional with TOTP offered, holding one signed-in user.
async fn signed_in(emulator: &Emulator, name: &str) -> SignedIn {
    let user_pool_id = pool(emulator, name, json!({})).await;
    ok(
        emulator,
        "SetUserPoolMfaConfig",
        &json!({
            "UserPoolId": user_pool_id,
            "MfaConfiguration": "OPTIONAL",
            "SoftwareTokenMfaConfiguration": { "Enabled": true },
        }),
    )
    .await;
    let client_id = app_client(emulator, &user_pool_id, "totp-client", json!({})).await;
    let username = "ada";
    user(emulator, &user_pool_id, username).await;
    let payload = sign_in(emulator, &client_id, username).await;
    let result = &payload["AuthenticationResult"];
    SignedIn {
        user_pool_id,
        username: username.to_string(),
        access_token: result["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a completed sign-in issues an access token: {payload}"))
            .to_string(),
        id_token: result["IdToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a completed sign-in issues an id token: {payload}"))
            .to_string(),
    }
}

/// The time step the instant `at` falls in: the TOTP counter.
fn current_step(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs()
        / TIME_STEP.as_secs()
}

/// The one-time password an authenticator provisioned with `secret_code` emits in
/// the time step `step`, computed here rather than asked of the service (see the
/// module docs): RFC 6238's TOTP over RFC 4226's HOTP, HMAC-SHA-1 of the counter,
/// dynamically truncated and taken to six digits.
fn one_time_password(secret_code: &str, step: u64) -> String {
    use hmac::Mac;

    let key = data_encoding::BASE32_NOPAD
        .decode(secret_code.trim_end_matches('=').as_bytes())
        .unwrap_or_else(|error| {
            panic!("a reported key must be base32, got {secret_code:?}: {error}")
        });
    let mut mac =
        hmac::Hmac::<sha1::Sha1>::new_from_slice(&key).expect("HMAC takes a key of any length");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let truncated = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:0width$}", truncated % 1_000_000, width = CODE_DIGITS)
}

/// Assert that `secret_code` satisfies the modelled `SecretCodeType` and is a key
/// an authenticator can be provisioned with.
fn assert_usable_key(case: &str, secret_code: &str) {
    assert!(
        secret_code.chars().count() >= SECRET_CODE_MIN_LENGTH,
        "case {case:?}: expected a key of at least {SECRET_CODE_MIN_LENGTH} characters, got {} \
         in {secret_code:?}",
        secret_code.chars().count()
    );
    assert!(
        secret_code.chars().all(char::is_alphanumeric) && secret_code.is_ascii(),
        "case {case:?}: expected a key of ASCII letters and digits, got {secret_code:?}"
    );
    assert!(
        data_encoding::BASE32_NOPAD
            .decode(secret_code.as_bytes())
            .is_ok(),
        "case {case:?}: expected a key an authenticator can read as base32, got {secret_code:?}"
    );
}

/// Assert that a session is inside the modelled `SessionType` bounds and return
/// it.
fn session_of(case: &str, payload: &Value) -> String {
    let session = payload["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a session, got {payload}"));
    let (low, high) = SESSION_LENGTH;
    assert!(
        (low..=high).contains(&session.chars().count()),
        "case {case:?}: expected a session inside the modelled length {SESSION_LENGTH:?}, got {} \
         characters",
        session.chars().count()
    );
    session.to_string()
}

/// Begin the setup for the credential `body` carries, requiring success, and
/// return the key and the session it answered.
async fn associate(emulator: &Emulator, case: &str, body: &Value) -> (String, Option<String>) {
    let payload = ok_self_service(emulator, "AssociateSoftwareToken", body).await;
    let secret_code = payload["SecretCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SecretCode, got {payload}"))
        .to_string();
    assert_usable_key(case, &secret_code);
    let session = payload
        .get("Session")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (secret_code, session)
}

/// Ask `GetUser` what MFA settings the user `access_token` names holds: the
/// activated settings and the preferred one.
async fn held_settings(emulator: &Emulator, access_token: &str) -> (Vec<String>, Option<String>) {
    let payload = ok_self_service(emulator, "GetUser", &json!({ "AccessToken": access_token })).await;
    let settings = payload["UserMFASettingList"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let preferred = payload
        .get("PreferredMfaSetting")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (settings, preferred)
}

/// Ask `GetUserAuthFactors` which factors the user `access_token` names has
/// configured.
async fn configured_factors(emulator: &Emulator, access_token: &str) -> Vec<String> {
    let payload = ok_self_service(
        emulator,
        "GetUserAuthFactors",
        &json!({ "AccessToken": access_token }),
    )
    .await;
    payload["ConfiguredUserAuthFactors"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Ask the user `access_token` names to activate TOTP MFA as its preferred factor,
/// and return the status and body of the answer.
async fn prefer_software_token(emulator: &Emulator, access_token: &str) -> (u16, Value) {
    dispatch(
        emulator,
        "SetUserMFAPreference",
        &json!({
            "AccessToken": access_token,
            "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": true },
        }),
        false,
    )
    .await
}

#[tokio::test]
async fn an_association_reports_a_fresh_usable_key_per_call() {
    let emulator = emulator();
    let first_user = signed_in(&emulator, "association-pool-one").await;
    let second_user = {
        // A SECOND user of the same pool, so the keys of two callers are shown to
        // differ as well as two keys of one caller.
        let client_id = app_client(
            &emulator,
            &first_user.user_pool_id,
            "second-client",
            json!({}),
        )
        .await;
        user(&emulator, &first_user.user_pool_id, "grace").await;
        let payload = sign_in(&emulator, &client_id, "grace").await;
        payload["AuthenticationResult"]["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a completed sign-in issues an access token: {payload}"))
            .to_string()
    };

    // (case, the access token the call carries)
    let rows: &[(&str, &str)] = &[
        ("the first association of one user", &first_user.access_token),
        (
            "a second association of the same user",
            &first_user.access_token,
        ),
        ("an association of another user", &second_user),
    ];
    let mut reported: Vec<String> = Vec::new();
    for (case, access_token) in rows {
        let (secret_code, session) = associate(
            &emulator,
            case,
            &json!({ "AccessToken": access_token }),
        )
        .await;
        assert!(
            session.is_none(),
            "case {case:?}: a call an access token authorized answers no session, got \
             {session:?}"
        );
        assert!(
            !reported.contains(&secret_code),
            "case {case:?}: expected a key no earlier call reported, got {secret_code:?} again"
        );
        reported.push(secret_code);
    }
}

#[tokio::test]
async fn a_verified_code_registers_the_authenticator_and_activates_totp_mfa() {
    let emulator = emulator();
    let signed_in = signed_in(&emulator, "registration-pool").await;
    let case = "a code the associated key emits";

    // Before any registration the user holds no TOTP delivery configuration, so
    // activating the factor is refused: the association alone registers nothing.
    let (secret_code, _) = associate(
        &emulator,
        case,
        &json!({ "AccessToken": signed_in.access_token }),
    )
    .await;
    let (status, payload) = prefer_software_token(&emulator, &signed_in.access_token).await;
    assert_eq!(
        status, 400,
        "case {case:?}: activating TOTP MFA before the authenticator is registered must be \
         refused, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {payload}"
    );

    let code = one_time_password(&secret_code, current_step(SystemTime::now()));
    let verified = ok_self_service(
        &emulator,
        "VerifySoftwareToken",
        &json!({
            "AccessToken": signed_in.access_token,
            "UserCode": code,
            "FriendlyDeviceName": "ada's phone",
        }),
    )
    .await;
    assert_eq!(
        verified["Status"].as_str(),
        Some("SUCCESS"),
        "case {case:?}: expected the verification to report SUCCESS, got {verified}"
    );
    assert!(
        verified.get("Session").is_none(),
        "case {case:?}: a call an access token authorized answers no session, got {verified}"
    );

    // The registration is what a later caller sees: the factor can now be
    // activated, and the activation is read back through the self-service reads.
    let (status, payload) = prefer_software_token(&emulator, &signed_in.access_token).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a registered authenticator must let TOTP MFA be activated, got {status} \
         with {payload}"
    );
    let (settings, preferred) = held_settings(&emulator, &signed_in.access_token).await;
    assert_eq!(
        settings,
        vec![SOFTWARE_TOKEN_MFA.to_string()],
        "case {case:?}: expected the activated settings [{SOFTWARE_TOKEN_MFA}], got {settings:?}"
    );
    assert_eq!(
        preferred.as_deref(),
        Some(SOFTWARE_TOKEN_MFA),
        "case {case:?}: expected {SOFTWARE_TOKEN_MFA} preferred, got {preferred:?}"
    );
    let factors = configured_factors(&emulator, &signed_in.access_token).await;
    assert!(
        factors.contains(&SOFTWARE_TOKEN.to_string()),
        "case {case:?}: expected {SOFTWARE_TOKEN} among the configured factors, got {factors:?}"
    );
    assert_eq!(
        signed_in.username, "ada",
        "the suite registers the authenticator of the user it signed in, got {:?}",
        signed_in.username
    );
}

/// One row of the presented-code table: which code the verification presents, and
/// whether the authenticator is registered by it.
struct CodeCase {
    case: &'static str,
    /// The time step the presented code is taken from, relative to the current
    /// one, or `None` for a row that presents a code of the key the association
    /// REPLACED.
    step_offset: Option<i64>,
    registered: bool,
}

/// The rows differ along WHICH code is presented: this time step's, the
/// neighbouring steps the resynchronisation allowance of RFC 6238 covers ("at most
/// one time step is allowed as the network delay"), steps far outside it, and a
/// code of the key a later association replaced.
const CODE_CASES: &[CodeCase] = &[
    CodeCase {
        case: "the code of this time step",
        step_offset: Some(0),
        registered: true,
    },
    CodeCase {
        case: "the code of the step just past, inside the drift allowance",
        step_offset: Some(-1),
        registered: true,
    },
    CodeCase {
        case: "the code of the step just ahead, inside the drift allowance",
        step_offset: Some(1),
        registered: true,
    },
    CodeCase {
        case: "a code from far behind the drift allowance",
        step_offset: Some(-50),
        registered: false,
    },
    CodeCase {
        case: "a code from far ahead of the drift allowance",
        step_offset: Some(50),
        registered: false,
    },
    CodeCase {
        case: "a code of the key a later association replaced",
        step_offset: None,
        registered: false,
    },
];

#[tokio::test]
async fn only_a_code_the_stored_key_emits_registers_the_authenticator() {
    for (index, row) in CODE_CASES.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let signed_in = signed_in(&emulator, &format!("code-pool-{index}")).await;
        let (superseded, _) = associate(
            &emulator,
            case,
            &json!({ "AccessToken": signed_in.access_token }),
        )
        .await;
        // The association the row verifies against: the LAST one, whose key is the
        // one the user holds.
        let (secret_code, _) = associate(
            &emulator,
            case,
            &json!({ "AccessToken": signed_in.access_token }),
        )
        .await;
        assert_ne!(
            superseded, secret_code,
            "case {case:?}: each association mints its own key, got {secret_code:?} twice"
        );
        let step = current_step(SystemTime::now());
        let code = match row.step_offset {
            Some(offset) => one_time_password(&secret_code, step.saturating_add_signed(offset)),
            None => one_time_password(&superseded, step),
        };

        let (status, payload) = dispatch(
            &emulator,
            "VerifySoftwareToken",
            &json!({ "AccessToken": signed_in.access_token, "UserCode": code }),
            false,
        )
        .await;
        if row.registered {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the code to register the authenticator, got {status} \
                 with {payload}"
            );
            assert_eq!(
                payload["Status"].as_str(),
                Some("SUCCESS"),
                "case {case:?}: expected SUCCESS, got {payload}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the code to be refused, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "EnableSoftwareTokenMFAException",
                "case {case:?}: expected EnableSoftwareTokenMFAException, got {payload}"
            );
        }

        // Whether the authenticator is registered is what the activation of TOTP
        // MFA rests on, so the row reads the registration back through it.
        let (status, payload) = prefer_software_token(&emulator, &signed_in.access_token).await;
        let activated = status == 200;
        assert_eq!(
            activated, row.registered,
            "case {case:?}: expected the authenticator registered = {}, and activating TOTP MFA \
             answered {status} with {payload}",
            row.registered
        );
    }
}

#[tokio::test]
async fn the_session_flow_carries_the_setup_from_the_challenge_to_the_response() {
    let emulator = emulator();
    let case = "a sign-in challenged to set a factor up";
    let user_pool_id = pool(&emulator, "mfa-setup-pool", json!({})).await;
    ok(
        &emulator,
        "SetUserPoolMfaConfig",
        &json!({
            "UserPoolId": user_pool_id,
            "MfaConfiguration": "ON",
            "SoftwareTokenMfaConfiguration": { "Enabled": true },
        }),
    )
    .await;
    let client_id = app_client(&emulator, &user_pool_id, "setup-client", json!({})).await;
    user(&emulator, &user_pool_id, "ada").await;

    let challenge = sign_in(&emulator, &client_id, "ada").await;
    assert_eq!(
        challenge["ChallengeName"].as_str(),
        Some(MFA_SETUP),
        "case {case:?}: expected the {MFA_SETUP} challenge, got {challenge}"
    );
    let challenge_session = session_of(case, &challenge);

    let (secret_code, association_session) = associate(
        &emulator,
        case,
        &json!({ "Session": challenge_session }),
    )
    .await;
    let association_session = association_session.unwrap_or_else(|| {
        panic!("case {case:?}: a call the session flow authorized answers a session")
    });
    assert_ne!(
        association_session, challenge_session,
        "case {case:?}: expected a session of its own, got the one the challenge carried"
    );
    let (low, high) = SESSION_LENGTH;
    assert!(
        (low..=high).contains(&association_session.chars().count()),
        "case {case:?}: expected a session inside the modelled length {SESSION_LENGTH:?}, got {} \
         characters",
        association_session.chars().count()
    );

    // The session the association handed out is one this service HOLDS: it
    // resolves the same user, so the setup can be carried on with it.
    let code = one_time_password(&secret_code, current_step(SystemTime::now()));
    let verified = ok_self_service(
        &emulator,
        "VerifySoftwareToken",
        &json!({ "Session": association_session, "UserCode": code }),
    )
    .await;
    assert_eq!(
        verified["Status"].as_str(),
        Some("SUCCESS"),
        "case {case:?}: expected the verification to report SUCCESS, got {verified}"
    );
    let setup_session = session_of(case, &verified);
    assert_ne!(
        setup_session, association_session,
        "case {case:?}: expected a session of its own, got the one the association answered"
    );

    // The registration the session flow wrote is the same fact the access-token
    // flow writes, so the administrative surface can now activate the factor for
    // this user, and the pool's own read of the user reports the activation.
    ok(
        &emulator,
        "AdminSetUserMFAPreference",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "ada",
            "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": true },
        }),
    )
    .await;
    let read = ok(
        &emulator,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "ada" }),
    )
    .await;
    let settings = read["UserMFASettingList"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    assert_eq!(
        settings,
        vec![SOFTWARE_TOKEN_MFA.to_string()],
        "case {case:?}: expected the activated settings [{SOFTWARE_TOKEN_MFA}], got {settings:?}"
    );
    assert_eq!(
        read["PreferredMfaSetting"].as_str(),
        Some(SOFTWARE_TOKEN_MFA),
        "case {case:?}: expected {SOFTWARE_TOKEN_MFA} preferred, got {read}"
    );
}

/// Which credential a refusal row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Credential {
    /// Neither member: the request names no user at all.
    None,
    /// Both members, each of which identifies a user on its own.
    Both,
    /// A value this emulator never issued as an access token.
    UnknownAccessToken,
    /// The id token of a completed sign-in, which is not the access token the
    /// member asks for.
    IdToken,
    /// An access token a global sign-out has revoked.
    RevokedAccessToken,
    /// A value this emulator never handed out as a session.
    UnknownSession,
    /// A stored session whose validity window has already closed.
    ExpiredSession,
}

/// One row of the credential table.
struct CredentialCase {
    case: &'static str,
    credential: Credential,
    /// The wire error code the refusal carries.
    code: &'static str,
    /// The sentence it carries.
    message: &'static str,
}

/// The status this endpoint answers every refusal with, whatever `@httpError` the
/// model annotates the error shape with: the rule [`super::error_wire`] states for
/// this endpoint, which the sibling self-service suite asserts.
const REFUSED: u16 = 400;

/// The rows differ along WHICH credential the request carries: none, both, an
/// access token that resolves to no signed-in user (three ways), and a session
/// that resolves no outstanding challenge (two ways). Every sentence is the one
/// this service already owes that class of credential elsewhere in its
/// self-service half.
const CREDENTIAL_CASES: &[CredentialCase] = &[
    CredentialCase {
        case: "neither an access token nor a session",
        credential: Credential::None,
        code: "InvalidParameterException",
        message: "Invalid request. You can provide either an access token or a session ID in the \
                  request.",
    },
    CredentialCase {
        case: "both an access token and a session",
        credential: Credential::Both,
        code: "InvalidParameterException",
        message: "Invalid request. You can provide either an access token or a session ID in the \
                  request.",
    },
    CredentialCase {
        case: "an access token this service never issued",
        credential: Credential::UnknownAccessToken,
        code: "NotAuthorizedException",
        message: "Invalid Access Token",
    },
    CredentialCase {
        case: "the id token of the sign-in rather than its access token",
        credential: Credential::IdToken,
        code: "NotAuthorizedException",
        message: "Invalid Access Token",
    },
    CredentialCase {
        case: "an access token a global sign-out revoked",
        credential: Credential::RevokedAccessToken,
        code: "NotAuthorizedException",
        message: "Access Token has been revoked",
    },
    CredentialCase {
        case: "a session this service never handed out",
        credential: Credential::UnknownSession,
        code: "NotAuthorizedException",
        message: "Invalid session for the user.",
    },
    CredentialCase {
        case: "a session whose validity window has closed",
        credential: Credential::ExpiredSession,
        code: "NotAuthorizedException",
        message: "Invalid session for the user, session is expired.",
    },
];

/// A session value no operation of this service handed out, inside the modelled
/// `SessionType` length.
const FOREIGN_SESSION: &str = "a-session-this-emulator-never-issued-0001";

/// Plant a challenge session for `username` whose validity window closed before
/// this call: no operation hands out a session that is already past its window, and
/// waiting for a real one to expire would tie the suite to the wall clock.
fn planted_expired_session(
    emulator: &Emulator,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> String {
    let service_state = state::service_state(&*emulator.engine).expect("this service's state");
    let mut guard = service_state.lock();
    let expired_at = SystemTime::now() - Duration::from_secs(60);
    guard.start_session(
        expired_at - Duration::from_secs(60),
        state::AuthSession {
            user_pool_id: user_pool_id.to_string(),
            client_id: client_id.to_string(),
            username: username.to_string(),
            challenge_name: MFA_SETUP.to_string(),
            expires_at: expired_at,
            code: None,
            spent: false,
        },
    )
}

#[tokio::test]
async fn both_software_token_operations_refuse_the_same_credentials() {
    for (index, row) in CREDENTIAL_CASES.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let signed_in = signed_in(&emulator, &format!("credential-pool-{index}")).await;
        let client_id = app_client(
            &emulator,
            &signed_in.user_pool_id,
            "credential-client",
            json!({}),
        )
        .await;
        if row.credential == Credential::RevokedAccessToken {
            ok_self_service(
                &emulator,
                "GlobalSignOut",
                &json!({ "AccessToken": signed_in.access_token }),
            )
            .await;
        }
        // Both operations resolve their caller by the same rule, so both owe the
        // same refusal: the verification carries a code as well, which it never
        // gets as far as checking. The credentials are built per operation because
        // a session the clock has already expired is SWEPT by the call that reads
        // it — the expiry was owed at that instant — so the second operation is
        // handed one of its own rather than the remains of the first one's.
        for operation in ["AssociateSoftwareToken", "VerifySoftwareToken"] {
            let credentials = match row.credential {
                Credential::None => json!({}),
                Credential::Both => json!({
                    "AccessToken": signed_in.access_token,
                    "Session": FOREIGN_SESSION,
                }),
                Credential::UnknownAccessToken => json!({ "AccessToken": "not.a.token" }),
                Credential::IdToken => json!({ "AccessToken": signed_in.id_token }),
                Credential::RevokedAccessToken => {
                    json!({ "AccessToken": signed_in.access_token })
                }
                Credential::UnknownSession => json!({ "Session": FOREIGN_SESSION }),
                Credential::ExpiredSession => json!({
                    "Session": planted_expired_session(
                        &emulator,
                        &signed_in.user_pool_id,
                        &client_id,
                        &signed_in.username,
                    ),
                }),
            };
            let mut body = credentials.as_object().cloned().unwrap_or_default();
            if operation == "VerifySoftwareToken" {
                body.insert("UserCode".to_string(), json!("000000"));
            }
            let (status, payload) =
                dispatch(&emulator, operation, &Value::Object(body), false).await;
            assert_eq!(
                status, REFUSED,
                "case {case:?}: expected {operation} to answer {REFUSED}, got {status} with \
                 {payload}"
            );
            assert_eq!(
                error_code(&payload),
                row.code,
                "case {case:?}: expected {operation} to answer {}, got {payload}",
                row.code
            );
            assert_eq!(
                error_message(&payload),
                row.message,
                "case {case:?}: expected {operation} to report {:?}, got {:?}",
                row.message,
                error_message(&payload)
            );
        }
    }
}
