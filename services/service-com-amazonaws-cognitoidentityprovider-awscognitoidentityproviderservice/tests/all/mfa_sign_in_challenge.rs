// SPDX-License-Identifier: CC0-1.0

//! Signing in with a second factor, over the wire, driven exactly as an SDK
//! drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.InitiateAuth` /
//! `.AdminInitiateAuth` / `.RespondToAuthChallenge` /
//! `.AdminRespondToAuthChallenge`).
//!
//! The rule under test: **once the first factor of a sign-in has passed, a
//! sign-in that a second factor applies to does NOT complete — the response
//! carries the `ChallengeNameType` of the factor to present, a `Session` and no
//! `AuthenticationResult`, and the tokens are minted only by the challenge
//! response that presents the right factor.** A second factor applies to every
//! factor the user has activated and holds the delivery configuration of
//! (`UserMFASettingList` is "the MFA options that are activated for the user" —
//! `smithy_query(query="shape",shape="UserType")` and
//! `SetUserMFAPreference`), the preferred one where it has expressed a
//! preference and the choice between them where it has not; it applies to a user
//! with no factor as the challenge to SET one up where the pool requires one;
//! and it applies to no sign-in from a device the user has confirmed and
//! remembers, since `AuthParameters` is where a caller "add[s] `DEVICE_KEY` if
//! you want to bypass multi-factor authentication with a remembered device".
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value. The rows differ along WHICH factor is activated (software
//! token, text message, email message, several at once), along whether the user
//! prefers one, along which pair of operations drives the sign-in (the
//! client-side pair and the administrative pair) and, for the device rows, along
//! what the presented `DEVICE_KEY` names.
//!
//! The one-time passwords the suite presents are computed from the key the
//! association answered with (the arithmetic an authenticator app does) for the
//! software token, and read off the challenge session this service stored for the
//! two message factors — a delivered code never travels in a response, so there
//! is nowhere else to read it. Everything else is real: the pools, app clients,
//! users, enrollments, challenge sessions and devices all come from this
//! service's own operations.

use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};

use cognito_idp::state;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use super::software_token_setup::{
    app_client, current_step, dispatch, emulator, error_code, error_message, ok, ok_self_service,
    one_time_password, pool, Emulator, GOOD_PASSWORD,
};

/// The `UserMFASettingList` values, which each name the `ChallengeNameType` the
/// factor is presented through
/// (`smithy_query(query="shape",shape="ChallengeNameType")`).
const SOFTWARE_TOKEN_MFA: &str = "SOFTWARE_TOKEN_MFA";
const SMS_MFA: &str = "SMS_MFA";
const EMAIL_OTP: &str = "EMAIL_OTP";

/// The challenge that asks which of several activated factors to present, and
/// the one that asks for a factor to be set up.
const SELECT_MFA_TYPE: &str = "SELECT_MFA_TYPE";
const MFA_SETUP: &str = "MFA_SETUP";

/// The `ChallengeParameters` entry naming the user a challenge is outstanding
/// for.
const USER_ID_FOR_SRP: &str = "USER_ID_FOR_SRP";

/// The `ChallengeParameters` entry a delivered challenge reports its medium
/// under.
const CODE_DELIVERY_DELIVERY_MEDIUM: &str = "CODE_DELIVERY_DELIVERY_MEDIUM";

/// The `ChallengeParameters` entry a `SELECT_MFA_TYPE` challenge lists the
/// factors to choose between under.
const MFAS_CAN_CHOOSE: &str = "MFAS_CAN_CHOOSE";

/// The user every row signs in as, and the address and number its message
/// factors are delivered to.
const USERNAME: &str = "ada";
const EMAIL_ADDRESS: &str = "ada@example.com";
const PHONE_NUMBER: &str = "+15555550123";

/// The modelled bounds of a `Session` (`SessionType`: length 20..4096).
const SESSION_LENGTH: (usize, usize) = (20, 4096);

/// A six-digit value that is not the code the challenge asks for.
const WRONG_CODE: &str = "000000";

/// How long a one-time-password presentation keeps being read afresh and presented
/// again, and how long it waits between tries.
///
/// Reading the clock and the service checking the password are two instants, and one
/// time step lasts 30 s, so a step boundary between them leaves the password outside
/// the window the service accepts. That race is the one thing about these fixtures
/// that depends on WHEN they run, so a presentation is repeated from a FRESH reading
/// until it is served or this deadline passes, rather than asserted on the first try
/// — the same bound the sibling `device_authentication` suite presents a password
/// under. The window is several time steps wide, so only a service that never accepts
/// the authenticator's password runs it out.
const PRESENT_CODE_DEADLINE: Duration = Duration::from_secs(150);
const PRESENT_CODE_POLL: Duration = Duration::from_millis(200);

