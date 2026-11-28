// SPDX-License-Identifier: CC0-1.0

//! Deleting a user's time-based one-time password (TOTP) authenticator
//! administratively, driven exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.AdminDeleteSoftwareToken`).
//!
//! The rule under test: **deleting a user's software token takes the WHOLE TOTP
//! factor away — the registration, the private key the association minted, the
//! `SOFTWARE_TOKEN_MFA` setting the user had activated and a preference for it —
//! so "after this operation, the user can no longer sign in with TOTP MFA, and can
//! register a new TOTP factor with `AssociateSoftwareToken`"
//! (`AdminDeleteSoftwareToken` in this service's model), while every other factor
//! the user holds is left exactly as it was.** A user that holds no software token
//! at all names no factor for the call to delete, so the call is REFUSED for the
//! factor that is missing and leaves the user untouched, as is a pool or a user
//! the request names and this emulator does not hold.
//!
//! Each row asserts what a LATER caller sees — `AdminGetUser` and
//! `AdminGetUserAuthFactors` for the MFA configuration, `VerifySoftwareToken` for
//! whether the deleted private key still registers anything, and
//! `AdminSetUserMFAPreference` for whether TOTP MFA can be activated at all — never
//! the answer of the call that wrote it. The rows differ along how far the user got
//! in setting its authenticator up (a key alone, a registration, an activation, a
//! preference) and along what OTHER factor it holds beside it.
//!
//! The pools, app clients, users, access tokens and keys are all real: they come
//! from `CreateUserPool`, `SetUserPoolMfaConfig`, `CreateUserPoolClient`,
//! `AdminCreateUser`, `AdminSetUserPassword`, `InitiateAuth`,
//! `AssociateSoftwareToken` and `VerifySoftwareToken`. The one-time passwords the
//! suite presents are computed from RFC 6238's definition over the key the
//! association answered with (`crate::software_token_setup::one_time_password`), so
//! a wrong algorithm in the service cannot be the reason a row passes.

use std::time::SystemTime;

use serde_json::{json, Map, Value};

use crate::software_token_setup::{
    app_client, current_step, dispatch, emulator, error_code, error_message, ok, ok_self_service,
    one_time_password, pool, Emulator, GOOD_PASSWORD,
};

/// The `UserMFASettingList` value TOTP MFA is reported under: "the possible values
/// in this list are `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`" (this
/// service's model).
const SOFTWARE_TOKEN_MFA: &str = "SOFTWARE_TOKEN_MFA";

/// The `UserMFASettingList` value SMS message MFA is reported under, which is the
/// other factor a row activates beside the software token.
const SMS_MFA: &str = "SMS_MFA";

/// The `AuthFactorType` value a registered and activated software token offers
/// (`smithy_query(query="shape", shape="AuthFactorType")`).
const SOFTWARE_TOKEN: &str = "SOFTWARE_TOKEN";

/// The wire error code of a request naming a resource this emulator does not hold.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The sentence a delete of a software token the user does not hold is refused
/// with, which states the factor that is not configured and names no value of the
/// request.
const NOT_CONFIGURED_MESSAGE: &str = "Software token MFA has not been configured for this user.";

/// The wire error code of a request naming a user the pool does not hold.
const USER_NOT_FOUND: &str = "UserNotFoundException";

/// The wire error code of a request whose parameters are refused.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The wire error code a verification for a user that holds NO software token at
/// all is refused with: the delete took the private key away, so there is no
/// factor for a presented code to be checked against rather than a code that
/// failed to match one.
const SOFTWARE_TOKEN_MFA_NOT_FOUND: &str = "SoftwareTokenMFANotFoundException";

/// The username every fixture user is created under.
const USERNAME: &str = "ada";

/// A pool with one app client holding one signed-in user, and the user's access
/// token.
struct Fixture {
    user_pool_id: String,
    access_token: String,
}

/// A pool whose MFA is optional with TOTP offered, holding one signed-in user that
/// carries `attributes`.
async fn fixture(emulator: &Emulator, name: &str, attributes: &[(&str, &str)]) -> Fixture {
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
    let user_attributes: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    ok(
        emulator,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "UserAttributes": user_attributes,
            "MessageAction": "SUPPRESS",
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
        .unwrap_or_else(|| panic!("a completed sign-in issues an access token: {signed_in}"))
        .to_string();
    Fixture {
        user_pool_id,
        access_token,
    }
}

/// Begin a setup for the user `access_token` names and return the key it minted.
async fn associate(emulator: &Emulator, case: &str, access_token: &str) -> String {
    let payload = ok_self_service(
        emulator,
        "AssociateSoftwareToken",
        &json!({ "AccessToken": access_token }),
    )
    .await;
    payload["SecretCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SecretCode, got {payload}"))
        .to_string()
}

