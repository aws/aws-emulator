// SPDX-License-Identifier: CC0-1.0

//! `AdminGetUserAuthFactors` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.AdminGetUserAuthFactors`).
//!
//! The rule under test: an administrative read of a user's authentication options
//! reports, for the STORED user that the request's `UserPoolId` and `Username`
//! resolve to, the MFA settings that user holds — the activated factors and the
//! preferred one — and the `AuthFactorType` values that user can authenticate with
//! in a choice-based (`USER_AUTH`) sign-in: a factor the pool admits as a first
//! authentication factor and the user holds what it needs for (a password, a
//! verified address a one-time password is delivered to, a registered passkey), or
//! a factor the user has activated as an MFA factor. A factor neither half offers
//! is not reported, the reported list is in the order the model declares
//! `AuthFactorType`'s values in rather than the order the pool lists them in, and
//! the whole answer is computed from the STORED pool and the STORED user, so
//! asking twice answers the same and asking changes nothing a later caller sees.
//!
//! A request naming a pool that does not exist is refused as
//! `ResourceNotFoundException`, and one naming no user of an existing pool as
//! `UserNotFoundException`.
//!
//! The pools, app clients and users are all real: they come from
//! `CreateUserPool`, `SetUserPoolMfaConfig`, `AdminCreateUser`,
//! `AdminSetUserPassword` and `AdminSetUserMFAPreference`. Two facts are planted
//! in this service's state rather than written through the operations that own
//! them: a user's registered TOTP authenticator (which `AssociateSoftwareToken`
//! and `VerifySoftwareToken` write, asserted by [`super::software_token_setup`],
//! so these rows need none of that arithmetic) and a registered passkey
//! (`CompleteWebAuthnRegistration`, still a scaffold stub).
//!
//! Each table row names the case it asserts.

use std::sync::Arc;
use std::time::SystemTime;

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

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The three `UserMFASettingList` values, which that member documents as "the MFA
/// options that are activated for the user. The possible values in this list are
/// `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`" (this service's model).
const SMS_MFA: &str = "SMS_MFA";
const EMAIL_OTP_MFA: &str = "EMAIL_OTP";
const SOFTWARE_TOKEN_MFA: &str = "SOFTWARE_TOKEN_MFA";

/// `AuthFactorType`'s values, as `smithy_query(query="shape",
/// shape="AuthFactorType")` lists them. Every row below expects a subset of these
/// and nothing else.
const PASSWORD: &str = "PASSWORD";
const EMAIL_OTP: &str = "EMAIL_OTP";
const SMS_OTP: &str = "SMS_OTP";
const WEB_AUTHN: &str = "WEB_AUTHN";
const SOFTWARE_TOKEN: &str = "SOFTWARE_TOKEN";

/// The one `WebAuthnFactorConfigurationType` value under which a passkey
/// satisfies an MFA requirement
/// (`WebAuthnConfigurationType.FactorConfiguration` in this service's model); its
/// only other value makes passkeys a single authentication factor.
const PASSKEY_MULTI_FACTOR: &str = "MULTI_FACTOR_WITH_USER_VERIFICATION";
const PASSKEY_SINGLE_FACTOR: &str = "SINGLE_FACTOR";

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

/// Dispatch the signed administrative `operation` with `body` and return its
/// status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION);
    let response = router
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

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
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

/// Create a pool and return its minted `UserPoolId`. `first_auth_factors` is the
/// `Policies.SignInPolicy.AllowedFirstAuthFactors` the pool is created with, left
/// unset when empty; `alias_attributes` is the `AliasAttributes` it configures,
/// likewise.
async fn pool(
    router: &MultiServiceRouter,
    name: &str,
    first_auth_factors: &[&str],
    alias_attributes: &[&str],
) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if !first_auth_factors.is_empty() {
        body.insert(
            "Policies".to_string(),
            json!({ "SignInPolicy": { "AllowedFirstAuthFactors": first_auth_factors } }),
        );
    }
    if !alias_attributes.is_empty() {
        body.insert("AliasAttributes".to_string(), json!(alias_attributes));
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create `username` in `user_pool_id` with `attributes` and give it a permanent
/// password, so the stored user holds what the password factor needs.
async fn user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": Value::Array(
                attributes
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            ),
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

