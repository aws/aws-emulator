// SPDX-License-Identifier: CC0-1.0

//! WHICH missing precondition an MFA activation is refused for, over the wire,
//! driven exactly as an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.SetUserMFAPreference` /
//! `.AdminSetUserMFAPreference`).
//!
//! The rule under test: **activating an MFA factor for a user needs both the
//! POOL's configuration that turns the factor on and the USER's own enrolment in
//! it, and the `InvalidParameterException` (400) a request asking to activate a
//! factor the user cannot use is refused with reports the precondition that is
//! missing: the delivery configuration the factor would reach the user through,
//! naming the `UserMFASettingList` setting the request asked for, or — where the
//! pool DOES turn software token MFA on and only the user's own authenticator is
//! unverified — that verification, whose sentence names nothing at all.** Both
//! sentences are fixed text apart from that one setting name, and neither path
//! stores anything.
//!
//! The rows span what the rule generalises over: the pool's software-token
//! configuration (never configured, configured off, configured on), how far the
//! user's enrolment got (nothing, `AssociateSoftwareToken` only, a verified
//! authenticator), whether the request also asks for the factor to be preferred,
//! and which half of the preference surface the request arrives on — the
//! token-addressed one and the administrative one, which read the same settings.
//! A second table keeps the other two `UserMFASettingList` settings on the
//! delivery-configuration sentence in a pool that offers the software token, so
//! the per-factor sentence is not traded for the verification one.
//!
//! The pools, app clients, users, access tokens, associations and verifications
//! are all real: they come from `CreateUserPool`, `SetUserPoolMfaConfig`,
//! `CreateUserPoolClient`, `AdminCreateUser`, `AdminSetUserPassword`,
//! `InitiateAuth`, `AssociateSoftwareToken` and `VerifySoftwareToken`, and the
//! one-time passwords are computed here from RFC 6238's definition
//! (`crate::software_token_setup::one_time_password`), so no row asks the service
//! for the code it presents. What a call left behind is read back through
//! `GetUser`.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value.

use std::time::SystemTime;

use serde_json::{json, Map, Value};

use super::software_token_setup::{
    GOOD_PASSWORD, Emulator, app_client, current_step, dispatch, emulator, error_code,
    error_message, ok, ok_self_service, one_time_password, pool,
};

/// The user every row sets a preference for.
const USERNAME: &str = "ada";

/// Every refusal of this service is answered 400 and names its class in the body.
const REFUSED: u16 = 400;

/// The class a request asking for an activation the user cannot use earns
/// (`smithy_query(query="operation",operation="SetUserMFAPreference")` declares
/// it at `@httpError` 400).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The three `UserMFASettingList` values: "the possible values in this list are
/// `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`"
/// (`smithy_query(query="doc",shape="GetUserResponse",member="UserMFASettingList")`).
const SMS_MFA: &str = "SMS_MFA";
const EMAIL_OTP_MFA: &str = "EMAIL_OTP";
const SOFTWARE_TOKEN_MFA: &str = "SOFTWARE_TOKEN_MFA";

/// The sentence an activation no delivery configuration is in place for is
/// refused with, whose only variable part is the setting the request named.
fn no_delivery_config(setting: &str) -> String {
    format!("User does not have delivery config set to turn on {setting}")
}

/// The sentence an activation of software token MFA is refused with in a pool
/// that turns the factor on, for a user whose authenticator is not verified.
const NOT_VERIFIED: &str = "User has not verified software token mfa";

/// What the pool offers the software token as, which is the POOL-level half of
/// the precondition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolOffers {
    /// No `SetUserPoolMfaConfig` call at all, so the pool stores no
    /// `SoftwareTokenMfaConfiguration`.
    Nothing,
    /// `SoftwareTokenMfaConfiguration` stored with `Enabled: false`.
    SoftwareTokenOff,
    /// `SoftwareTokenMfaConfiguration` stored with `Enabled: true`.
    SoftwareTokenOn,
}

/// How far the user's own software-token enrolment got, which is the USER-level
/// half of the precondition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enrolment {
    /// Nothing: no key associated for the user at all.
    Nothing,
    /// `AssociateSoftwareToken` answered a `SecretCode`, and no verification
    /// followed.
    Associated,
    /// `AssociateSoftwareToken` and then `VerifySoftwareToken` with a code the
    /// minted key emits, so the authenticator is registered.
    Verified,
}

/// Which half of the preference surface the request arrives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Half {
    /// `SetUserMFAPreference`, naming the user by its access token alone.
    SignedInUser,
    /// `AdminSetUserMFAPreference`, naming the pool and the user.
    Administrator,
}