/// Present a one-time password `secret_code` emits now, and return the status and
/// body of the answer.
async fn verify(emulator: &Emulator, access_token: &str, secret_code: &str) -> (u16, Value) {
    let code = one_time_password(secret_code, current_step(SystemTime::now()));
    dispatch(
        emulator,
        "VerifySoftwareToken",
        &json!({ "AccessToken": access_token, "UserCode": code }),
        false,
    )
    .await
}

/// Ask for `member` to be activated for the fixture user, and `preferred` as well,
/// and return the status and body of the answer.
async fn set_preference(
    emulator: &Emulator,
    user_pool_id: &str,
    member: &str,
    preferred: bool,
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Username".to_string(), json!(USERNAME));
    body.insert(
        member.to_string(),
        json!({ "Enabled": true, "PreferredMfa": preferred }),
    );
    dispatch(
        emulator,
        "AdminSetUserMFAPreference",
        &Value::Object(body),
        true,
    )
    .await
}

/// What `AdminGetUser` and `AdminGetUserAuthFactors` report about the fixture
/// user's MFA configuration: the activated settings, the preferred one, and the
/// factors it is configured for.
struct Reported {
    settings: Vec<String>,
    preferred: Option<String>,
    factors: Vec<String>,
}

/// Read the fixture user's MFA configuration back through the administrative reads.
async fn reported(emulator: &Emulator, user_pool_id: &str) -> Reported {
    let asked = json!({ "UserPoolId": user_pool_id, "Username": USERNAME });
    let user = ok(emulator, "AdminGetUser", &asked).await;
    let factors = ok(emulator, "AdminGetUserAuthFactors", &asked).await;
    Reported {
        settings: strings(&user["UserMFASettingList"]),
        preferred: user
            .get("PreferredMfaSetting")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        factors: strings(&factors["ConfiguredUserAuthFactors"]),
    }
}