/// Plant the TOTP authenticator registration `AssociateSoftwareToken` and
/// `VerifySoftwareToken` write, so the rows that need a user who HAS registered
/// one are driven without this suite computing one-time passwords.
fn register_software_token(instance: &Emulator, user_pool_id: &str, username: &str) {
    let service_state = state::service_state(&*instance.engine).expect("this service's state");
    let mut guard = service_state.lock();
    let stored = guard.pool_mut(user_pool_id).expect("the created pool");
    let stored = stored.user_mut(username).expect("the created user");
    stored.software_token_registered = true;
}

/// Plant one registered passkey for the user, which
/// `CompleteWebAuthnRegistration` will write once that scaffold is implemented.
fn register_passkey(instance: &Emulator, user_pool_id: &str, username: &str) {
    let service_state = state::service_state(&*instance.engine).expect("this service's state");
    let mut guard = service_state.lock();
    let stored = guard.pool_mut(user_pool_id).expect("the created pool");
    let stored = stored.user_mut(username).expect("the created user");
    stored.web_authn_credentials.push(state::WebAuthnCredential {
        credential_id: "planted-credential".to_string(),
        friendly_credential_name: "planted passkey".to_string(),
        relying_party_id: "auth.example.com".to_string(),
        authenticator_attachment: None,
        authenticator_transports: vec!["internal".to_string()],
        created_at: SystemTime::now(),
    });
}

/// What one `AdminGetUserAuthFactors` answer reports.
#[derive(Debug, PartialEq, Eq)]
struct Factors {
    username: String,
    /// `ConfiguredUserAuthFactors`; empty where the response carries none.
    configured: Vec<String>,
    /// `UserMFASettingList`; empty where the response carries none.
    settings: Vec<String>,
    /// `PreferredMfaSetting`, absent where the response carries none.
    preferred: Option<String>,
}

/// The string members of `payload[member]`, empty when the member is absent.
fn string_list(payload: &Value, member: &str) -> Vec<String> {
    payload[member]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Call `AdminGetUserAuthFactors` for `username` of `user_pool_id` and return
/// what it reported. Requires success.
async fn factors(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> Factors {
    let payload = ok(
        router,
        "AdminGetUserAuthFactors",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    Factors {
        username: payload["Username"]
            .as_str()
            .unwrap_or("<no Username>")
            .to_string(),
        configured: string_list(&payload, "ConfiguredUserAuthFactors"),
        settings: string_list(&payload, "UserMFASettingList"),
        preferred: payload["PreferredMfaSetting"]
            .as_str()
            .map(str::to_string),
    }
}

/// One row of the reported-factors table.
#[derive(Clone, Copy, Debug)]
struct Case {
    /// What this row asserts.
    case: &'static str,
    /// The `AllowedFirstAuthFactors` the pool is created with; empty leaves the
    /// sign-in policy unset, which admits `PASSWORD` alone.
    first_auth_factors: &'static [&'static str],
    /// The attributes `AdminCreateUser` gives the user.
    attributes: &'static [(&'static str, &'static str)],
    /// Whether the user has registered a TOTP authenticator.
    software_token_registered: bool,
    /// Whether the user has registered a passkey.
    passkey_registered: bool,
    /// The `WebAuthnConfiguration.FactorConfiguration` `SetUserPoolMfaConfig`
    /// puts on the pool first, when the row sets one.
    passkey_mfa: Option<&'static str>,
    /// The `(Enabled, PreferredMfa)` pairs `AdminSetUserMFAPreference` submits
    /// for SMS message MFA, software token MFA and email message MFA, each only
    /// when the row names it.
    sms: Option<(bool, bool)>,
    software_token: Option<(bool, bool)>,
    email: Option<(bool, bool)>,
    /// The `ConfiguredUserAuthFactors` the answer must carry, in order.
    configured: &'static [&'static str],
    /// The `UserMFASettingList` the answer must carry.
    settings: &'static [&'static str],
    /// The `PreferredMfaSetting` the answer must carry.
    preferred: Option<&'static str>,
}

