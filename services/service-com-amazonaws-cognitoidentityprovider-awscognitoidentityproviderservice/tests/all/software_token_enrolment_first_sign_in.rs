// SPDX-License-Identifier: CC0-1.0

//! The documented TOTP setup flow through to the FIRST sign-in that presents the
//! authenticator it enrolled, over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.InitiateAuth` / `.AdminInitiateAuth` /
//! `.AssociateSoftwareToken` / `.VerifySoftwareToken` /
//! `.RespondToAuthChallenge` / `.AdminRespondToAuthChallenge` / `.GetUser`).
//!
//! The rule under test: **a time step's software-token one-time password is
//! spent by the challenge response that AUTHENTICATES a sign-in with it, and by
//! nothing else — a call that only proves the authenticator holds the key
//! (`VerifySoftwareToken`, which "marks the user's software token MFA status as
//! verified") spends no step, and neither does the `MFA_SETUP` response that
//! activates the factor — so the first sign-in after an enrolment is accepted
//! with a password from any step of the authenticator's resynchronisation
//! window, the step the registration verified included, and answers an
//! `AuthenticationResult` whose access token resolves that user.** The
//! single-use rule itself is unchanged: a step a completed sign-in spent is
//! refused `ExpiredCodeException` ("thrown if a code has expired",
//! `smithy_query(query="operation",operation="AdminRespondToAuthChallenge")`)
//! the next time it is presented, while `VerifySoftwareToken` declares no
//! expired-code error at all
//! (`smithy_query(query="operation",operation="VerifySoftwareToken")`).
//!
//! The rows differ along which pair of operations drives the sign-in (the
//! client-side pair and the administrative pair) and along which step of the
//! window the presented password comes from: the step the registration verified,
//! and the step before, at and after the instant the code is read — the ±1-step
//! resynchronisation allowance RFC 6238 §6 recommends ("at most one time step is
//! allowed as the network delay").
//!
//! The one-time passwords are computed here from the key the association
//! answered with — the arithmetic an authenticator app provisioned with that key
//! does — so nothing but the service decides whether a code is accepted.
//! Everything else is real: the pool, its app client, the user, the enrolment and
//! every challenge session come from this service's own operations.

use std::time::SystemTime;

use serde_json::{json, Map, Value};

use super::software_token_setup::{
    app_client, current_step, dispatch, emulator, error_code, error_message, ok, ok_self_service,
    one_time_password, pool, Emulator, GOOD_PASSWORD,
};

/// The user every row enrols a factor for and signs in.
const USERNAME: &str = "ada";

/// The `ChallengeNameType` value a user of a pool that requires MFA and holds no
/// factor is answered with, and the one a user with an activated software token
/// is answered with.
const MFA_SETUP: &str = "MFA_SETUP";
const SOFTWARE_TOKEN_MFA: &str = "SOFTWARE_TOKEN_MFA";

/// The `ChallengeResponses` entry the software token's code travels in:
/// `{"SOFTWARE_TOKEN_MFA_CODE": [authenticator_code]}`
/// (`smithy_query(query="doc",shape="AdminRespondToAuthChallengeRequest",member="ChallengeResponses")`).
const CODE_ENTRY: &str = "SOFTWARE_TOKEN_MFA_CODE";

/// Every refusal of this service is answered 400 and names its class in the
/// body, so a row asserting a refusal asserts that status with the code and the
/// sentence.
const REFUSED: u16 = 400;

/// The error class a used-up password earns, and the entirely fixed sentence it
/// carries: nothing of the request or of the stored user is interpolated.
const EXPIRED_CODE: &str = "ExpiredCodeException";
const CODE_ALREADY_USED_MESSAGE: &str = "Your software token has already been used once.";