/// Present the one-time password `code_for_step` computes to the self-service
/// `operation`, reading the time step afresh on every try ([`PRESENT_CODE_DEADLINE`]).
async fn presented_until_served(
    emulator: &Emulator,
    operation: &str,
    code_for_step: impl Fn(u64) -> Value,
) -> Value {
    let deadline = SystemTime::now() + PRESENT_CODE_DEADLINE;
    loop {
        let body = code_for_step(current_step(SystemTime::now()));
        let (status, payload) = dispatch(emulator, operation, &body, false).await;
        if status == 200 {
            return payload;
        }
        assert!(
            SystemTime::now() < deadline,
            "{operation} must be served with the authenticator's own one-time password, got \
             {status} with {payload}"
        );
        ::tokio::time::sleep(PRESENT_CODE_POLL).await;
    }
}

/// The message a wrong code is refused with: the software token's own sentence,
/// and for a factor whose code was DELIVERED the wording of the authentication
/// state the response did not match.
const TOTP_MISMATCH_MESSAGE: &str = "Invalid code received for user";
const DELIVERED_MISMATCH_MESSAGE: &str = "Invalid code or auth state for the user.";

/// The message a one-time password the software token already authenticated a
/// sign-in with is refused with.
const CODE_ALREADY_USED_MESSAGE: &str = "Your software token has already been used once.";

/// Which pair of operations drives a sign-in: the client-side pair, authorized
/// by the app client alone, and the administrative pair, authorized by IAM
/// credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Driver {
    Client,
    Admin,
}

impl Driver {
    /// The operation that STARTS the sign-in.
    fn initiate(self) -> &'static str {
        match self {
            Self::Client => "InitiateAuth",
            Self::Admin => "AdminInitiateAuth",
        }
    }

    /// The operation that ANSWERS an outstanding challenge.
    fn respond(self) -> &'static str {
        match self {
            Self::Client => "RespondToAuthChallenge",
            Self::Admin => "AdminRespondToAuthChallenge",
        }
    }

    /// Whether this pair is addressed with a credential scope, as the SDK sends
    /// every administrative operation.
    fn signed(self) -> bool {
        self == Self::Admin
    }

    /// The `AuthFlowType` value this pair's password sign-in asks for.
    fn auth_flow(self) -> &'static str {
        match self {
            Self::Client => "USER_PASSWORD_AUTH",
            Self::Admin => "ADMIN_USER_PASSWORD_AUTH",
        }
    }

    /// The request body of a password sign-in through this pair, with
    /// `parameters` merged into its `AuthParameters`.
    fn sign_in_body(self, fixture: &Enrolled, parameters: Value) -> Value {
        let mut auth_parameters = Map::new();
        auth_parameters.insert("USERNAME".to_string(), json!(USERNAME));
        auth_parameters.insert("PASSWORD".to_string(), json!(GOOD_PASSWORD));
        if let Some(members) = parameters.as_object() {
            for (name, value) in members {
                auth_parameters.insert(name.clone(), value.clone());
            }
        }
        let mut body = Map::new();
        if self == Self::Admin {
            body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
        }
        body.insert("ClientId".to_string(), json!(fixture.client_id));
        body.insert("AuthFlow".to_string(), json!(self.auth_flow()));
        body.insert(
            "AuthParameters".to_string(),
            Value::Object(auth_parameters),
        );
        Value::Object(body)
    }

    /// The request body of a challenge response through this pair.
    fn respond_body(
        self,
        fixture: &Enrolled,
        challenge_name: &str,
        session: &str,
        responses: Value,
    ) -> Value {
        let mut challenge_responses = Map::new();
        challenge_responses.insert("USERNAME".to_string(), json!(USERNAME));
        if let Some(members) = responses.as_object() {
            for (name, value) in members {
                challenge_responses.insert(name.clone(), value.clone());
            }
        }
        let mut body = Map::new();
        if self == Self::Admin {
            body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
        }
        body.insert("ClientId".to_string(), json!(fixture.client_id));
        body.insert("ChallengeName".to_string(), json!(challenge_name));
        body.insert("Session".to_string(), json!(session));
        body.insert(
            "ChallengeResponses".to_string(),
            Value::Object(challenge_responses),
        );
        Value::Object(body)
    }
}