/// The strings of a JSON list member, and none for a member the answer leaves out.
fn strings(member: &Value) -> Vec<String> {
    member
        .as_array()
        .map(|list| {
            list.iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Delete the fixture user's software token and return the status and body of the
/// answer.
async fn delete_software_token(emulator: &Emulator, user_pool_id: &str) -> (u16, Value) {
    dispatch(
        emulator,
        "AdminDeleteSoftwareToken",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
        true,
    )
    .await
}

/// How far a row's user got in setting its software token up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Setup {
    /// `AssociateSoftwareToken` alone: a private key is stored for the user and no
    /// code has been presented, so nothing is registered.
    Associated,
    /// A code the key emits was verified, so the authenticator is registered —
    /// which is what lets TOTP MFA be activated at all.
    Registered,
    /// Registered and activated as an MFA factor.
    Activated,
    /// Registered, activated and the user's preferred factor.
    Preferred,
}

/// One row: what the user holds before the delete, and what a later read must
/// report after it.
struct Row {
    case: &'static str,
    /// The attributes the user is created with, which decide what OTHER factor it
    /// can receive.
    attributes: &'static [(&'static str, &'static str)],
    setup: Setup,
    /// Whether SMS message MFA is activated as the user's preferred factor beside
    /// the software token — a factor the delete must leave alone.
    sms: bool,
    /// The `UserMFASettingList` a later read must report after the delete.
    settings_after: &'static [&'static str],
    /// The `PreferredMfaSetting` a later read must report after the delete.
    preferred_after: Option<&'static str>,
}

/// A verified phone number, which is the delivery configuration SMS message MFA
/// reaches a user through.
const VERIFIED_PHONE: &[(&str, &str)] = &[
    ("phone_number", "+15550100001"),
    ("phone_number_verified", "true"),
];

/// The rows differ along how far the setup got, and along what the user holds
/// BESIDE the software token.
const ROWS: &[Row] = &[
    Row {
        case: "a registered authenticator the user prefers",
        attributes: &[],
        setup: Setup::Preferred,
        sms: false,
        settings_after: &[],
        preferred_after: None,
    },
    Row {
        case: "a registered authenticator activated without being preferred",
        attributes: &[],
        setup: Setup::Activated,
        sms: false,
        settings_after: &[],
        preferred_after: None,
    },
    Row {
        case: "a registered authenticator the user never activated",
        attributes: &[],
        setup: Setup::Registered,
        sms: false,
        settings_after: &[],
        preferred_after: None,
    },
    Row {
        case: "an association whose code was never verified",
        attributes: &[],
        setup: Setup::Associated,
        sms: false,
        settings_after: &[],
        preferred_after: None,
    },
    Row {
        case: "an activated authenticator beside SMS message MFA, which the user prefers",
        attributes: VERIFIED_PHONE,
        setup: Setup::Activated,
        sms: true,
        settings_after: &[SMS_MFA],
        preferred_after: Some(SMS_MFA),
    },
];

#[tokio::test]
async fn deleting_a_software_token_takes_the_whole_totp_factor_away() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let fixture = fixture(
            &emulator,
            &format!("delete-software-token-pool-{index}"),
            row.attributes,
        )
        .await;

        // The key this row's setup provisioned the authenticator with, which the
        // delete has to take away with the rest of the factor.
        let secret_code = associate(&emulator, case, &fixture.access_token).await;
        if row.setup != Setup::Associated {
            let (status, payload) = verify(&emulator, &fixture.access_token, &secret_code).await;
            assert_eq!(
                (status, payload["Status"].as_str()),
                (200, Some("SUCCESS")),
                "case {case:?}: a code the stored key emits must register the authenticator, \
                 got {status} with {payload}"
            );
        }
        if matches!(row.setup, Setup::Activated | Setup::Preferred) {
            let (status, payload) = set_preference(
                &emulator,
                &fixture.user_pool_id,
                "SoftwareTokenMfaSettings",
                row.setup == Setup::Preferred,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: activating TOTP MFA for a registered authenticator must be \
                 served, got {status} with {payload}"
            );
        }
        if row.sms {
            let (status, payload) =
                set_preference(&emulator, &fixture.user_pool_id, "SMSMfaSettings", true).await;
            assert_eq!(
                status, 200,
                "case {case:?}: activating SMS message MFA for a user with a verified phone \
                 number must be served, got {status} with {payload}"
            );
        }

        // What the user holds BEFORE the delete, so the row asserts a change rather
        // than a state it was always in.
        let before = reported(&emulator, &fixture.user_pool_id).await;
        let activated_before = matches!(row.setup, Setup::Activated | Setup::Preferred);
        assert_eq!(
            before
                .settings
                .iter()
                .any(|setting| setting == SOFTWARE_TOKEN_MFA),
            activated_before,
            "case {case:?}: expected TOTP MFA activated before the delete to be \
             {activated_before}, got {:?}",
            before.settings
        );

        let (status, payload) = delete_software_token(&emulator, &fixture.user_pool_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting the software token must be served, got {status} with \
             {payload}"
        );
        assert!(
            payload
                .as_object()
                .is_none_or(|members| members.is_empty()),
            "case {case:?}: expected an answer carrying no member, got {payload}"
        );

        let after = reported(&emulator, &fixture.user_pool_id).await;
        assert_eq!(
            after.settings, row.settings_after,
            "case {case:?}: expected the activated settings {:?} after the delete, got {:?}",
            row.settings_after, after.settings
        );
        assert_eq!(
            after.preferred.as_deref(),
            row.preferred_after,
            "case {case:?}: expected the preferred setting {:?} after the delete, got {:?}",
            row.preferred_after,
            after.preferred
        );
        assert!(
            !after.factors.iter().any(|factor| factor == SOFTWARE_TOKEN),
            "case {case:?}: expected no {SOFTWARE_TOKEN} factor after the delete, got {:?}",
            after.factors
        );

        // The private key is gone with the registration: the user holds no
        // software token at all, so a code the DELETED key emits has no factor
        // to be checked against any more.
        let (status, payload) = verify(&emulator, &fixture.access_token, &secret_code).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, SOFTWARE_TOKEN_MFA_NOT_FOUND),
            "case {case:?}: expected a code from the deleted key to be refused with a 400 \
             {SOFTWARE_TOKEN_MFA_NOT_FOUND}, got {status} with {payload}"
        );

        // And with no registration there is no delivery configuration TOTP MFA
        // could be activated with.
        let (status, payload) = set_preference(
            &emulator,
            &fixture.user_pool_id,
            "SoftwareTokenMfaSettings",
            false,
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, INVALID_PARAMETER),
            "case {case:?}: expected activating TOTP MFA after the delete to be refused with a \
             400 {INVALID_PARAMETER}, got {status} with {payload}"
        );

        // "Can register a new TOTP factor with `AssociateSoftwareToken`": a fresh
        // setup mints its own key and registers the authenticator again.
        let fresh_code = associate(&emulator, case, &fixture.access_token).await;
        assert_ne!(
            fresh_code, secret_code,
            "case {case:?}: expected a fresh association to mint a new key, got the deleted one \
             back"
        );
        let (status, payload) = verify(&emulator, &fixture.access_token, &fresh_code).await;
        assert_eq!(
            (status, payload["Status"].as_str()),
            (200, Some("SUCCESS")),
            "case {case:?}: a code the new key emits must register the authenticator again, got \
             {status} with {payload}"
        );
        let (status, payload) = set_preference(
            &emulator,
            &fixture.user_pool_id,
            "SoftwareTokenMfaSettings",
            true,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: activating TOTP MFA for the re-registered authenticator must be \
             served, got {status} with {payload}"
        );
        let again = reported(&emulator, &fixture.user_pool_id).await;
        assert!(
            again
                .settings
                .iter()
                .any(|setting| setting == SOFTWARE_TOKEN_MFA)
                && again.preferred.as_deref() == Some(SOFTWARE_TOKEN_MFA)
                && again.factors.iter().any(|factor| factor == SOFTWARE_TOKEN),
            "case {case:?}: expected the re-registered factor to be activated, preferred and \
             configured again, got settings {:?}, preferred {:?} and factors {:?}",
            again.settings,
            again.preferred,
            again.factors
        );
    }
}

