// SPDX-License-Identifier: CC0-1.0

//! `AdminSetUserMFAPreference` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.AdminSetUserMFAPreference`).
//!
//! The rule under test: an administrative write of one user's MFA preference
//! stores, for the STORED user the request's `UserPoolId` and `Username` resolve
//! to, what the submitted settings ask for — each submitted structure REPLACING
//! what that user held for the factor it configures, every factor the request
//! names none of left exactly as it was, and at most one factor preferred, since
//! "only one factor can be set as preferred" and a factor that is not activated
//! is not one the user prefers (`AdminSetUserMFAPreference` in this service's
//! model). Activating a factor needs the delivery configuration it reaches the
//! user through — a verified phone number for SMS message MFA, a verified email
//! address for email message MFA, a registered TOTP authenticator for software
//! token MFA — so a request that asks for one the user cannot receive is refused
//! with `InvalidParameterException`, as is one that asks for a factor to be
//! PREFERRED while not activating it; the whole request is checked before
//! anything is written, so a refused one leaves that user's activated factors and
//! preferred factor as they were. Turning a factor OFF needs no such
//! configuration. Passkey MFA is submitted on its own structure, which carries no
//! preference and is no `UserMFASettingList` setting. A request naming a pool that
//! does not exist is refused as `ResourceNotFoundException`, and one naming no
//! user of the named pool — including a user that exists only in ANOTHER pool — as
//! `UserNotFoundException`.
//!
//! What is stored is read back through `AdminGetUser` and
//! `AdminGetUserAuthFactors`, so each row asserts the preference a LATER caller
//! sees rather than the answer of the call that set it (this operation models no
//! output member at all).
//!
//! The pools and users are real: they come from `CreateUserPool`,
//! `SetUserPoolMfaConfig`, `AdminCreateUser` and `AdminSetUserPassword`. Two facts
//! are planted in this service's state rather than written through the operations
//! that own them: a user's registered TOTP authenticator (which
//! `AssociateSoftwareToken` and `VerifySoftwareToken` write, asserted by
//! [`super::software_token_setup`], so these rows need none of that arithmetic)
//! and a registered passkey (`CompleteWebAuthnRegistration`, still a scaffold
//! stub).
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

/// The `AuthFactorType` value a registered passkey offers
/// (`smithy_query(query="shape", shape="AuthFactorType")`).
const WEB_AUTHN: &str = "WEB_AUTHN";

/// The one `WebAuthnFactorConfigurationType` value under which a passkey
/// satisfies an MFA requirement
/// (`WebAuthnConfigurationType.FactorConfiguration` in this service's model); its
/// only other value makes passkeys a single authentication factor.
const PASSKEY_MULTI_FACTOR: &str = "MULTI_FACTOR_WITH_USER_VERIFICATION";

/// The only `DeliveryMediumType` and the only attribute an `MFAOptionType` may
/// carry: `MFAOptionType.DeliveryMedium` is one "you can use this parameter to set
/// only the `SMS` delivery medium value" for, and its `AttributeName`'s "only
/// valid value is `phone_number`" (both in this service's model).
const SMS_DELIVERY_MEDIUM: &str = "SMS";
const SMS_OPTION_ATTRIBUTE: &str = "phone_number";

/// A pool id satisfying `UserPoolIdType`'s pattern that no pool was created
/// under, so the request reaches the handler and the refusal is the handler's
/// own.
const ABSENT_POOL_ID: &str = "us-east-1_absentpool";

/// The status this endpoint answers every refusal of the CALLER with: the model
/// declares 400 for `InvalidParameterException` and 404 for both not-found
/// classes, while this `awsJson1_1` endpoint carries the class in the body's
/// `__type` and answers one status for all of them (as the `error_wire` suite
/// pins for this whole surface).
const REFUSED: u16 = 400;

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