/// A pool with one app client and one user that has REGISTERED a software token
/// and holds a verified address and number, so any of the three factors can be
/// activated for it.
struct Enrolled {
    user_pool_id: String,
    client_id: String,
    /// The private key the association minted, which the authenticator this
    /// suite stands in for is provisioned with.
    secret_code: String,
}

/// Build [`Enrolled`] in a pool named `name`, with `pool_extra` merged into the
/// create.
async fn enrolled(emulator: &Emulator, name: &str, pool_extra: Value) -> Enrolled {
    let user_pool_id = pool(emulator, name, pool_extra).await;
    // The pool offers TOTP as an MFA method; whether a FACTOR applies to a
    // sign-in is the user's own activated settings, which each row sets.
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
    let client_id = app_client(emulator, &user_pool_id, "mfa-client", json!({})).await;
    ok(
        emulator,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "MessageAction": "SUPPRESS",
            "UserAttributes": [
                { "Name": "email", "Value": EMAIL_ADDRESS },
                { "Name": "email_verified", "Value": "true" },
                { "Name": "phone_number", "Value": PHONE_NUMBER },
                { "Name": "phone_number_verified", "Value": "true" },
            ],
        }),
    )
    .await;
    ok(
        emulator,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;

    // The first sign-in completes: no factor is activated yet, which is what
    // lets the user enroll one with the access token it issues.
    let fixture = Enrolled {
        user_pool_id,
        client_id,
        secret_code: String::new(),
    };
    let signed_in = ok_self_service(
        emulator,
        "InitiateAuth",
        &Driver::Client.sign_in_body(&fixture, json!({})),
    )
    .await;
    let access_token = signed_in["AuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("a sign-in with no factor activated issues an access token: {signed_in}")
        })
        .to_string();
    let associated = ok_self_service(
        emulator,
        "AssociateSoftwareToken",
        &json!({ "AccessToken": access_token }),
    )
    .await;
    let secret_code = associated["SecretCode"]
        .as_str()
        .expect("the association reports a key")
        .to_string();
    // The registration presents the one-time password of the time step BEFORE
    // this one, which an authenticator a step behind the server shows and the
    // resynchronisation window accepts: one time step's password is good for one
    // use, and the code a registration consumes is spent, so enrolling with the
    // previous step's code leaves the code each row then presents for a SIGN-IN
    // unspent. The step is read afresh on every try, because a boundary crossed
    // between the reading and the check would leave that password two steps old
    // ([`presented_until_served`]).
    presented_until_served(emulator, "VerifySoftwareToken", |step| {
        json!({
            "AccessToken": access_token,
            "UserCode": one_time_password(&secret_code, step.saturating_sub(1)),
        })
    })
    .await;
    Enrolled {
        secret_code,
        ..fixture
    }
}

/// Activate `settings` as the user's MFA preference.
async fn prefer(emulator: &Emulator, fixture: &Enrolled, settings: &Value) {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
    body.insert("Username".to_string(), json!(USERNAME));
    if let Some(members) = settings.as_object() {
        for (name, value) in members {
            body.insert(name.clone(), value.clone());
        }
    }
    ok(
        emulator,
        "AdminSetUserMFAPreference",
        &Value::Object(body),
    )
    .await;
}

/// The one-time password the outstanding challenge `challenge_name` is answered
/// with: computed from the enrolled key for the software token, and read off the
/// stored challenge session for a factor that was DELIVERED as a message, which
/// no response carries.
fn presented_code(
    emulator: &Emulator,
    fixture: &Enrolled,
    challenge_name: &str,
    session: &str,
) -> String {
    if challenge_name == SOFTWARE_TOKEN_MFA {
        return one_time_password(&fixture.secret_code, current_step(SystemTime::now()));
    }
    let service_state = state::service_state(&*emulator.engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .auth_sessions
        .get(session)
        .and_then(|stored| stored.code.as_ref())
        .map(|code| code.code.clone())
        .unwrap_or_else(|| panic!("a {challenge_name} challenge delivers a code"))
}

/// The `ChallengeResponses` entry the response to `challenge_name` carries its
/// code in (`smithy_query(query="doc",shape="AdminRespondToAuthChallengeRequest",
/// member="ChallengeResponses")`).
fn code_entry(challenge_name: &str) -> &'static str {
    match challenge_name {
        SOFTWARE_TOKEN_MFA => "SOFTWARE_TOKEN_MFA_CODE",
        SMS_MFA => "SMS_MFA_CODE",
        EMAIL_OTP => "EMAIL_OTP_CODE",
        other => panic!("{other} is not a factor this suite presents a code for"),
    }
}