/// One row of the refusal table: how the row's user comes to hold no software
/// token, and what it holds BESIDE one.
struct NoFactor {
    case: &'static str,
    /// The attributes the user is created with, which decide what other factor it
    /// can receive.
    attributes: &'static [(&'static str, &'static str)],
    /// Whether SMS message MFA is activated for the user, so the row asserts that
    /// another activated factor is no software token.
    sms: bool,
    /// How many whole setups a delete already took away before the refused call:
    /// none for a user that never set an authenticator up, one for a user whose
    /// factor a delete removed, and two for one that set a fresh factor up
    /// afterwards and had that deleted as well.
    setups_deleted: usize,
}

/// The rows differ along how the user came to hold no software token and along
/// what it holds beside one.
const NO_FACTOR_ROWS: &[NoFactor] = &[
    NoFactor {
        case: "a user that never set an authenticator up",
        attributes: &[],
        sms: false,
        setups_deleted: 0,
    },
    NoFactor {
        case: "a user holding SMS message MFA and no authenticator",
        attributes: VERIFIED_PHONE,
        sms: true,
        setups_deleted: 0,
    },
    NoFactor {
        case: "a user whose registered authenticator a delete already took away",
        attributes: &[],
        sms: false,
        setups_deleted: 1,
    },
    NoFactor {
        case: "a user whose second authenticator a second delete took away",
        attributes: VERIFIED_PHONE,
        sms: true,
        setups_deleted: 2,
    },
];