/// What a row expects of the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The preference is stored and a later read reports the factor.
    Stored,
    /// The request is refused `InvalidParameterException` with the delivery
    /// configuration sentence for the setting it named.
    NoDeliveryConfig,
    /// The request is refused `InvalidParameterException` with the verification
    /// the user still owes.
    NotVerified,
}

/// One row of the software-token table.
struct Row {
    case: &'static str,
    offers: PoolOffers,
    enrolment: Enrolment,
    /// Whether the request also asks for the factor to be the preferred one.
    preferred: bool,
    half: Half,
    expected: Expect,
}

/// The rows: the pool's configuration crossed with how far the user's enrolment
/// got, over both halves of the surface and both answers to `PreferredMfa`.
const ROWS: &[Row] = &[
    Row {
        case: "a pool that turns TOTP MFA on, for a user that associated no key",
        offers: PoolOffers::SoftwareTokenOn,
        enrolment: Enrolment::Nothing,
        preferred: false,
        half: Half::SignedInUser,
        expected: Expect::NotVerified,
    },
    Row {
        case: "a pool that turns TOTP MFA on, for a user holding an unverified association, \
               asked for as the preferred factor",
        offers: PoolOffers::SoftwareTokenOn,
        enrolment: Enrolment::Associated,
        preferred: true,
        half: Half::SignedInUser,
        expected: Expect::NotVerified,
    },
    Row {
        case: "the same unverified association through the administrative half",
        offers: PoolOffers::SoftwareTokenOn,
        enrolment: Enrolment::Associated,
        preferred: true,
        half: Half::Administrator,
        expected: Expect::NotVerified,
    },
    Row {
        case: "a pool that turns TOTP MFA on, for a user whose authenticator is verified",
        offers: PoolOffers::SoftwareTokenOn,
        enrolment: Enrolment::Verified,
        preferred: true,
        half: Half::SignedInUser,
        expected: Expect::Stored,
    },
    Row {
        case: "a verified authenticator activated without the preference, through the \
               administrative half",
        offers: PoolOffers::SoftwareTokenOn,
        enrolment: Enrolment::Verified,
        preferred: false,
        half: Half::Administrator,
        expected: Expect::Stored,
    },
    Row {
        case: "a pool whose TOTP MFA configuration is turned off, for a user holding an \
               unverified association",
        offers: PoolOffers::SoftwareTokenOff,
        enrolment: Enrolment::Associated,
        preferred: true,
        half: Half::SignedInUser,
        expected: Expect::NoDeliveryConfig,
    },
    Row {
        case: "a pool that was never given a TOTP MFA configuration, for a user that \
               associated no key",
        offers: PoolOffers::Nothing,
        enrolment: Enrolment::Nothing,
        preferred: true,
        half: Half::Administrator,
        expected: Expect::NoDeliveryConfig,
    },
    Row {
        case: "a pool that was never given a TOTP MFA configuration, for a user holding an \
               unverified association",
        offers: PoolOffers::Nothing,
        enrolment: Enrolment::Associated,
        preferred: false,
        half: Half::SignedInUser,
        expected: Expect::NoDeliveryConfig,
    },
];

/// A pool, its app client, one user and that user's access token.
struct Fixture {
    user_pool_id: String,
    access_token: String,
}

/// Build [`Fixture`] in a pool named `name` that offers `offers`.
async fn fixture(emulator: &Emulator, name: &str, offers: PoolOffers) -> Fixture {
    let user_pool_id = pool(emulator, name, json!({})).await;
    let software_token = match offers {
        PoolOffers::Nothing => None,
        PoolOffers::SoftwareTokenOff => Some(false),
        PoolOffers::SoftwareTokenOn => Some(true),
    };
    if let Some(enabled) = software_token {
        ok(
            emulator,
            "SetUserPoolMfaConfig",
            &json!({
                "UserPoolId": user_pool_id,
                // Optional MFA demands nothing of a user, so the rows are about
                // the activation alone.
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": enabled },
            }),
        )
        .await;
    }
    let client_id = app_client(emulator, &user_pool_id, "preference-client", json!({})).await;
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
    let signed_in = ok_self_service(
        emulator,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let access_token = signed_in["AuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in with no factor activated issues a token: {signed_in}"))
        .to_string();
    Fixture {
        user_pool_id,
        access_token,
    }
}