/// Assert that `payload` is an outstanding challenge for `challenge_name` and
/// return its session.
fn challenge_session(case: &str, challenge_name: &str, payload: &Value) -> String {
    assert_eq!(
        payload["ChallengeName"].as_str(),
        Some(challenge_name),
        "case {case:?}: expected ChallengeName={challenge_name}, got {payload}"
    );
    assert!(
        payload.get("AuthenticationResult").is_none(),
        "case {case:?}: an outstanding challenge withholds the tokens, got {payload}"
    );
    let session = payload["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Session, got {payload}"));
    let (low, high) = SESSION_LENGTH;
    assert!(
        (low..=high).contains(&session.chars().count()),
        "case {case:?}: expected a Session inside the modelled length {SESSION_LENGTH:?}, got {} \
         characters",
        session.chars().count()
    );
    assert_eq!(
        payload["ChallengeParameters"][USER_ID_FOR_SRP].as_str(),
        Some(USERNAME),
        "case {case:?}: the challenge names the user it is outstanding for, got {payload}"
    );
    session.to_string()
}

/// Assert that `payload` is a COMPLETED sign-in and return its device key, if the
/// pool reported one for an unrecognised device.
fn completed(case: &str, payload: &Value) -> Option<String> {
    assert!(
        payload.get("ChallengeName").is_none(),
        "case {case:?}: a completed sign-in leaves no challenge outstanding, got {payload}"
    );
    let result = &payload["AuthenticationResult"];
    for member in ["AccessToken", "IdToken", "RefreshToken"] {
        assert!(
            result[member].as_str().is_some_and(|token| !token.is_empty()),
            "case {case:?}: a completed sign-in issues {member}, got {payload}"
        );
    }
    result["NewDeviceMetadata"]["DeviceKey"]
        .as_str()
        .map(ToString::to_string)
}

/// One row of the factor table: the MFA preference it activates and the
/// challenge the next sign-in therefore owes.
struct FactorCase {
    case: &'static str,
    /// The `*MfaSettings` members `AdminSetUserMFAPreference` is given.
    settings: fn() -> Value,
    challenge: &'static str,
    /// Whether the challenge DELIVERS the code it asks for.
    delivered: bool,
}

/// The rows differ along which factor is activated, along whether the user
/// prefers one and along how many are activated at once.
const FACTOR_CASES: &[FactorCase] = &[
    FactorCase {
        case: "a preferred software token",
        settings: || json!({ "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": true } }),
        challenge: SOFTWARE_TOKEN_MFA,
        delivered: false,
    },
    FactorCase {
        case: "a software token activated without a preference",
        settings: || {
            json!({ "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": false } })
        },
        challenge: SOFTWARE_TOKEN_MFA,
        delivered: false,
    },
    FactorCase {
        case: "a preferred text-message factor",
        settings: || json!({ "SMSMfaSettings": { "Enabled": true, "PreferredMfa": true } }),
        challenge: SMS_MFA,
        delivered: true,
    },
    FactorCase {
        case: "a preferred email-message factor",
        settings: || json!({ "EmailMfaSettings": { "Enabled": true, "PreferredMfa": true } }),
        challenge: EMAIL_OTP,
        delivered: true,
    },
    FactorCase {
        case: "a preferred software token beside an activated email factor",
        settings: || {
            json!({
                "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": true },
                "EmailMfaSettings": { "Enabled": true, "PreferredMfa": false },
            })
        },
        challenge: SOFTWARE_TOKEN_MFA,
        delivered: false,
    },
    FactorCase {
        case: "a preferred email factor beside an activated software token",
        settings: || {
            json!({
                "EmailMfaSettings": { "Enabled": true, "PreferredMfa": true },
                "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": false },
            })
        },
        challenge: EMAIL_OTP,
        delivered: true,
    },
];