/// A pool that REQUIRES a factor from every sign-in, offers the software token as
/// the method to set one up with, and holds one user with a permanent password
/// and no factor enrolled yet.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Which pair of operations a row drives: the client-side pair, authorized by the
/// app client alone, and the administrative pair, authorized by IAM credentials.
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

    /// Whether this pair is addressed with a credential scope, as an SDK sends
    /// every administrative operation.
    fn signed(self) -> bool {
        self == Self::Admin
    }

    /// The `AuthFlowType` this pair's password sign-in asks for.
    fn auth_flow(self) -> &'static str {
        match self {
            Self::Client => "USER_PASSWORD_AUTH",
            Self::Admin => "ADMIN_USER_PASSWORD_AUTH",
        }
    }

    /// The request body of a password sign-in through this pair.
    fn sign_in_body(self, fixture: &Fixture) -> Value {
        let mut body = Map::new();
        if self == Self::Admin {
            body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
        }
        body.insert("ClientId".to_string(), json!(fixture.client_id));
        body.insert("AuthFlow".to_string(), json!(self.auth_flow()));
        body.insert(
            "AuthParameters".to_string(),
            json!({ "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD }),
        );
        Value::Object(body)
    }

    /// The request body of a response to `challenge_name` through this pair, with
    /// `responses` merged into the `ChallengeResponses` every challenge names the
    /// user in.
    fn respond_body(
        self,
        fixture: &Fixture,
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

/// What a sign-in or a setup call handed on as its `Session`.
fn session_of(case: &str, payload: &Value) -> String {
    payload["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a session, got {payload}"))
        .to_string()
}

/// Build the [`Fixture`] in a pool named `name`.
async fn enrolment_fixture(emulator: &Emulator, name: &str) -> Fixture {
    let user_pool_id = pool(emulator, name, json!({})).await;
    ok(
        emulator,
        "SetUserPoolMfaConfig",
        &json!({
            "UserPoolId": user_pool_id,
            "MfaConfiguration": "ON",
            "SoftwareTokenMfaConfiguration": { "Enabled": true },
        }),
    )
    .await;
    let client_id = app_client(emulator, &user_pool_id, "enrolment-client", json!({})).await;
    ok(
        emulator,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
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
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// Run the documented setup flow for `fixture`'s user through `driver`: the
/// sign-in that owes `MFA_SETUP`, the association that mints the key, the
/// verification that registers the authenticator, and the `MFA_SETUP` response
/// that completes that sign-in. Answers the key and the time step the
/// verification's own password belonged to.
async fn enrolled(
    emulator: &Emulator,
    driver: Driver,
    fixture: &Fixture,
    case: &str,
) -> (String, u64) {
    let (status, started) = dispatch(
        emulator,
        driver.initiate(),
        &driver.sign_in_body(fixture),
        driver.signed(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} must be served, got {status} with {started}",
        driver.initiate()
    );
    assert_eq!(
        started["ChallengeName"].as_str(),
        Some(MFA_SETUP),
        "case {case:?}: a user holding no factor owes the {MFA_SETUP} challenge, got {started}"
    );
    let challenge_session = session_of(case, &started);

    let associated = ok_self_service(
        emulator,
        "AssociateSoftwareToken",
        &json!({ "Session": challenge_session }),
    )
    .await;
    let secret_code = associated["SecretCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the association reports a key, got {associated}"))
        .to_string();
    let association_session = session_of(case, &associated);

    let enrolment_step = current_step(SystemTime::now());
    let verified = ok_self_service(
        emulator,
        "VerifySoftwareToken",
        &json!({
            "Session": association_session,
            "UserCode": one_time_password(&secret_code, enrolment_step),
        }),
    )
    .await;
    assert_eq!(
        verified["Status"].as_str(),
        Some("SUCCESS"),
        "case {case:?}: the presented password registers the authenticator, got {verified}"
    );
    let setup_session = session_of(case, &verified);

    let (status, completed) = dispatch(
        emulator,
        driver.respond(),
        &driver.respond_body(fixture, MFA_SETUP, &setup_session, json!({})),
        driver.signed(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the setup response must complete the sign-in, got {status} with {completed}"
    );
    assert!(
        completed["AuthenticationResult"]["AccessToken"].is_string(),
        "case {case:?}: the completed setup issues tokens, got {completed}"
    );
    (secret_code, enrolment_step)
}

/// Start a sign-in through `driver` and answer the session of the software-token
/// challenge it owes.
async fn software_token_challenge(
    emulator: &Emulator,
    driver: Driver,
    fixture: &Fixture,
    case: &str,
) -> String {
    let (status, payload) = dispatch(
        emulator,
        driver.initiate(),
        &driver.sign_in_body(fixture),
        driver.signed(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} must be served, got {status} with {payload}",
        driver.initiate()
    );
    assert_eq!(
        payload["ChallengeName"].as_str(),
        Some(SOFTWARE_TOKEN_MFA),
        "case {case:?}: the enrolled factor must be challenged, got {payload}"
    );
    session_of(case, &payload)
}

/// Present `code` for the outstanding software-token challenge `session`.
async fn present(
    emulator: &Emulator,
    driver: Driver,
    fixture: &Fixture,
    session: &str,
    code: &str,
) -> (u16, Value) {
    dispatch(
        emulator,
        driver.respond(),
        &driver.respond_body(
            fixture,
            SOFTWARE_TOKEN_MFA,
            session,
            json!({ CODE_ENTRY: code }),
        ),
        driver.signed(),
    )
    .await
}

/// Which time step of the authenticator the password a row presents comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentedStep {
    /// The very step whose password the registration verified.
    Registration,
    /// The step before the one the code is read in, which the ±1-step
    /// resynchronisation window covers.
    Previous,
    /// The step the code is read in.
    Current,
    /// The step after the one the code is read in, the other half of the window.
    Next,
}

impl PresentedStep {
    /// The time step this row's password belongs to, for an enrolment whose own
    /// password belonged to `enrolment_step`, read at `at`.
    fn step(self, enrolment_step: u64, at: SystemTime) -> u64 {
        match self {
            Self::Registration => enrolment_step,
            Self::Previous => current_step(at).saturating_sub(1),
            Self::Current => current_step(at),
            Self::Next => current_step(at).saturating_add(1),
        }
    }
}

/// Every step the first sign-in after an enrolment can read a password from, and
/// both pairs of operations it can present it through.
const PRESENTED_STEPS: [PresentedStep; 4] = [
    PresentedStep::Registration,
    PresentedStep::Previous,
    PresentedStep::Current,
    PresentedStep::Next,
];

/// The FIRST sign-in that presents a freshly enrolled authenticator completes:
/// the setup spent no one-time password, so every step of the window — the one
/// the registration verified included — still authenticates, and the access
/// token the sign-in answers resolves the user that enrolled the factor.
#[tokio::test]
async fn the_first_sign_in_after_an_enrolment_is_accepted() {
    for driver in [Driver::Client, Driver::Admin] {
        for presented in PRESENTED_STEPS {
            let emulator = emulator();
            let case = format!(
                "the {presented:?} step's password presented to {}",
                driver.respond()
            );
            let fixture = enrolment_fixture(
                &emulator,
                &format!("enrolment-pool-{}-{presented:?}", driver.respond()),
            )
            .await;
            let (secret_code, enrolment_step) = enrolled(&emulator, driver, &fixture, &case).await;

            let session = software_token_challenge(&emulator, driver, &fixture, &case).await;
            let step = presented.step(enrolment_step, SystemTime::now());
            let (status, payload) = present(
                &emulator,
                driver,
                &fixture,
                &session,
                &one_time_password(&secret_code, step),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first sign-in after the enrolment must be served, got {status} \
                 with {payload}"
            );
            let access_token = payload["AuthenticationResult"]["AccessToken"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the completed sign-in issues tokens, got {payload}")
                })
                .to_string();
            for member in ["IdToken", "RefreshToken"] {
                assert!(
                    payload["AuthenticationResult"][member].is_string(),
                    "case {case:?}: the completed sign-in answers a {member}, got {payload}"
                );
            }

            // The token the sign-in issued is the user's own: the self-service
            // read it authorizes names that user.
            let read = ok_self_service(
                &emulator,
                "GetUser",
                &json!({ "AccessToken": access_token }),
            )
            .await;
            assert_eq!(
                read["Username"].as_str(),
                Some(USERNAME),
                "case {case:?}: the issued token must resolve the enrolled user, got {read}"
            );
        }
    }
}

/// The single-use rule is untouched by the enrolment: the step the first sign-in
/// SPENT is refused `ExpiredCodeException` when the next sign-in presents it
/// again, and that refusal is confined to the spent step — a later step of the
/// same authenticator still completes the very session that was refused.
#[tokio::test]
async fn a_step_the_first_sign_in_spent_is_refused_on_the_next_one() {
    for driver in [Driver::Client, Driver::Admin] {
        let emulator = emulator();
        let case = format!(
            "replaying the first sign-in's password to {}",
            driver.respond()
        );
        let fixture = enrolment_fixture(
            &emulator,
            &format!("enrolment-replay-pool-{}", driver.respond()),
        )
        .await;
        let (secret_code, enrolment_step) = enrolled(&emulator, driver, &fixture, &case).await;

        let session = software_token_challenge(&emulator, driver, &fixture, &case).await;
        let (status, payload) = present(
            &emulator,
            driver,
            &fixture,
            &session,
            &one_time_password(&secret_code, enrolment_step),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first sign-in after the enrolment must be served, got {status} \
             with {payload}"
        );

        let session = software_token_challenge(&emulator, driver, &fixture, &case).await;
        let (status, payload) = present(
            &emulator,
            driver,
            &fixture,
            &session,
            &one_time_password(&secret_code, enrolment_step),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (REFUSED, EXPIRED_CODE, CODE_ALREADY_USED_MESSAGE),
            "case {case:?}: the password the first sign-in spent is used up, got {status} with \
             {payload}"
        );
        assert!(
            payload.get("AuthenticationResult").is_none(),
            "case {case:?}: a refused response issues no tokens, got {payload}"
        );

        let (status, completed) = present(
            &emulator,
            driver,
            &fixture,
            &session,
            &one_time_password(&secret_code, enrolment_step + 1),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an unspent step must still answer that session, got {status} with \
             {completed}"
        );
        assert!(
            completed["AuthenticationResult"]["AccessToken"].is_string(),
            "case {case:?}: the completed sign-in issues tokens, got {completed}"
        );
    }
}