/// The attribute sets the rows draw on.
const VERIFIED_EMAIL: &[(&str, &str)] = &[("email", "ada@example.com"), ("email_verified", "true")];
const UNVERIFIED_EMAIL: &[(&str, &str)] = &[("email", "grace@example.com")];
const VERIFIED_PHONE: &[(&str, &str)] = &[
    ("phone_number", "+15550100001"),
    ("phone_number_verified", "true"),
];
const VERIFIED_BOTH: &[(&str, &str)] = &[
    ("email", "barbara@example.com"),
    ("email_verified", "true"),
    ("phone_number", "+15550100002"),
    ("phone_number_verified", "true"),
];

/// A row with nothing configured, which each case below varies from.
const BASE: Case = Case {
    case: "",
    first_auth_factors: &[],
    attributes: &[],
    software_token_registered: false,
    passkey_registered: false,
    passkey_mfa: None,
    sms: None,
    software_token: None,
    email: None,
    configured: &[PASSWORD],
    settings: &[],
    preferred: None,
};

/// The rows differ along WHICH factors the pool admits and in which order it
/// lists them, HOW MANY of them the user is set up for, WHAT the user holds (a
/// verified address, an unverified one, none at all, a registered authenticator
/// or passkey, or none), whether passkey MFA is enabled at the pool level, and
/// whether a factor is offered as a first factor or activated as an MFA factor —
/// with or without being the preferred one.
const CASES: &[Case] = &[
    Case {
        case: "a pool that configures no sign-in policy offers the password factor alone",
        ..BASE
    },
    Case {
        case: "a verified email makes the email one-time-password factor available",
        first_auth_factors: &[PASSWORD, EMAIL_OTP],
        attributes: VERIFIED_EMAIL,
        configured: &[PASSWORD, EMAIL_OTP],
        ..BASE
    },
    Case {
        case: "an unverified email is no address a one-time password is delivered to",
        first_auth_factors: &[PASSWORD, EMAIL_OTP],
        attributes: UNVERIFIED_EMAIL,
        configured: &[PASSWORD],
        ..BASE
    },
    Case {
        case: "a user holding no email at all is not offered that factor either",
        first_auth_factors: &[PASSWORD, EMAIL_OTP],
        configured: &[PASSWORD],
        ..BASE
    },
    Case {
        case: "a verified phone number makes the SMS one-time-password factor available",
        first_auth_factors: &[PASSWORD, SMS_OTP],
        attributes: VERIFIED_PHONE,
        configured: &[PASSWORD, SMS_OTP],
        ..BASE
    },
    Case {
        case: "a factor the pool does not admit is not offered though the user holds what it \
               needs",
        attributes: VERIFIED_BOTH,
        configured: &[PASSWORD],
        ..BASE
    },
    Case {
        case: "the passkey factor needs a registered passkey, which this user has none of",
        first_auth_factors: &[PASSWORD, WEB_AUTHN],
        attributes: VERIFIED_EMAIL,
        configured: &[PASSWORD],
        ..BASE
    },
    Case {
        case: "a registered passkey the pool admits as a first factor is reported",
        first_auth_factors: &[PASSWORD, WEB_AUTHN],
        passkey_registered: true,
        configured: &[PASSWORD, WEB_AUTHN],
        ..BASE
    },
    Case {
        case: "passkey MFA enabled at the pool offers the passkey factor to a user holding one \
               though the pool admits it as no first factor",
        passkey_registered: true,
        passkey_mfa: Some(PASSKEY_MULTI_FACTOR),
        configured: &[PASSWORD, WEB_AUTHN],
        ..BASE
    },
    Case {
        case: "passkey MFA enabled at the pool offers no passkey factor to a user holding no \
               passkey",
        passkey_mfa: Some(PASSKEY_MULTI_FACTOR),
        configured: &[PASSWORD],
        ..BASE
    },
    Case {
        case: "passkeys configured as a single factor are no MFA factor of the pool at all",
        passkey_registered: true,
        passkey_mfa: Some(PASSKEY_SINGLE_FACTOR),
        configured: &[PASSWORD],
        ..BASE
    },
    Case {
        case: "the software token factor is an MFA factor only, so no sign-in policy entry \
               offers it",
        first_auth_factors: &[PASSWORD, SOFTWARE_TOKEN],
        configured: &[PASSWORD],
        ..BASE
    },
    Case {
        case: "every admitted factor the user is set up for is reported, in the order the model \
               declares them and not the order the pool lists them",
        first_auth_factors: &[SMS_OTP, EMAIL_OTP, PASSWORD],
        attributes: VERIFIED_BOTH,
        configured: &[PASSWORD, EMAIL_OTP, SMS_OTP],
        ..BASE
    },
    Case {
        case: "an activated SMS MFA factor is available to the user though the pool admits it as \
               no first factor",
        attributes: VERIFIED_PHONE,
        sms: Some((true, false)),
        configured: &[PASSWORD, SMS_OTP],
        settings: &[SMS_MFA],
        ..BASE
    },
    Case {
        case: "an activated and preferred email message MFA factor is reported as both the \
               activated setting and the preferred one",
        attributes: VERIFIED_EMAIL,
        email: Some((true, true)),
        configured: &[PASSWORD, EMAIL_OTP],
        settings: &[EMAIL_OTP_MFA],
        preferred: Some(EMAIL_OTP_MFA),
        ..BASE
    },
    Case {
        case: "an activated software token MFA factor offers the software token auth factor the \
               pool admits as no first factor",
        software_token_registered: true,
        software_token: Some((true, true)),
        configured: &[PASSWORD, SOFTWARE_TOKEN],
        settings: &[SOFTWARE_TOKEN_MFA],
        preferred: Some(SOFTWARE_TOKEN_MFA),
        ..BASE
    },
    Case {
        case: "three activated MFA factors are all reported, the preferred one among them",
        attributes: VERIFIED_BOTH,
        software_token_registered: true,
        sms: Some((true, false)),
        software_token: Some((true, true)),
        email: Some((true, false)),
        configured: &[PASSWORD, EMAIL_OTP, SMS_OTP, SOFTWARE_TOKEN],
        settings: &[SMS_MFA, SOFTWARE_TOKEN_MFA, EMAIL_OTP_MFA],
        preferred: Some(SOFTWARE_TOKEN_MFA),
        ..BASE
    },
    Case {
        case: "a deactivated factor is reported as neither an activated setting nor an auth \
               factor",
        attributes: VERIFIED_PHONE,
        sms: Some((false, false)),
        configured: &[PASSWORD],
        ..BASE
    },
    Case {
        case: "all five auth factors at once: the pool admits four of them and the fifth is \
               activated as an MFA factor",
        first_auth_factors: &[PASSWORD, EMAIL_OTP, SMS_OTP, WEB_AUTHN],
        attributes: VERIFIED_BOTH,
        software_token_registered: true,
        passkey_registered: true,
        software_token: Some((true, true)),
        configured: &[PASSWORD, EMAIL_OTP, SMS_OTP, WEB_AUTHN, SOFTWARE_TOKEN],
        settings: &[SOFTWARE_TOKEN_MFA],
        preferred: Some(SOFTWARE_TOKEN_MFA),
        ..BASE
    },
];