#[tokio::test]
async fn an_activated_factor_is_challenged_before_any_token_is_issued() {
    for (index, row) in FACTOR_CASES.iter().enumerate() {
        for driver in [Driver::Client, Driver::Admin] {
            let emulator = emulator();
            let case = format!("{} through {}", row.case, driver.initiate());
            let fixture = enrolled(&emulator, &format!("factor-pool-{index}"), json!({})).await;
            prefer(&emulator, &fixture, &(row.settings)()).await;

            // The sign-in stops at the factor the user activated: no token is
            // issued by the call that only checked the password.
            let (status, payload) = dispatch(
                &emulator,
                driver.initiate(),
                &driver.sign_in_body(&fixture, json!({})),
                driver.signed(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the sign-in must be served, got {status} with {payload}"
            );
            let session = challenge_session(&case, row.challenge, &payload);
            assert_eq!(
                payload["ChallengeParameters"][CODE_DELIVERY_DELIVERY_MEDIUM]
                    .as_str()
                    .is_some(),
                row.delivered,
                "case {case:?}: expected a delivery to be reported={}, got {payload}",
                row.delivered
            );

            // The tokens are minted by the response that presents the factor.
            let code = presented_code(&emulator, &fixture, row.challenge, &session);
            let (status, payload) = dispatch(
                &emulator,
                driver.respond(),
                &driver.respond_body(
                    &fixture,
                    row.challenge,
                    &session,
                    json!({ code_entry(row.challenge): code }),
                ),
                driver.signed(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: presenting the factor must complete the sign-in, got {status} \
                 with {payload}"
            );
            completed(&case, &payload);
        }
    }
}

#[tokio::test]
async fn a_wrong_code_is_refused_and_leaves_the_challenge_outstanding() {
    // The rows differ along the factor whose code is presented wrongly, which is
    // also whether the code was delivered or read off an authenticator, and
    // along the sentence each refusal owes.
    let rows: &[(&str, Value, &str, &str)] = &[
        (
            "a wrong software-token code",
            json!({ "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": true } }),
            SOFTWARE_TOKEN_MFA,
            TOTP_MISMATCH_MESSAGE,
        ),
        (
            "a wrong text-message code",
            json!({ "SMSMfaSettings": { "Enabled": true, "PreferredMfa": true } }),
            SMS_MFA,
            DELIVERED_MISMATCH_MESSAGE,
        ),
        (
            "a wrong email-message code",
            json!({ "EmailMfaSettings": { "Enabled": true, "PreferredMfa": true } }),
            EMAIL_OTP,
            DELIVERED_MISMATCH_MESSAGE,
        ),
    ];
    for (index, (case, settings, challenge, message)) in rows.iter().enumerate() {
        for driver in [Driver::Client, Driver::Admin] {
            let emulator = emulator();
            let case = format!("{case} through {}", driver.respond());
            let fixture = enrolled(&emulator, &format!("mismatch-pool-{index}"), json!({})).await;
            prefer(&emulator, &fixture, settings).await;
            let started = ok_self_service_or_admin(&emulator, driver, &fixture).await;
            let session = challenge_session(&case, challenge, &started);

            let right = presented_code(&emulator, &fixture, challenge, &session);
            let wrong = if right == WRONG_CODE {
                "111111".to_string()
            } else {
                WRONG_CODE.to_string()
            };
            let (status, payload) = dispatch(
                &emulator,
                driver.respond(),
                &driver.respond_body(
                    &fixture,
                    challenge,
                    &session,
                    json!({ code_entry(challenge): wrong }),
                ),
                driver.signed(),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, "CodeMismatchException", *message),
                "case {case:?}: a wrong code owes a code mismatch, got {status} with {payload}"
            );

            // The refused response SPENT nothing: the same session still carries
            // the challenge, so the right code completes the sign-in.
            let (status, payload) = dispatch(
                &emulator,
                driver.respond(),
                &driver.respond_body(
                    &fixture,
                    challenge,
                    &session,
                    json!({ code_entry(challenge): right }),
                ),
                driver.signed(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the session must still be answerable with the right code, got \
                 {status} with {payload}"
            );
            completed(&case, &payload);
        }
    }
}

#[tokio::test]
async fn several_activated_factors_ask_the_caller_which_to_present() {
    // The rows differ along which of the two activated factors the caller
    // chooses, including the name the response documents the email factor under.
    let rows: &[(&str, &str, &str)] = &[
        (
            "choosing the software token",
            SOFTWARE_TOKEN_MFA,
            SOFTWARE_TOKEN_MFA,
        ),
        ("choosing the email factor", EMAIL_OTP, EMAIL_OTP),
        (
            "choosing the email factor under the name the response documents",
            "EMAIL_MFA",
            EMAIL_OTP,
        ),
    ];
    for (index, (case, answer, challenge)) in rows.iter().enumerate() {
        for driver in [Driver::Client, Driver::Admin] {
            let emulator = emulator();
            let case = format!("{case} through {}", driver.respond());
            let fixture = enrolled(&emulator, &format!("select-pool-{index}"), json!({})).await;
            // Two factors activated and NEITHER preferred, so the user has
            // expressed no choice and the sign-in asks for one.
            prefer(
                &emulator,
                &fixture,
                &json!({
                    "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": false },
                    "EmailMfaSettings": { "Enabled": true, "PreferredMfa": false },
                }),
            )
            .await;

            let started = ok_self_service_or_admin(&emulator, driver, &fixture).await;
            let session = challenge_session(&case, SELECT_MFA_TYPE, &started);
            let offered = started["ChallengeParameters"][MFAS_CAN_CHOOSE]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the choice lists the factors, got {started}")
                });
            assert_eq!(
                serde_json::from_str::<Value>(offered).ok(),
                Some(json!([EMAIL_OTP, SOFTWARE_TOKEN_MFA])),
                "case {case:?}: expected the activated factors in the order the model documents \
                 them, got {offered:?}"
            );

            // Choosing a factor answers that challenge and starts the chosen
            // factor's own, which is what the tokens then come from.
            let (status, payload) = dispatch(
                &emulator,
                driver.respond(),
                &driver.respond_body(
                    &fixture,
                    SELECT_MFA_TYPE,
                    &session,
                    json!({ "ANSWER": answer }),
                ),
                driver.signed(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the choice must be served, got {status} with {payload}"
            );
            let next = challenge_session(&case, challenge, &payload);
            assert_ne!(
                next, session,
                "case {case:?}: the chosen factor is outstanding on a session of its own"
            );

            let code = presented_code(&emulator, &fixture, challenge, &next);
            let (status, payload) = dispatch(
                &emulator,
                driver.respond(),
                &driver.respond_body(
                    &fixture,
                    challenge,
                    &next,
                    json!({ code_entry(challenge): code }),
                ),
                driver.signed(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: presenting the chosen factor must complete the sign-in, got \
                 {status} with {payload}"
            );
            completed(&case, &payload);
        }
    }
}