/// Take the user's enrolment as far as `enrolment`, through the operations that
/// own it: the association that mints the key and the verification that
/// registers the authenticator holding it.
async fn enrol(emulator: &Emulator, case: &str, token: &str, enrolment: Enrolment) {
    if enrolment == Enrolment::Nothing {
        return;
    }
    let associated = ok_self_service(
        emulator,
        "AssociateSoftwareToken",
        &json!({ "AccessToken": token }),
    )
    .await;
    let secret_code = associated["SecretCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the association reports a key, got {associated}"))
        .to_string();
    if enrolment == Enrolment::Verified {
        let code = one_time_password(&secret_code, current_step(SystemTime::now()));
        let verified = ok_self_service(
            emulator,
            "VerifySoftwareToken",
            &json!({ "AccessToken": token, "UserCode": code }),
        )
        .await;
        assert_eq!(
            verified["Status"].as_str(),
            Some("SUCCESS"),
            "case {case:?}: a code the minted key emits must register the authenticator, got \
             {verified}"
        );
    }
}

/// The request one row submits on its half of the surface: only the settings
/// structure `member`, asking for the factor to be activated and for
/// `preferred`.
fn preference_body(fixture: &Fixture, half: Half, member: &str, preferred: bool) -> Value {
    let mut body = Map::new();
    match half {
        Half::SignedInUser => {
            body.insert("AccessToken".to_string(), json!(fixture.access_token));
        }
        Half::Administrator => {
            body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
            body.insert("Username".to_string(), json!(USERNAME));
        }
    }
    body.insert(
        member.to_string(),
        json!({ "Enabled": true, "PreferredMfa": preferred }),
    );
    Value::Object(body)
}

/// Submit the preference `body` on `half` and return the status and the answer.
async fn set_preference(emulator: &Emulator, half: Half, body: &Value) -> (u16, Value) {
    match half {
        Half::SignedInUser => dispatch(emulator, "SetUserMFAPreference", body, false).await,
        Half::Administrator => dispatch(emulator, "AdminSetUserMFAPreference", body, true).await,
    }
}

/// What `GetUser` reports about the user's MFA preference: the activated settings
/// and the preferred one.
async fn reported(emulator: &Emulator, token: &str) -> (Vec<String>, Option<String>) {
    let user = ok_self_service(emulator, "GetUser", &json!({ "AccessToken": token })).await;
    let settings = user["UserMFASettingList"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let preferred = user
        .get("PreferredMfaSetting")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (settings, preferred)
}

/// RULE: which of the two preconditions of activating software token MFA is
/// missing decides what the refusal SAYS — the pool's configuration of the
/// factor, or the user's own verification of its authenticator — and a user that
/// has both gets the preference stored.
#[tokio::test]
async fn a_software_token_activation_reports_the_precondition_that_is_missing() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let fixture = fixture(&emulator, &format!("preference-pool-{index}"), row.offers).await;
        enrol(&emulator, case, &fixture.access_token, row.enrolment).await;

        let body = preference_body(
            &fixture,
            row.half,
            "SoftwareTokenMfaSettings",
            row.preferred,
        );
        let (status, payload) = set_preference(&emulator, row.half, &body).await;

        let expected_settings: Vec<String> = match row.expected {
            Expect::Stored => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the preference must be stored, got {status} with {payload}"
                );
                vec![SOFTWARE_TOKEN_MFA.to_string()]
            }
            Expect::NoDeliveryConfig | Expect::NotVerified => {
                assert_eq!(
                    (status, error_code(&payload)),
                    (REFUSED, INVALID_PARAMETER),
                    "case {case:?}: expected a {REFUSED} {INVALID_PARAMETER}, got {status} with \
                     {payload}"
                );
                let wanted = if row.expected == Expect::NotVerified {
                    NOT_VERIFIED.to_string()
                } else {
                    no_delivery_config(SOFTWARE_TOKEN_MFA)
                };
                let message = error_message(&payload);
                assert_eq!(
                    message, wanted,
                    "case {case:?}: expected the refusal to report {wanted:?}, got {message:?}"
                );
                // A refused request writes nothing, so the user still has no
                // factor activated.
                Vec::new()
            }
        };
        let expected_preferred = (row.expected == Expect::Stored && row.preferred)
            .then(|| SOFTWARE_TOKEN_MFA.to_string());

        let (settings, preferred) = reported(&emulator, &fixture.access_token).await;
        assert_eq!(
            (settings, preferred),
            (expected_settings.clone(), expected_preferred.clone()),
            "case {case:?}: expected the read to report {expected_settings:?} activated and \
             {expected_preferred:?} preferred"
        );
    }
}