/// The `AdminSetUserMFAPreference` body one row submits: only the structures the
/// row names are present, as an SDK sends only the settings the caller passed.
fn preference_body(user_pool_id: &str, username: &str, row: &Case) -> Option<Value> {
    let mut body = Map::new();
    for (member, settings) in [
        ("SMSMfaSettings", row.sms),
        ("SoftwareTokenMfaSettings", row.software_token),
        ("EmailMfaSettings", row.email),
    ] {
        if let Some((enabled, preferred)) = settings {
            body.insert(
                member.to_string(),
                json!({ "Enabled": enabled, "PreferredMfa": preferred }),
            );
        }
    }
    if body.is_empty() {
        return None;
    }
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Username".to_string(), json!(username));
    Some(Value::Object(body))
}

/// Stand one row's pool and user up and return the pool's minted id.
async fn arrange(instance: &Emulator, index: usize, row: &Case) -> String {
    let username = "ada";
    let user_pool_id = pool(
        &instance.router,
        &format!("admin-factors-pool-{index}"),
        row.first_auth_factors,
        &[],
    )
    .await;
    if let Some(factor_configuration) = row.passkey_mfa {
        // `WebAuthnConfigurationType.FactorConfiguration` is what enables passkey
        // MFA at the user pool level; a relying party and a user-verification
        // requirement go with it.
        ok(
            &instance.router,
            "SetUserPoolMfaConfig",
            &json!({
                "UserPoolId": user_pool_id,
                "WebAuthnConfiguration": {
                    "RelyingPartyId": "auth.example.com",
                    "UserVerification": "required",
                    "FactorConfiguration": factor_configuration,
                },
            }),
        )
        .await;
    }
    user(&instance.router, &user_pool_id, username, row.attributes).await;
    if row.software_token_registered {
        register_software_token(instance, &user_pool_id, username);
    }
    if row.passkey_registered {
        register_passkey(instance, &user_pool_id, username);
    }
    if let Some(body) = preference_body(&user_pool_id, username, row) {
        ok(&instance.router, "AdminSetUserMFAPreference", &body).await;
    }
    user_pool_id
}