/// One row of the device table: what the pool's `DeviceConfiguration` asks of a
/// new device, whether the user's answer makes it remembered, and whether the
/// next sign-in from it still owes the factor.
struct DeviceCase {
    case: &'static str,
    device_configuration: fn() -> Value,
    /// Whether `UpdateDeviceStatus` is asked to remember the confirmed device.
    remember: bool,
    /// Whether the sign-in that presents the device key is still challenged.
    challenged: bool,
}

/// The rows differ along what the presented `DEVICE_KEY` names: a device the user
/// confirmed and remembers, one it confirmed and does not remember, and (below)
/// a key naming no device of this user at all.
const DEVICE_CASES: &[DeviceCase] = &[
    DeviceCase {
        case: "a device the confirm remembered straight away",
        device_configuration: || json!({}),
        remember: false,
        challenged: false,
    },
    DeviceCase {
        case: "a confirmed device the user has not remembered",
        device_configuration: || json!({ "DeviceOnlyRememberedOnUserPrompt": true }),
        remember: false,
        challenged: true,
    },
    DeviceCase {
        case: "a confirmed device the user then remembered",
        device_configuration: || json!({ "DeviceOnlyRememberedOnUserPrompt": true }),
        remember: true,
        challenged: false,
    },
];

#[tokio::test]
async fn only_a_remembered_device_skips_the_factor() {
    for (index, row) in DEVICE_CASES.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let fixture = enrolled(
            &emulator,
            &format!("device-pool-{index}"),
            json!({ "DeviceConfiguration": (row.device_configuration)() }),
        )
        .await;
        prefer(
            &emulator,
            &fixture,
            &json!({ "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": true } }),
        )
        .await;

        // The sign-in from an unknown device is challenged, and the response that
        // presents the factor is what registers the device and reports its key.
        let started = ok_self_service(
            &emulator,
            "InitiateAuth",
            &Driver::Client.sign_in_body(&fixture, json!({})),
        )
        .await;
        let session = challenge_session(case, SOFTWARE_TOKEN_MFA, &started);
        let code = presented_code(&emulator, &fixture, SOFTWARE_TOKEN_MFA, &session);
        let answered = ok_self_service(
            &emulator,
            "RespondToAuthChallenge",
            &Driver::Client.respond_body(
                &fixture,
                SOFTWARE_TOKEN_MFA,
                &session,
                json!({ "SOFTWARE_TOKEN_MFA_CODE": code }),
            ),
        )
        .await;
        let device_key = completed(case, &answered)
            .unwrap_or_else(|| panic!("case {case:?}: expected a new device key, got {answered}"));
        let access_token = answered["AuthenticationResult"]["AccessToken"]
            .as_str()
            .expect("a completed sign-in issues an access token")
            .to_string();

        // A key that names NO device of this user names a device that does not
        // exist, whatever the pool remembers, so the sign-in presenting it is
        // refused rather than falling back on the user's own factor.
        let (status, unknown) = dispatch(
            &emulator,
            "InitiateAuth",
            &Driver::Client.sign_in_body(
                &fixture,
                json!({ "DEVICE_KEY": format!("{device_key}-no-such-device") }),
            ),
            false,
        )
        .await;
        assert_eq!(
            (status, error_code(&unknown), error_message(&unknown)),
            (400, "ResourceNotFoundException", "Device does not exist."),
            "case {case:?}: a key naming no device of this user is refused, got {status} with \
             {unknown}"
        );

        ok_self_service(
            &emulator,
            "ConfirmDevice",
            &json!({
                "AccessToken": access_token,
                "DeviceKey": device_key,
                "DeviceName": "ada's phone",
                "DeviceSecretVerifierConfig": {
                    "PasswordVerifier": "ZGV2aWNlLXBhc3N3b3JkLXZlcmlmaWVy",
                    "Salt": "ZGV2aWNlLXNhbHQ",
                },
            }),
        )
        .await;
        if row.remember {
            ok_self_service(
                &emulator,
                "UpdateDeviceStatus",
                &json!({
                    "AccessToken": access_token,
                    "DeviceKey": device_key,
                    "DeviceRememberedStatus": "remembered",
                }),
            )
            .await;
        }

        let again = ok_self_service(
            &emulator,
            "InitiateAuth",
            &Driver::Client.sign_in_body(&fixture, json!({ "DEVICE_KEY": device_key })),
        )
        .await;
        if row.challenged {
            challenge_session(case, SOFTWARE_TOKEN_MFA, &again);
        } else {
            assert!(
                again.get("ChallengeName").is_none(),
                "case {case:?}: a remembered device bypasses the factor, got {again}"
            );
            completed(case, &again);
        }
    }
}