/// RULE, the other two factors: the delivery-configuration sentence is still
/// what an activation of a factor the user cannot receive is refused with, and
/// it names the setting the request asked for — in a pool that DOES turn the
/// software token on, so the verification sentence is not handed out for a
/// factor it says nothing about.
#[tokio::test]
async fn the_other_factors_keep_the_delivery_configuration_sentence() {
    let emulator = emulator();
    let fixture = fixture(
        &emulator,
        "other-factor-pool",
        PoolOffers::SoftwareTokenOn,
    )
    .await;

    // The user holds no verified phone number and no verified email address, so
    // neither message factor reaches it.
    for (setting, member, half) in [
        (SMS_MFA, "SMSMfaSettings", Half::SignedInUser),
        (EMAIL_OTP_MFA, "EmailMfaSettings", Half::SignedInUser),
        (SMS_MFA, "SMSMfaSettings", Half::Administrator),
        (EMAIL_OTP_MFA, "EmailMfaSettings", Half::Administrator),
    ] {
        let case = format!("{setting} on {half:?}");
        let body = preference_body(&fixture, half, member, true);
        let (status, payload) = set_preference(&emulator, half, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (REFUSED, INVALID_PARAMETER),
            "case {case:?}: expected a {REFUSED} {INVALID_PARAMETER}, got {status} with {payload}"
        );
        let wanted = no_delivery_config(setting);
        let message = error_message(&payload);
        assert_eq!(
            message, wanted,
            "case {case:?}: expected the refusal to report {wanted:?}, got {message:?}"
        );
    }
}

/// RULE, end to end: the verification is the only thing that changes between the
/// refused call and the served one — the identical `SetUserMFAPreference` is
/// refused for the verification the user owes, and served once
/// `VerifySoftwareToken` has succeeded.
#[tokio::test]
async fn verifying_the_authenticator_turns_the_refused_call_into_a_stored_preference() {
    let emulator = emulator();
    let fixture = fixture(&emulator, "enrolment-pool", PoolOffers::SoftwareTokenOn).await;
    let associated = ok_self_service(
        &emulator,
        "AssociateSoftwareToken",
        &json!({ "AccessToken": fixture.access_token }),
    )
    .await;
    let secret_code = associated["SecretCode"]
        .as_str()
        .unwrap_or_else(|| panic!("the association reports a key, got {associated}"))
        .to_string();

    // A code the minted key does not emit registers nothing, so the user is
    // still unverified: the digits of the current one-time password, rotated by
    // one, which no step of this key emits as the current code.
    let current = one_time_password(&secret_code, current_step(SystemTime::now()));
    let wrong: String = current
        .chars()
        .map(|digit| {
            let next = digit.to_digit(10).unwrap_or(0).wrapping_add(1) % 10;
            char::from_digit(next, 10).unwrap_or('0')
        })
        .collect();
    let (status, payload) = dispatch(
        &emulator,
        "VerifySoftwareToken",
        &json!({ "AccessToken": fixture.access_token, "UserCode": wrong }),
        false,
    )
    .await;
    assert_ne!(
        (status, payload["Status"].as_str()),
        (200, Some("SUCCESS")),
        "a code the key does not emit must not register the authenticator, got {payload}"
    );

    let body = preference_body(
        &fixture,
        Half::SignedInUser,
        "SoftwareTokenMfaSettings",
        true,
    );
    let (status, payload) = set_preference(&emulator, Half::SignedInUser, &body).await;
    assert_eq!(
        (status, error_code(&payload)),
        (REFUSED, INVALID_PARAMETER),
        "the activation before a verification is refused, got {status} with {payload}"
    );
    let message = error_message(&payload);
    assert_eq!(
        message, NOT_VERIFIED,
        "expected the refusal to report {NOT_VERIFIED:?}, got {message:?}"
    );
    let (settings, preferred) = reported(&emulator, &fixture.access_token).await;
    assert_eq!(
        (settings.as_slice(), preferred.as_deref()),
        (&[] as &[String], None),
        "the refused request activates nothing, got {settings:?} and {preferred:?}"
    );

    // The one change: the user verifies its authenticator.
    let code = one_time_password(&secret_code, current_step(SystemTime::now()));
    let verified = ok_self_service(
        &emulator,
        "VerifySoftwareToken",
        &json!({ "AccessToken": fixture.access_token, "UserCode": code }),
    )
    .await;
    assert_eq!(
        verified["Status"].as_str(),
        Some("SUCCESS"),
        "a code the minted key emits must register the authenticator, got {verified}"
    );

    let (status, payload) = set_preference(&emulator, Half::SignedInUser, &body).await;
    assert_eq!(
        status, 200,
        "the identical activation after the verification must be served, got {status} with \
         {payload}"
    );
    let (settings, preferred) = reported(&emulator, &fixture.access_token).await;
    assert_eq!(
        (settings.as_slice(), preferred.as_deref()),
        (
            [SOFTWARE_TOKEN_MFA.to_string()].as_slice(),
            Some(SOFTWARE_TOKEN_MFA)
        ),
        "expected the verified factor to be activated and preferred, got {settings:?} and \
         {preferred:?}"
    );
}