#[tokio::test]
async fn admin_get_user_auth_factors_reports_the_factors_the_stored_user_can_sign_in_with() {
    for (index, row) in CASES.iter().enumerate() {
        let case = row.case;
        let instance = emulator();
        let user_pool_id = arrange(&instance, index, row).await;

        let expected = Factors {
            username: "ada".to_string(),
            configured: row.configured.iter().map(|f| (*f).to_string()).collect(),
            settings: row.settings.iter().map(|s| (*s).to_string()).collect(),
            preferred: row.preferred.map(str::to_string),
        };
        let reported = factors(&instance.router, &user_pool_id, "ada").await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );

        // The MFA members this operation reports are the ones `AdminGetUser`
        // reports for the same stored user: one user holds one MFA state.
        let stored = ok(
            &instance.router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": "ada" }),
        )
        .await;
        assert_eq!(
            (
                string_list(&stored, "UserMFASettingList"),
                stored["PreferredMfaSetting"].as_str().map(str::to_string),
            ),
            (reported.settings.clone(), reported.preferred.clone()),
            "case {case:?}: AdminGetUser must report the same MFA state, got {stored}"
        );
    }
}

/// One row of the user-naming table: a value submitted as `Username` that names
/// the one stored user.
#[derive(Clone, Copy, Debug)]
enum Named {
    /// The stored username itself.
    Username,
    /// The user's own `sub`, which `AdminGetUser` reads the same way.
    Sub,
    /// A verified alias attribute's value, for a pool that configures that
    /// attribute as an alias.
    Alias(&'static str),
}

#[tokio::test]
async fn admin_get_user_auth_factors_resolves_every_way_a_request_names_one_user() {
    let instance = emulator();
    // The pool configures both alias attributes, so an alias value names the user
    // as well as the username does.
    let user_pool_id = pool(
        &instance.router,
        "admin-factors-naming-pool",
        &[PASSWORD, EMAIL_OTP],
        &["email", "phone_number"],
    )
    .await;
    user(
        &instance.router,
        &user_pool_id,
        "ada",
        &[
            ("email", "ada.lovelace@example.com"),
            ("email_verified", "true"),
            ("phone_number", "+15550100007"),
            ("phone_number_verified", "true"),
        ],
    )
    .await;
    let stored = ok(
        &instance.router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "ada" }),
    )
    .await;
    let sub = stored["UserAttributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == json!("sub"))
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("the minted sub")
        .to_string();

    let rows = [
        Named::Username,
        Named::Sub,
        Named::Alias("ada.lovelace@example.com"),
        Named::Alias("+15550100007"),
    ];
    for row in rows {
        let named = match row {
            Named::Username => "ada".to_string(),
            Named::Sub => sub.clone(),
            Named::Alias(value) => value.to_string(),
        };
        let reported = factors(&instance.router, &user_pool_id, &named).await;
        // Whatever the request named the user by, the answer reports the STORED
        // username.
        assert_eq!(
            reported.username, "ada",
            "case {row:?}: expected the stored username, got {reported:?}"
        );
        assert_eq!(
            reported.configured,
            vec![PASSWORD.to_string(), EMAIL_OTP.to_string()],
            "case {row:?}: expected both admitted factors, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn admin_get_user_auth_factors_is_a_pure_read_of_the_stored_user() {
    let instance = emulator();
    let user_pool_id = pool(
        &instance.router,
        "admin-factors-pure-read-pool",
        &[PASSWORD, SMS_OTP],
        &[],
    )
    .await;
    user(&instance.router, &user_pool_id, "grace", VERIFIED_PHONE).await;

    let first = factors(&instance.router, &user_pool_id, "grace").await;
    assert_eq!(
        first.configured,
        vec![PASSWORD.to_string(), SMS_OTP.to_string()],
        "case \"both admitted factors are reported\": got {first:?}"
    );

    // Asking again answers the same, and the user the first call read is
    // unchanged: a read activates no factor and moves no preference.
    let second = factors(&instance.router, &user_pool_id, "grace").await;
    assert_eq!(
        second, first,
        "case \"asking twice answers the same\": expected {first:?}, got {second:?}"
    );
    let stored = ok(
        &instance.router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "grace" }),
    )
    .await;
    assert!(
        stored["UserMFASettingList"].is_null() && stored["PreferredMfaSetting"].is_null(),
        "case \"a read activates no MFA factor\": got {stored}"
    );
}