#[tokio::test]
async fn the_setup_challenge_enrolls_the_factor_and_activates_it() {
    // The rows differ along which pair of operations drives the enrollment.
    for driver in [Driver::Client, Driver::Admin] {
        let emulator = emulator();
        let case = format!("enrolling through {}", driver.respond());
        let user_pool_id = pool(&emulator, "setup-pool", json!({})).await;
        // The pool REQUIRES a factor from every sign-in and offers the software
        // token as the method to set one up with.
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
        ok(
            &emulator,
            "AdminCreateUser",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": USERNAME,
                "MessageAction": "SUPPRESS",
            }),
        )
        .await;
        ok(
            &emulator,
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": USERNAME,
                "Password": GOOD_PASSWORD,
                "Permanent": true,
            }),
        )
        .await;
        let fixture = Enrolled {
            user_pool_id,
            client_id,
            secret_code: String::new(),
        };

        // A user with no factor at all is asked to SET one up.
        let started = ok_self_service_or_admin(&emulator, driver, &fixture).await;
        let session = challenge_session(&case, MFA_SETUP, &started);

        // The setup is carried by the session: the association hands it on and
        // the verification hands on the one that satisfies the challenge.
        let associated = ok_self_service(
            &emulator,
            "AssociateSoftwareToken",
            &json!({ "Session": session }),
        )
        .await;
        let secret_code = associated["SecretCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a key, got {associated}"))
            .to_string();
        let verified = presented_until_served(&emulator, "VerifySoftwareToken", |step| {
            json!({
                "Session": associated["Session"],
                "UserCode": one_time_password(&secret_code, step),
            })
        })
        .await;
        let setup_session = verified["Session"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the verification hands on a session, got {verified}"))
            .to_string();

        let (status, payload) = dispatch(
            &emulator,
            driver.respond(),
            &driver.respond_body(&fixture, MFA_SETUP, &setup_session, json!({})),
            driver.signed(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the setup response must complete the sign-in, got {status} with \
             {payload}"
        );
        completed(&case, &payload);

        // The factor the user set up is ACTIVATED for it, which is what a later
        // read reports and what the next sign-in presents rather than setting up
        // again.
        let read = ok(
            &emulator,
            "AdminGetUser",
            &json!({ "UserPoolId": fixture.user_pool_id, "Username": USERNAME }),
        )
        .await;
        assert_eq!(
            read["UserMFASettingList"],
            json!([SOFTWARE_TOKEN_MFA]),
            "case {case:?}: the factor set up is activated for the user, got {read}"
        );
        assert_eq!(
            read["PreferredMfaSetting"].as_str(),
            Some(SOFTWARE_TOKEN_MFA),
            "case {case:?}: the factor set up is the user's preference, got {read}"
        );
        let again = ok_self_service_or_admin(&emulator, driver, &fixture).await;
        challenge_session(&case, SOFTWARE_TOKEN_MFA, &again);
    }
}