/// Create a pool and return its minted `UserPoolId`.
/// `passkey_factor_configuration` is the
/// `WebAuthnConfiguration.FactorConfiguration` the pool is given, when the caller
/// wants one.
async fn pool(
    router: &MultiServiceRouter,
    name: &str,
    passkey_factor_configuration: Option<&str>,
) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    if let Some(factor_configuration) = passkey_factor_configuration {
        // `WebAuthnConfigurationType.FactorConfiguration` is what enables passkey
        // MFA at the user pool level; a relying party and a user-verification
        // requirement go with it.
        ok(
            router,
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
    user_pool_id
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

/// The MFA state a later administrative read of one user reports.
#[derive(Debug, PartialEq, Eq)]
struct Stored {
    /// `UserMFASettingList`; empty where the response carries none.
    settings: Vec<String>,
    /// `PreferredMfaSetting`, absent where the response carries none.
    preferred: Option<String>,
    /// `MFAOptions`, the legacy SMS-only view of the same state; empty where the
    /// response carries none.
    sms_options: Vec<(String, String)>,
}

/// Read `username` of `user_pool_id` back through `AdminGetUser`.
async fn stored(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> Stored {
    let payload = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    Stored {
        settings: string_list(&payload, "UserMFASettingList"),
        preferred: payload["PreferredMfaSetting"]
            .as_str()
            .map(str::to_string),
        sms_options: payload["MFAOptions"]
            .as_array()
            .map(|options| {
                options
                    .iter()
                    .map(|option| {
                        (
                            option["DeliveryMedium"].as_str().unwrap_or_default().to_string(),
                            option["AttributeName"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The `ConfiguredUserAuthFactors` the same user's `AdminGetUserAuthFactors`
/// reports.
async fn configured_factors(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> Vec<String> {
    let payload = ok(
        router,
        "AdminGetUserAuthFactors",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    string_list(&payload, "ConfiguredUserAuthFactors")
}

/// The `AdminSetUserMFAPreference` body one row submits: only the structures the
/// row names are present, as an SDK sends only the settings the caller passed.
fn preference_body(
    user_pool_id: &str,
    username: &str,
    sms: Option<(bool, bool)>,
    software_token: Option<(bool, bool)>,
    email: Option<(bool, bool)>,
) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Username".to_string(), json!(username));
    for (member, settings) in [
        ("SMSMfaSettings", sms),
        ("SoftwareTokenMfaSettings", software_token),
        ("EmailMfaSettings", email),
    ] {
        if let Some((enabled, preferred)) = settings {
            body.insert(
                member.to_string(),
                json!({ "Enabled": enabled, "PreferredMfa": preferred }),
            );
        }
    }
    Value::Object(body)
}

/// What one row expects of the call and of the preference stored after it.
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// The call succeeds and the user holds these settings, with this preferred
    /// factor.
    Stored {
        settings: &'static [&'static str],
        preferred: Option<&'static str>,
    },
    /// The call is refused for ACTIVATING this `UserMFASettingList` setting while
    /// the user holds no delivery configuration for it.
    NoDeliveryConfig { setting: &'static str },
    /// The call is refused for asking that a factor be PREFERRED while the
    /// request does not activate it.
    PreferredWhileDisabled,
}

/// One row of the preference table.
#[derive(Clone, Copy, Debug)]
struct Case {
    /// What this row asserts.
    case: &'static str,
    /// The attributes `AdminCreateUser` gives the user.
    attributes: &'static [(&'static str, &'static str)],
    /// Whether the user has registered a TOTP authenticator.
    software_token_registered: bool,
    /// `SMSMfaSettings` as `(Enabled, PreferredMfa)`, when the row submits it.
    sms: Option<(bool, bool)>,
    /// `SoftwareTokenMfaSettings` as `(Enabled, PreferredMfa)`, when submitted.
    software_token: Option<(bool, bool)>,
    /// `EmailMfaSettings` as `(Enabled, PreferredMfa)`, when submitted.
    email: Option<(bool, bool)>,
    expected: Expect,
}

/// A verified phone number, and an unverified one.
const VERIFIED_PHONE: &[(&str, &str)] = &[
    ("phone_number", "+15550100001"),
    ("phone_number_verified", "true"),
];
const UNVERIFIED_PHONE: &[(&str, &str)] = &[("phone_number", "+15550100002")];

/// A verified email address, and an unverified one.
const VERIFIED_EMAIL: &[(&str, &str)] = &[("email", "ada@example.com"), ("email_verified", "true")];
const UNVERIFIED_EMAIL: &[(&str, &str)] = &[("email", "grace@example.com")];

/// A user holding a verified address for both factors that are delivered to one.
const VERIFIED_BOTH: &[(&str, &str)] = &[
    ("email", "ada@example.com"),
    ("email_verified", "true"),
    ("phone_number", "+15550100003"),
    ("phone_number_verified", "true"),
];

/// The rows differ along WHICH factor the request configures, HOW MANY factors it
/// configures at once, whether it activates, prefers or deactivates each, and WHAT
/// the user holds to receive that factor with (a verified address, an unverified
/// one, none at all, a registered TOTP authenticator or none).
const CASES: &[Case] = &[
    Case {
        case: "SMS MFA activated and preferred for a user with a verified phone number",
        attributes: VERIFIED_PHONE,
        software_token_registered: false,
        sms: Some((true, true)),
        software_token: None,
        email: None,
        expected: Expect::Stored {
            settings: &[SMS_MFA],
            preferred: Some(SMS_MFA),
        },
    },
    Case {
        case: "email message MFA activated and preferred for a user with a verified email address",
        attributes: VERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: None,
        email: Some((true, true)),
        expected: Expect::Stored {
            settings: &[EMAIL_OTP_MFA],
            preferred: Some(EMAIL_OTP_MFA),
        },
    },
    Case {
        case: "software token MFA activated and preferred for a user who registered an \
               authenticator",
        attributes: &[],
        software_token_registered: true,
        sms: None,
        software_token: Some((true, true)),
        email: None,
        expected: Expect::Stored {
            settings: &[SOFTWARE_TOKEN_MFA],
            preferred: Some(SOFTWARE_TOKEN_MFA),
        },
    },
    Case {
        case: "a factor activated without a preference leaves the user preferring none",
        attributes: VERIFIED_PHONE,
        software_token_registered: false,
        sms: Some((true, false)),
        software_token: None,
        email: None,
        expected: Expect::Stored {
            settings: &[SMS_MFA],
            preferred: None,
        },
    },
    Case {
        case: "two factors activated at once, one of them preferred",
        attributes: VERIFIED_BOTH,
        software_token_registered: false,
        sms: Some((true, false)),
        software_token: None,
        email: Some((true, true)),
        expected: Expect::Stored {
            settings: &[SMS_MFA, EMAIL_OTP_MFA],
            preferred: Some(EMAIL_OTP_MFA),
        },
    },
    Case {
        case: "three factors activated at once, the TOTP one preferred",
        attributes: VERIFIED_BOTH,
        software_token_registered: true,
        sms: Some((true, false)),
        software_token: Some((true, true)),
        email: Some((true, false)),
        expected: Expect::Stored {
            settings: &[SMS_MFA, SOFTWARE_TOKEN_MFA, EMAIL_OTP_MFA],
            preferred: Some(SOFTWARE_TOKEN_MFA),
        },
    },
    Case {
        case: "turning every factor off needs no delivery configuration at all",
        attributes: &[],
        software_token_registered: false,
        sms: Some((false, false)),
        software_token: Some((false, false)),
        email: Some((false, false)),
        expected: Expect::Stored {
            settings: &[],
            preferred: None,
        },
    },
    Case {
        case: "activating SMS MFA for a user holding no phone number at all is refused",
        attributes: &[],
        software_token_registered: false,
        sms: Some((true, false)),
        software_token: None,
        email: None,
        expected: Expect::NoDeliveryConfig { setting: SMS_MFA },
    },
    Case {
        case: "activating SMS MFA for a user whose phone number is not verified is refused",
        attributes: UNVERIFIED_PHONE,
        software_token_registered: false,
        sms: Some((true, true)),
        software_token: None,
        email: None,
        expected: Expect::NoDeliveryConfig { setting: SMS_MFA },
    },
    Case {
        case: "activating email message MFA for a user whose email address is not verified is \
               refused",
        attributes: UNVERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: None,
        email: Some((true, false)),
        expected: Expect::NoDeliveryConfig {
            setting: EMAIL_OTP_MFA,
        },
    },
    Case {
        case: "activating software token MFA before a TOTP authenticator is registered is refused",
        attributes: VERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: Some((true, true)),
        email: None,
        expected: Expect::NoDeliveryConfig {
            setting: SOFTWARE_TOKEN_MFA,
        },
    },
    Case {
        case: "one unreceivable factor refuses the whole request, storing not even the \
               receivable one",
        attributes: VERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: Some((true, true)),
        email: Some((true, false)),
        expected: Expect::NoDeliveryConfig {
            setting: SOFTWARE_TOKEN_MFA,
        },
    },
    Case {
        case: "preferring SMS MFA the request does not activate is refused for the preference",
        attributes: VERIFIED_PHONE,
        software_token_registered: false,
        sms: Some((false, true)),
        software_token: None,
        email: None,
        expected: Expect::PreferredWhileDisabled,
    },
    Case {
        case: "preferring email message MFA the request does not activate is refused for the \
               preference, even for a user that could receive it",
        attributes: VERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: None,
        email: Some((false, true)),
        expected: Expect::PreferredWhileDisabled,
    },
    Case {
        case: "preferring software token MFA the request does not activate is refused for the \
               preference, before the preference is even receivable",
        attributes: &[],
        software_token_registered: false,
        sms: None,
        software_token: Some((false, true)),
        email: None,
        expected: Expect::PreferredWhileDisabled,
    },
];

/// The refusal an unreceivable activation reports, which names the factor it
/// refused.
fn no_delivery_config_message(setting: &str) -> String {
    format!("User does not have delivery config set to turn on {setting}")
}

/// The refusal a preference over a factor the request leaves off reports, which
/// names no factor.
const PREFERRED_WHILE_DISABLED_MESSAGE: &str = "Can't mark a disabled mfa as preferred";

/// Which refusal a mixed request earns.
#[derive(Clone, Copy, Debug)]
enum Refusal {
    /// The `UserMFASettingList` setting the request asks to ACTIVATE while the
    /// user holds no delivery configuration for it.
    NoDeliveryConfig(&'static str),
    /// A factor the request asks to PREFER while not activating it.
    PreferredWhileDisabled,
}

/// One row of the mixed-request table: a request whose halves would be served and
/// refused on their own, submitted together.
#[derive(Clone, Copy, Debug)]
struct MixedCase {
    /// What this row asserts.
    case: &'static str,
    /// `SMSMfaSettings`, `SoftwareTokenMfaSettings` and `EmailMfaSettings` as
    /// `(Enabled, PreferredMfa)`, each only when the row submits it.
    sms: Option<(bool, bool)>,
    software_token: Option<(bool, bool)>,
    email: Option<(bool, bool)>,
    refusal: Refusal,
}

/// Each row asks for something the user cannot have, while ALSO asking for a
/// change that would be served on its own: the whole request is checked before
/// anything is written, so neither half lands. The rows differ along which half
/// is refused, and along what the other half asks for.
const MIXED_REFUSALS: &[MixedCase] = &[
    MixedCase {
        case: "an unreceivable activation beside a deactivation that would be served",
        sms: None,
        software_token: Some((true, true)),
        email: Some((false, false)),
        refusal: Refusal::NoDeliveryConfig(SOFTWARE_TOKEN_MFA),
    },
    MixedCase {
        case: "an unreceivable activation beside an activation that would be served",
        sms: Some((true, false)),
        software_token: None,
        email: Some((true, false)),
        refusal: Refusal::NoDeliveryConfig(SMS_MFA),
    },
    MixedCase {
        case: "a preference over a factor the request leaves off, beside a served deactivation",
        sms: Some((false, true)),
        software_token: None,
        email: Some((false, false)),
        refusal: Refusal::PreferredWhileDisabled,
    },
];

#[tokio::test]
async fn admin_set_user_mfa_preference_stores_the_factors_the_user_can_receive() {
    for (index, row) in CASES.iter().enumerate() {
        let case = row.case;
        let instance = emulator();
        let user_pool_id = pool(
            &instance.router,
            &format!("admin-preference-pool-{index}"),
            None,
        )
        .await;
        user(&instance.router, &user_pool_id, "ada", row.attributes).await;
        if row.software_token_registered {
            register_software_token(&instance, &user_pool_id, "ada");
        }

        let (status, payload) = dispatch(
            &instance.router,
            "AdminSetUserMFAPreference",
            &preference_body(
                &user_pool_id,
                "ada",
                row.sms,
                row.software_token,
                row.email,
            ),
        )
        .await;

        let expected_settings: Vec<String> = match row.expected {
            Expect::Stored { settings, .. } => settings.iter().map(|s| (*s).to_string()).collect(),
            // A refused request writes nothing, so the user still holds what it
            // held when it was created: no activated factor and no preference.
            _ => Vec::new(),
        };
        let expected_preferred = match row.expected {
            Expect::Stored { preferred, .. } => preferred.map(str::to_string),
            _ => None,
        };
        match row.expected {
            Expect::Stored { .. } => assert_eq!(
                status, 200,
                "case {case:?}: the request must be served, got {status} with {payload}"
            ),
            Expect::NoDeliveryConfig { setting } => {
                assert_refused(
                    case,
                    status,
                    &payload,
                    "InvalidParameterException",
                    &no_delivery_config_message(setting),
                );
            }
            Expect::PreferredWhileDisabled => {
                assert_refused(
                    case,
                    status,
                    &payload,
                    "InvalidParameterException",
                    PREFERRED_WHILE_DISABLED_MESSAGE,
                );
            }
        }

        // What the call asserted is the preference a LATER read of the same user
        // renders: this operation models no output member of its own.
        let read = stored(&instance.router, &user_pool_id, "ada").await;
        assert_eq!(
            (read.settings.clone(), read.preferred.clone()),
            (expected_settings.clone(), expected_preferred.clone()),
            "case {case:?}: expected {expected_settings:?} preferring \
             {expected_preferred:?}, got {read:?}"
        );
        // SMS message MFA is activated exactly while the one MFA option that
        // configures it is stored for the user, which is what the legacy
        // `MFAOptions` view reports.
        let expected_options: Vec<(String, String)> = if expected_settings
            .iter()
            .any(|setting| setting == SMS_MFA)
        {
            vec![(
                SMS_DELIVERY_MEDIUM.to_string(),
                SMS_OPTION_ATTRIBUTE.to_string(),
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            read.sms_options, expected_options,
            "case {case:?}: expected MFAOptions {expected_options:?}, got {:?}",
            read.sms_options
        );
        // One user holds one MFA state, whichever administrative read renders it.
        let factors = ok(
            &instance.router,
            "AdminGetUserAuthFactors",
            &json!({ "UserPoolId": user_pool_id, "Username": "ada" }),
        )
        .await;
        assert_eq!(
            (
                string_list(&factors, "UserMFASettingList"),
                factors["PreferredMfaSetting"].as_str().map(str::to_string),
            ),
            (expected_settings.clone(), expected_preferred.clone()),
            "case {case:?}: AdminGetUserAuthFactors must report the same MFA state, \
             got {factors}"
        );
    }
}

/// Assert that `status` and `payload` are the refusal `code` carrying `message`.
fn assert_refused(case: &str, status: u16, payload: &Value, code: &str, message: &str) {
    assert_eq!(
        status, REFUSED,
        "case {case:?}: the refusal keeps this endpoint's status, got {status} with {payload}"
    );
    let reported = error_code(payload);
    assert!(
        reported.ends_with(code),
        "case {case:?}: expected {code:?}, got {reported:?} with {payload}"
    );
    let reported = error_message(payload);
    assert_eq!(
        reported, message,
        "case {case:?}: expected the refusal to report {message:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn admin_set_user_mfa_preference_replaces_only_the_factors_the_request_names() {
    let instance = emulator();
    let user_pool_id = pool(&instance.router, "admin-preference-partial-pool", None).await;
    user(&instance.router, &user_pool_id, "ada", VERIFIED_BOTH).await;
    register_software_token(&instance, &user_pool_id, "ada");

    // All three factors on at once, the TOTP one preferred.
    ok(
        &instance.router,
        "AdminSetUserMFAPreference",
        &preference_body(
            &user_pool_id,
            "ada",
            Some((true, false)),
            Some((true, true)),
            Some((true, false)),
        ),
    )
    .await;
    let read = stored(&instance.router, &user_pool_id, "ada").await;
    assert_eq!(
        (read.settings.clone(), read.preferred.clone()),
        (
            vec![
                SMS_MFA.to_string(),
                SOFTWARE_TOKEN_MFA.to_string(),
                EMAIL_OTP_MFA.to_string()
            ],
            Some(SOFTWARE_TOKEN_MFA.to_string())
        ),
        "case \"three factors on, the TOTP one preferred\": got {read:?}"
    );

    // A request that names ONE factor replaces that factor and leaves the other
    // two exactly as they were, preference included.
    ok(
        &instance.router,
        "AdminSetUserMFAPreference",
        &preference_body(&user_pool_id, "ada", None, None, Some((false, false))),
    )
    .await;
    let read = stored(&instance.router, &user_pool_id, "ada").await;
    assert_eq!(
        (read.settings.clone(), read.preferred.clone()),
        (
            vec![SMS_MFA.to_string(), SOFTWARE_TOKEN_MFA.to_string()],
            Some(SOFTWARE_TOKEN_MFA.to_string())
        ),
        "case \"deactivating one factor leaves the others and the preference alone\": got {read:?}"
    );

    // Submitting the preferred factor as no longer preferred, while leaving it
    // activated, leaves the user preferring none — and no other factor inherits
    // the preference.
    ok(
        &instance.router,
        "AdminSetUserMFAPreference",
        &preference_body(&user_pool_id, "ada", None, Some((true, false)), None),
    )
    .await;
    let read = stored(&instance.router, &user_pool_id, "ada").await;
    assert_eq!(
        (read.settings.clone(), read.preferred.clone()),
        (
            vec![SMS_MFA.to_string(), SOFTWARE_TOKEN_MFA.to_string()],
            None
        ),
        "case \"a factor submitted as no longer preferred leaves no preference\": got {read:?}"
    );

    // Moving the preference to another activated factor replaces the preference
    // rather than adding a second one: only one factor can be set as preferred.
    ok(
        &instance.router,
        "AdminSetUserMFAPreference",
        &preference_body(&user_pool_id, "ada", Some((true, true)), None, None),
    )
    .await;
    let read = stored(&instance.router, &user_pool_id, "ada").await;
    assert_eq!(
        (read.settings.clone(), read.preferred.clone()),
        (
            vec![SOFTWARE_TOKEN_MFA.to_string(), SMS_MFA.to_string()],
            Some(SMS_MFA.to_string())
        ),
        "case \"the preference moves to the factor the request prefers\": got {read:?}"
    );

    // Submitting the same body again asks for the same state, so the user holds
    // the same thing: the write is idempotent.
    ok(
        &instance.router,
        "AdminSetUserMFAPreference",
        &preference_body(&user_pool_id, "ada", Some((true, true)), None, None),
    )
    .await;
    let again = stored(&instance.router, &user_pool_id, "ada").await;
    assert_eq!(
        again, read,
        "case \"resubmitting the same preference stores the same state\": expected \
         {read:?}, got {again:?}"
    );

    // Deactivating the PREFERRED factor takes the preference with it: a factor
    // that is off is not one the user prefers.
    ok(
        &instance.router,
        "AdminSetUserMFAPreference",
        &preference_body(&user_pool_id, "ada", Some((false, false)), None, None),
    )
    .await;
    let read = stored(&instance.router, &user_pool_id, "ada").await;
    assert_eq!(
        (read.settings.clone(), read.preferred.clone()),
        (vec![SOFTWARE_TOKEN_MFA.to_string()], None),
        "case \"deactivating the preferred factor leaves no preference\": got {read:?}"
    );
}

#[tokio::test]
async fn admin_set_user_mfa_preference_leaves_a_refused_users_preference_as_it_was() {
    let instance = emulator();
    let user_pool_id = pool(&instance.router, "admin-preference-refused-pool", None).await;
    user(&instance.router, &user_pool_id, "ada", VERIFIED_EMAIL).await;
    ok(
        &instance.router,
        "AdminSetUserMFAPreference",
        &preference_body(&user_pool_id, "ada", None, None, Some((true, true))),
    )
    .await;
    let before = stored(&instance.router, &user_pool_id, "ada").await;
    assert_eq!(
        (before.settings.clone(), before.preferred.clone()),
        (
            vec![EMAIL_OTP_MFA.to_string()],
            Some(EMAIL_OTP_MFA.to_string())
        ),
        "case \"email message MFA is the stored preference to begin with\": got {before:?}"
    );

    for row in MIXED_REFUSALS {
        let case = row.case;
        let (status, payload) = dispatch(
            &instance.router,
            "AdminSetUserMFAPreference",
            &preference_body(&user_pool_id, "ada", row.sms, row.software_token, row.email),
        )
        .await;
        let message = match row.refusal {
            Refusal::NoDeliveryConfig(setting) => no_delivery_config_message(setting),
            Refusal::PreferredWhileDisabled => PREFERRED_WHILE_DISABLED_MESSAGE.to_string(),
        };
        assert_refused(case, status, &payload, "InvalidParameterException", &message);
        let after = stored(&instance.router, &user_pool_id, "ada").await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused request must leave the user as it was, expected \
             {before:?}, got {after:?}"
        );
    }
}

#[tokio::test]
async fn admin_set_user_mfa_preference_stores_the_passkey_setting_on_its_own() {
    let instance = emulator();
    // The pool admits passkeys as an MFA factor, and the user holds one, so the
    // passkey factor is reported until the user deactivates passkey MFA.
    let user_pool_id = pool(
        &instance.router,
        "admin-preference-passkey-pool",
        Some(PASSKEY_MULTI_FACTOR),
    )
    .await;
    user(&instance.router, &user_pool_id, "ada", VERIFIED_EMAIL).await;
    register_passkey(&instance, &user_pool_id, "ada");
    let offered = configured_factors(&instance.router, &user_pool_id, "ada").await;
    assert!(
        offered.iter().any(|factor| factor == WEB_AUTHN),
        "case \"a registered passkey is an MFA factor of a pool that admits one\": got \
         {offered:?}"
    );

    for row in PASSKEY_CASES {
        let case = row.case;
        let mut body = preference_body(&user_pool_id, "ada", None, None, row.email);
        body["WebAuthnMfaSettings"] = json!({ "Enabled": row.enabled });
        ok(&instance.router, "AdminSetUserMFAPreference", &body).await;

        let factors = configured_factors(&instance.router, &user_pool_id, "ada").await;
        let reports_passkey = factors.iter().any(|factor| factor == WEB_AUTHN);
        assert_eq!(
            reports_passkey, row.offers_passkey,
            "case {case:?}: expected the passkey factor reported {}, got {factors:?}",
            row.offers_passkey
        );
        // Passkey MFA is no `UserMFASettingList` setting and carries no
        // preference, so it never appears among the activated settings.
        let read = stored(&instance.router, &user_pool_id, "ada").await;
        let expected: Vec<String> = row.settings.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            (read.settings.clone(), read.preferred.clone()),
            (expected.clone(), row.preferred.map(str::to_string)),
            "case {case:?}: expected {expected:?} preferring {:?}, got {read:?}",
            row.preferred
        );
    }
}

/// One row of the passkey table.
#[derive(Clone, Copy, Debug)]
struct PasskeyCase {
    /// What this row asserts.
    case: &'static str,
    /// `WebAuthnMfaSettings.Enabled`, the whole of what that structure carries.
    enabled: bool,
    /// `EmailMfaSettings` as `(Enabled, PreferredMfa)`, when the row submits it
    /// beside the passkey structure.
    email: Option<(bool, bool)>,
    /// Whether `ConfiguredUserAuthFactors` must then report the passkey factor.
    offers_passkey: bool,
    /// The `UserMFASettingList` and `PreferredMfaSetting` the user must then hold.
    settings: &'static [&'static str],
    preferred: Option<&'static str>,
}

/// The rows differ along what the passkey structure asks for, and along whether
/// the request submits a `UserMFASettingList` factor beside it.
const PASSKEY_CASES: &[PasskeyCase] = &[
    PasskeyCase {
        case: "passkey MFA deactivated on its own",
        enabled: false,
        email: None,
        offers_passkey: false,
        settings: &[],
        preferred: None,
    },
    PasskeyCase {
        case: "passkey MFA activated again on its own",
        enabled: true,
        email: None,
        offers_passkey: true,
        settings: &[],
        preferred: None,
    },
    PasskeyCase {
        case: "passkey MFA deactivated beside an email message MFA activation",
        enabled: false,
        email: Some((true, true)),
        offers_passkey: false,
        settings: &[EMAIL_OTP_MFA],
        preferred: Some(EMAIL_OTP_MFA),
    },
    PasskeyCase {
        case: "passkey MFA activated beside an email message MFA deactivation",
        enabled: true,
        email: Some((false, false)),
        offers_passkey: true,
        settings: &[],
        preferred: None,
    },
];

/// One row of the addressing table: which named resource the request fails to
/// resolve, and the refusal that reports it.
#[derive(Clone, Copy, Debug)]
struct AddressCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the request names the pool the user was created in; otherwise it
    /// names the OTHER created pool, or a pool id no `CreateUserPool` minted.
    named_pool: NamedPool,
    /// The `Username` the request submits.
    username: &'static str,
    /// The error code and message the refusal must carry.
    code: &'static str,
    message: &'static str,
}

/// Which pool a row's request names.
#[derive(Clone, Copy, Debug)]
enum NamedPool {
    /// The pool holding the user.
    Holding,
    /// Another pool this emulator holds, which the user is no user of.
    Other,
    /// A pool id of the right shape that no pool was created under.
    Absent,
}

/// The rows differ along WHICH named resource is missing: the pool itself, a user
/// of an existing pool, or a user that exists only in ANOTHER pool of the same
/// emulator.
const ADDRESSES: &[AddressCase] = &[
    AddressCase {
        case: "a pool id no pool was created under",
        named_pool: NamedPool::Absent,
        username: "ada",
        code: "ResourceNotFoundException",
        message: "User pool us-east-1_absentpool does not exist.",
    },
    AddressCase {
        case: "a username no user of the named pool answers to",
        named_pool: NamedPool::Holding,
        username: "grace",
        code: "UserNotFoundException",
        message: "User does not exist.",
    },
    AddressCase {
        case: "a user of another pool of the same emulator, named across the pool boundary",
        named_pool: NamedPool::Other,
        username: "ada",
        code: "UserNotFoundException",
        message: "User does not exist.",
    },
    AddressCase {
        case: "an email address that is no sign-in name of the named pool",
        named_pool: NamedPool::Holding,
        username: "nobody@example.com",
        code: "UserNotFoundException",
        message: "User does not exist.",
    },
];

#[tokio::test]
async fn admin_set_user_mfa_preference_refuses_a_pool_or_user_that_does_not_exist() {
    let instance = emulator();
    let holding = pool(&instance.router, "admin-preference-holding-pool", None).await;
    let other = pool(&instance.router, "admin-preference-other-pool", None).await;
    user(&instance.router, &holding, "ada", VERIFIED_EMAIL).await;
    ok(
        &instance.router,
        "AdminSetUserMFAPreference",
        &preference_body(&holding, "ada", None, None, Some((true, true))),
    )
    .await;
    let before = stored(&instance.router, &holding, "ada").await;

    for row in ADDRESSES {
        let case = row.case;
        let named = match row.named_pool {
            NamedPool::Holding => holding.as_str(),
            NamedPool::Other => other.as_str(),
            NamedPool::Absent => ABSENT_POOL_ID,
        };
        let (status, payload) = dispatch(
            &instance.router,
            "AdminSetUserMFAPreference",
            &preference_body(named, row.username, Some((true, true)), None, None),
        )
        .await;
        assert_refused(case, status, &payload, row.code, row.message);
        // The stored user of the pool that DOES hold one is untouched by a request
        // that resolved to no user at all.
        let after = stored(&instance.router, &holding, "ada").await;
        assert_eq!(
            after, before,
            "case {case:?}: the stored user must be untouched, expected {before:?}, \
             got {after:?}"
        );
    }
}