/// One row of the refusal table.
#[derive(Clone, Copy, Debug)]
struct RefusalCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the request names the created pool; otherwise it names a pool id
    /// of the right shape that no `CreateUserPool` minted.
    existing_pool: bool,
    /// The `Username` the request submits.
    username: &'static str,
    /// The error code and message the refusal must carry.
    code: &'static str,
    message: &'static str,
}

/// A pool id satisfying `UserPoolIdType`'s pattern that no pool was created
/// under, so the request reaches the handler and the refusal is the handler's
/// own.
const ABSENT_POOL_ID: &str = "us-east-1_absentpool";

/// The status this endpoint answers every refusal of the CALLER with: the model
/// declares 404 for both classes below, while this `awsJson1_1` endpoint carries
/// the class in the body's `__type` and answers one status for all of them (as
/// the `error_wire` suite pins for this whole surface).
const REFUSED: u16 = 400;

/// The rows differ along WHICH named resource is missing: the pool, or a user of
/// an existing pool — named by a value that is no username, no `sub` and no
/// sign-in name of it.
const REFUSALS: &[RefusalCase] = &[
    RefusalCase {
        case: "a pool id no pool was created under",
        existing_pool: false,
        username: "ada",
        code: "ResourceNotFoundException",
        message: "User pool us-east-1_absentpool does not exist.",
    },
    RefusalCase {
        case: "a username no user of the existing pool answers to",
        existing_pool: true,
        username: "grace",
        code: "UserNotFoundException",
        message: "User does not exist.",
    },
    RefusalCase {
        case: "an email address that is no sign-in name of the existing pool",
        existing_pool: true,
        username: "nobody@example.com",
        code: "UserNotFoundException",
        message: "User does not exist.",
    },
];

#[tokio::test]
async fn admin_get_user_auth_factors_refuses_a_pool_or_user_that_does_not_exist() {
    let instance = emulator();
    let user_pool_id = pool(&instance.router, "admin-factors-refusal-pool", &[], &[]).await;
    user(&instance.router, &user_pool_id, "ada", VERIFIED_EMAIL).await;

    for row in REFUSALS {
        let case = row.case;
        let named_pool = if row.existing_pool {
            user_pool_id.as_str()
        } else {
            ABSENT_POOL_ID
        };
        let (status, payload) = dispatch(
            &instance.router,
            "AdminGetUserAuthFactors",
            &json!({ "UserPoolId": named_pool, "Username": row.username }),
        )
        .await;
        assert_eq!(
            status, REFUSED,
            "case {case:?}: the refusal keeps this endpoint's status, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with(row.code),
            "case {case:?}: expected {:?}, got {code:?} with {payload}",
            row.code
        );
        let message = error_message(&payload);
        assert_eq!(
            message, row.message,
            "case {case:?}: expected the refusal to report {:?}, got {message:?}",
            row.message
        );
    }
}