#[tokio::test]
async fn one_software_token_code_authenticates_one_sign_in_only() {
    // The rows differ along which pair of operations presents the code again.
    for driver in [Driver::Client, Driver::Admin] {
        let emulator = emulator();
        let case = format!("replaying a code through {}", driver.respond());
        let fixture = enrolled(&emulator, "replay-pool", json!({})).await;
        prefer(
            &emulator,
            &fixture,
            &json!({ "SoftwareTokenMfaSettings": { "Enabled": true, "PreferredMfa": true } }),
        )
        .await;

        // One time step's one-time password completes one sign-in.
        let started = ok_self_service_or_admin(&emulator, driver, &fixture).await;
        let session = challenge_session(&case, SOFTWARE_TOKEN_MFA, &started);
        let step = current_step(SystemTime::now());
        let code = one_time_password(&fixture.secret_code, step);
        let answered = dispatch(
            &emulator,
            driver.respond(),
            &driver.respond_body(
                &fixture,
                SOFTWARE_TOKEN_MFA,
                &session,
                json!({ "SOFTWARE_TOKEN_MFA_CODE": code }),
            ),
            driver.signed(),
        )
        .await;
        assert_eq!(
            answered.0, 200,
            "case {case:?}: the first sign-in must complete, got {} with {}",
            answered.0, answered.1
        );
        completed(&case, &answered.1);

        // The next sign-in cannot be completed with that same code: it is used
        // up rather than wrong, and presenting it leaves the challenge standing.
        let again = ok_self_service_or_admin(&emulator, driver, &fixture).await;
        let next = challenge_session(&case, SOFTWARE_TOKEN_MFA, &again);
        for attempt in ["the first replay", "a second replay of the same code"] {
            let case = format!("{case}, {attempt}");
            let (status, payload) = dispatch(
                &emulator,
                driver.respond(),
                &driver.respond_body(
                    &fixture,
                    SOFTWARE_TOKEN_MFA,
                    &next,
                    json!({ "SOFTWARE_TOKEN_MFA_CODE": code }),
                ),
                driver.signed(),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, "ExpiredCodeException", CODE_ALREADY_USED_MESSAGE),
                "case {case:?}: a used code owes an expired code, got {status} with {payload}"
            );
        }

        // The session the replays were presented to is still outstanding, so the
        // next step's code — the one the authenticator shows next — completes it.
        let (status, payload) = dispatch(
            &emulator,
            driver.respond(),
            &driver.respond_body(
                &fixture,
                SOFTWARE_TOKEN_MFA,
                &next,
                json!({
                    "SOFTWARE_TOKEN_MFA_CODE": one_time_password(&fixture.secret_code, step + 1),
                }),
            ),
            driver.signed(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the next code must complete the outstanding challenge, got {status} \
             with {payload}"
        );
        completed(&case, &payload);
    }
}

/// Start the password sign-in of `fixture` through `driver`, requiring success,
/// and return the answer.
async fn ok_self_service_or_admin(
    emulator: &Emulator,
    driver: Driver,
    fixture: &Enrolled,
) -> Value {
    let (status, payload) = dispatch(
        emulator,
        driver.initiate(),
        &driver.sign_in_body(fixture, json!({})),
        driver.signed(),
    )
    .await;
    assert_eq!(
        status, 200,
        "the sign-in through {} must be served, got {status} with {payload}",
        driver.initiate()
    );
    payload
}