/// A user that holds no software token has none for this call to delete, so the
/// call is refused for the missing factor — the class, the sentence and the error
/// shape an SDK resolves from the header — and nothing of the user changes, which
/// is what makes a following delete refuse as well.
#[tokio::test]
async fn deleting_a_software_token_the_user_does_not_hold_is_refused_for_the_missing_factor() {
    for (index, row) in NO_FACTOR_ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let fixture = fixture(
            &emulator,
            &format!("no-software-token-pool-{index}"),
            row.attributes,
        )
        .await;
        if row.sms {
            let (status, payload) =
                set_preference(&emulator, &fixture.user_pool_id, "SMSMfaSettings", true).await;
            assert_eq!(
                status, 200,
                "case {case:?}: activating SMS message MFA for a user with a verified phone \
                 number must be served, got {status} with {payload}"
            );
        }
        for round in 0..row.setups_deleted {
            // A whole setup — a key, a code it emits and the registration that code
            // completes — and then the delete that takes it away, so the refused
            // call below is refused for a factor that is GONE rather than one that
            // was never there.
            let secret_code = associate(&emulator, case, &fixture.access_token).await;
            let (status, payload) = verify(&emulator, &fixture.access_token, &secret_code).await;
            assert_eq!(
                (status, payload["Status"].as_str()),
                (200, Some("SUCCESS")),
                "case {case:?}, setup {round}: a code the stored key emits must register the \
                 authenticator, got {status} with {payload}"
            );
            let (status, payload) = delete_software_token(&emulator, &fixture.user_pool_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}, setup {round}: deleting the registered factor must be served, got \
                 {status} with {payload}"
            );
        }

        let asked = json!({ "UserPoolId": fixture.user_pool_id, "Username": USERNAME });
        let before = ok(&emulator, "AdminGetUser", &asked).await;
        // Driven through the harness that reports the response HEADERS too, so the
        // error shape an AWS SDK resolves the exception type from is asserted
        // beside the body (`crate::error_wire`).
        let answered = crate::error_wire::call(
            &emulator.router,
            "AdminDeleteSoftwareToken",
            &json!({ "UserPoolId": fixture.user_pool_id, "Username": USERNAME }),
        )
        .await;
        assert_eq!(
            (
                answered.status,
                error_code(&answered.payload),
                error_message(&answered.payload)
            ),
            (400, RESOURCE_NOT_FOUND, NOT_CONFIGURED_MESSAGE),
            "case {case:?}: expected a 400 {RESOURCE_NOT_FOUND} saying \
             {NOT_CONFIGURED_MESSAGE:?}, got {} with {}",
            answered.status,
            answered.payload
        );
        let named = format!("{RESOURCE_NOT_FOUND}:");
        assert_eq!(
            answered.errortype.as_deref(),
            Some(named.as_str()),
            "case {case:?}: expected the refusal to name {named:?} in its error-type header, got \
             {:?}",
            answered.errortype
        );

        let after = ok(&emulator, "AdminGetUser", &asked).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused call to leave the user untouched, got {after} \
             instead of {before}"
        );
    }
}

/// One refusal row: the pool and the user the request names, and the error it owes.
struct Refusal {
    case: &'static str,
    /// Whether the request names the pool the fixture created, or one no call did.
    held_pool: bool,
    username: &'static str,
    code: &'static str,
}

/// A pool id that satisfies the modelled `UserPoolIdType` (pattern
/// `^[\w-]+_[0-9a-zA-Z]+$`) and that no `CreateUserPool` minted.
const ABSENT_POOL_ID: &str = "us-east-1_absentpool";

/// The rows differ along WHICH named resource is missing.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a pool no call created",
        held_pool: false,
        username: USERNAME,
        code: RESOURCE_NOT_FOUND,
    },
    Refusal {
        case: "a username the pool does not hold",
        held_pool: true,
        username: "grace",
        code: USER_NOT_FOUND,
    },
    Refusal {
        case: "a username that is no alias of any user of the pool",
        held_pool: true,
        username: "ada@example.com",
        code: USER_NOT_FOUND,
    },
];

#[tokio::test]
async fn the_delete_names_the_resource_the_request_asked_for_and_this_emulator_does_not_hold() {
    let emulator = emulator();
    let fixture = fixture(&emulator, "refused-delete-pool", &[]).await;

    for row in REFUSALS {
        let case = row.case;
        let asked_pool = if row.held_pool {
            fixture.user_pool_id.clone()
        } else {
            ABSENT_POOL_ID.to_string()
        };
        let (status, payload) = dispatch(
            &emulator,
            "AdminDeleteSoftwareToken",
            &json!({ "UserPoolId": asked_pool, "Username": row.username }),
            true,
        )
        .await;
        // This `awsJson1_1` endpoint carries every modelled refusal on HTTP 400 and
        // names the class in `__type` (as the `error_wire` suite pins), whatever
        // status the model declares for it.
        assert_eq!(
            (status, error_code(&payload)),
            (400, row.code),
            "case {case:?}: expected a 400 {}, got {status} with {payload}",
            row.code
        );
        let expected_message = if row.held_pool {
            "User does not exist.".to_string()
        } else {
            format!("User pool {asked_pool} does not exist.")
        };
        let message = error_message(&payload);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
    }
}
