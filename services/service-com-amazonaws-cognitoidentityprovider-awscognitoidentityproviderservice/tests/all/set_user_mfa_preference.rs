// SPDX-License-Identifier: CC0-1.0

//! `SetUserMFAPreference` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.SetUserMFAPreference`).
//!
//! The rule under test: the operation stores, for the user its `AccessToken`
//! names, the MFA settings the request submits — each submitted structure
//! replacing what that user held for the factor it configures and leaving every
//! factor the request does not name alone, with at most one factor preferred —
//! and refuses with HTTP 400 `InvalidParameterException`, changing nothing, any
//! request that asks to ACTIVATE a factor the user holds no delivery
//! configuration for: a verified phone number for SMS message MFA, a verified
//! email address for email message MFA, a registered TOTP authenticator for
//! software token MFA. A factor the request does not activate is not one it can
//! also make PREFERRED, so a setting that asks for the preference without the
//! activation is refused for the preference itself. Turning a factor OFF needs no
//! such configuration.
//!
//! What is stored is read back through `GetUserAuthFactors` and `GetUser`, so
//! each row asserts the preference a later caller sees rather than the answer of
//! the call that set it.
//!
//! The pools, app clients, users and access tokens are all real: they come from
//! `CreateUserPool`, `SetUserPoolMfaConfig`, `CreateUserPoolClient`,
//! `AdminCreateUser`, `AdminSetUserPassword` and `InitiateAuth`. Two facts are
//! planted in this service's state rather than written through the operations
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

/// The emulator's own endpoint: an address, so an unsigned request addressed to
/// it names no service in its `Host` header and `X-Amz-Target` is the only
/// routing signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

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
/// (`WebAuthnConfigurationType.FactorConfiguration` in this service's model).
const PASSKEY_MULTI_FACTOR: &str = "MULTI_FACTOR_WITH_USER_VERIFICATION";

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

/// Dispatch `operation` with `body` and return its status and JSON response
/// body. A `signed` call carries a credential scope, as the SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a signed-in user's client calls the self-service
/// operations the model marks `@optionalAuth`.
async fn dispatch(
    router: &MultiServiceRouter,
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

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, true).await;
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

/// A pool with one app client, one signed-in user and that user's tokens.
struct Fixture {
    user_pool_id: String,
    username: String,
    access_token: String,
    id_token: String,
}

/// Create a pool named `name` holding user `ada` with `attributes`, sign that
/// user in and return the fixture. `passkey_factor_configuration` is the
/// `WebAuthnConfiguration.FactorConfiguration` the pool is given, when the
/// caller wants one.
async fn fixture(
    router: &MultiServiceRouter,
    name: &str,
    attributes: &[(&str, &str)],
    passkey_factor_configuration: Option<&str>,
) -> Fixture {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    if let Some(factor_configuration) = passkey_factor_configuration {
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
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "preference-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    let username = "ada";
    let mut created_user = Map::new();
    created_user.insert("UserPoolId".to_string(), json!(user_pool_id));
    created_user.insert("Username".to_string(), json!(username));
    created_user.insert(
        "UserAttributes".to_string(),
        Value::Array(
            attributes
                .iter()
                .map(|(name, value)| json!({ "Name": name, "Value": value }))
                .collect(),
        ),
    );
    ok(router, "AdminCreateUser", &Value::Object(created_user)).await;
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
    let (status, payload) = dispatch(
        router,
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
        "signing {username} in must succeed, got {status} with {payload}"
    );
    let result = &payload["AuthenticationResult"];
    Fixture {
        user_pool_id,
        username: username.to_string(),
        access_token: result["AccessToken"]
            .as_str()
            .expect("an access token")
            .to_string(),
        id_token: result["IdToken"].as_str().expect("an id token").to_string(),
    }
}

/// Plant the TOTP authenticator registration `AssociateSoftwareToken` and
/// `VerifySoftwareToken` write, so the rows that need a user who HAS registered
/// one are driven without this suite computing one-time passwords.
fn register_software_token(emulator: &Emulator, user_pool_id: &str, username: &str) {
    let service_state = state::service_state(&*emulator.engine).expect("this service's state");
    let mut guard = service_state.lock();
    let pool = guard.pool_mut(user_pool_id).expect("the created pool");
    let user = pool.user_mut(username).expect("the created user");
    user.software_token_registered = true;
}

/// Plant one registered passkey for the user, which
/// `CompleteWebAuthnRegistration` will write once that scaffold is implemented.
fn register_passkey(emulator: &Emulator, user_pool_id: &str, username: &str) {
    let service_state = state::service_state(&*emulator.engine).expect("this service's state");
    let mut guard = service_state.lock();
    let pool = guard.pool_mut(user_pool_id).expect("the created pool");
    let user = pool.user_mut(username).expect("the created user");
    user.web_authn_credentials.push(state::WebAuthnCredential {
        credential_id: "planted-credential".to_string(),
        friendly_credential_name: "planted passkey".to_string(),
        relying_party_id: "auth.example.com".to_string(),
        authenticator_attachment: None,
        authenticator_transports: vec!["internal".to_string()],
        created_at: SystemTime::now(),
    });
}

/// The request body one row submits: only the structures the row names are
/// present, as an SDK sends only the settings the caller passed.
fn preference_body(access_token: &str, row: &Case) -> Value {
    let mut body = Map::new();
    body.insert("AccessToken".to_string(), json!(access_token));
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
    Value::Object(body)
}

/// What one `GetUserAuthFactors` answer reports about the stored preference.
#[derive(Debug, PartialEq, Eq)]
struct Preference {
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

/// The stored preference, read back through `GetUserAuthFactors`.
async fn stored_preference(router: &MultiServiceRouter, access_token: &str) -> Preference {
    let (status, payload) = dispatch(
        router,
        "GetUserAuthFactors",
        &json!({ "AccessToken": access_token }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "GetUserAuthFactors must succeed, got {status} with {payload}"
    );
    Preference {
        settings: string_list(&payload, "UserMFASettingList"),
        preferred: payload["PreferredMfaSetting"]
            .as_str()
            .map(str::to_string),
    }
}

/// The `ConfiguredUserAuthFactors` the same read reports.
async fn configured_factors(router: &MultiServiceRouter, access_token: &str) -> Vec<String> {
    let (status, payload) = dispatch(
        router,
        "GetUserAuthFactors",
        &json!({ "AccessToken": access_token }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "GetUserAuthFactors must succeed, got {status} with {payload}"
    );
    string_list(&payload, "ConfiguredUserAuthFactors")
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
    /// the user holds no delivery configuration for it, and the user holds
    /// nothing.
    Refused { setting: &'static str },
    /// The call is refused for asking that a factor be PREFERRED while the
    /// request does not activate it, and the user holds nothing.
    RefusedPreference,
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

/// The rows differ along WHICH factor the request configures, HOW MANY factors
/// it configures at once, whether it activates, prefers or deactivates each, and
/// WHAT the user holds to receive that factor with (a verified address, an
/// unverified one, none at all, a registered TOTP authenticator or none).
const CASES: &[Case] = &[
    Case {
        case: "SMS MFA activated and preferred by a user with a verified phone number",
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
        case: "email message MFA activated and preferred by a user with a verified email address",
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
        case: "software token MFA activated and preferred by a user who registered an \
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
        case: "turning factors off needs no delivery configuration at all",
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
        expected: Expect::Refused { setting: SMS_MFA },
    },
    Case {
        case: "activating and preferring SMS MFA for a user whose phone number is not verified \
               is refused for the delivery configuration",
        attributes: UNVERIFIED_PHONE,
        software_token_registered: false,
        sms: Some((true, true)),
        software_token: None,
        email: None,
        expected: Expect::Refused { setting: SMS_MFA },
    },
    Case {
        case: "activating email message MFA for a user whose email address is not verified is \
               refused",
        attributes: UNVERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: None,
        email: Some((true, false)),
        expected: Expect::Refused {
            setting: EMAIL_OTP_MFA,
        },
    },
    Case {
        case: "activating and preferring email message MFA for a user whose email address is \
               not verified is refused for the delivery configuration too",
        attributes: UNVERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: None,
        email: Some((true, true)),
        expected: Expect::Refused {
            setting: EMAIL_OTP_MFA,
        },
    },
    Case {
        case: "activating and preferring software token MFA before a TOTP authenticator is \
               registered is refused for the delivery configuration",
        attributes: VERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: Some((true, true)),
        email: None,
        expected: Expect::Refused {
            setting: SOFTWARE_TOKEN_MFA,
        },
    },
    Case {
        case: "activating software token MFA before a TOTP authenticator is registered, without \
               preferring it, is refused for the delivery configuration",
        attributes: VERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: Some((true, false)),
        email: None,
        expected: Expect::Refused {
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
        expected: Expect::RefusedPreference,
    },
    Case {
        case: "preferring email message MFA the request does not activate is refused for the \
               preference, even for a user that could receive it",
        attributes: VERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: None,
        email: Some((false, true)),
        expected: Expect::RefusedPreference,
    },
    Case {
        case: "preferring a factor the request does not even activate is refused just the same",
        attributes: &[],
        software_token_registered: false,
        sms: None,
        software_token: Some((false, true)),
        email: None,
        expected: Expect::RefusedPreference,
    },
    Case {
        case: "one unreceivable factor refuses the whole request, storing not even the \
               receivable one",
        attributes: VERIFIED_EMAIL,
        software_token_registered: false,
        sms: None,
        software_token: Some((true, true)),
        email: Some((true, false)),
        expected: Expect::Refused {
            setting: SOFTWARE_TOKEN_MFA,
        },
    },
];

#[tokio::test]
async fn set_user_mfa_preference_stores_the_factors_the_user_can_receive() {
    for (index, row) in CASES.iter().enumerate() {
        let case = row.case;
        let instance = emulator();
        let user = fixture(
            &instance.router,
            &format!("preference-pool-{index}"),
            row.attributes,
            None,
        )
        .await;
        if row.software_token_registered {
            register_software_token(&instance, &user.user_pool_id, &user.username);
        }

        let (status, payload) = dispatch(
            &instance.router,
            "SetUserMFAPreference",
            &preference_body(&user.access_token, row),
            false,
        )
        .await;
        let expected = match row.expected {
            Expect::Stored {
                settings,
                preferred,
            } => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the preference must be stored, got {status} with {payload}"
                );
                Preference {
                    settings: settings.iter().map(|s| (*s).to_string()).collect(),
                    preferred: preferred.map(str::to_string),
                }
            }
            Expect::Refused { setting } => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the refusal is a 400, got {status} with {payload}"
                );
                let code = error_code(&payload);
                assert!(
                    code.ends_with("InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {code:?} with {payload}"
                );
                let message = error_message(&payload);
                let wanted = format!("User does not have delivery config set to turn on {setting}");
                assert_eq!(
                    message, wanted,
                    "case {case:?}: expected the refusal to report {wanted:?}, got {message:?}"
                );
                // A refused request changes nothing, so the user still holds the
                // preference it was created with: none.
                Preference {
                    settings: Vec::new(),
                    preferred: None,
                }
            }
            Expect::RefusedPreference => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the refusal is a 400, got {status} with {payload}"
                );
                let code = error_code(&payload);
                assert!(
                    code.ends_with("InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {code:?} with {payload}"
                );
                let message = error_message(&payload);
                // The sentence names no factor: a factor the request leaves off is
                // not one a user prefers, whichever factor the request named.
                let wanted = "Can't mark a disabled mfa as preferred";
                assert_eq!(
                    message, wanted,
                    "case {case:?}: expected the refusal to report {wanted:?}, got {message:?}"
                );
                Preference {
                    settings: Vec::new(),
                    preferred: None,
                }
            }
        };

        let stored = stored_preference(&instance.router, &user.access_token).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected {expected:?}, got {stored:?}"
        );
    }
}

/// One step of the sequence below: the settings one call submits, and what the
/// user holds afterwards.
struct Step {
    /// What this step asserts.
    case: &'static str,
    sms: Option<(bool, bool)>,
    software_token: Option<(bool, bool)>,
    email: Option<(bool, bool)>,
    settings: &'static [&'static str],
    preferred: Option<&'static str>,
}

#[tokio::test]
async fn set_user_mfa_preference_replaces_only_the_factors_the_request_names() {
    let instance = emulator();
    let user = fixture(&instance.router, "sequence-pool", VERIFIED_BOTH, None).await;
    register_software_token(&instance, &user.user_pool_id, &user.username);

    // Each step submits one request and asserts what the user holds after it, so
    // every row's expectation is about the STORED preference the step before it
    // left plus what this step asks for.
    let steps = [
        Step {
            case: "the first call activates SMS MFA and prefers it",
            sms: Some((true, true)),
            software_token: None,
            email: None,
            settings: &[SMS_MFA],
            preferred: Some(SMS_MFA),
        },
        Step {
            case: "a call that names only email MFA leaves SMS MFA activated and moves the \
                   preference, because only one factor can be preferred",
            sms: None,
            software_token: None,
            email: Some((true, true)),
            settings: &[SMS_MFA, EMAIL_OTP_MFA],
            preferred: Some(EMAIL_OTP_MFA),
        },
        Step {
            case: "repeating that same call stores the same preference again",
            sms: None,
            software_token: None,
            email: Some((true, true)),
            settings: &[SMS_MFA, EMAIL_OTP_MFA],
            preferred: Some(EMAIL_OTP_MFA),
        },
        Step {
            case: "deactivating SMS MFA leaves the email preference, which the request did not \
                   name, alone",
            sms: Some((false, false)),
            software_token: None,
            email: None,
            settings: &[EMAIL_OTP_MFA],
            preferred: Some(EMAIL_OTP_MFA),
        },
        Step {
            case: "submitting the preferred factor as no longer preferred leaves it activated \
                   and the user preferring none",
            sms: None,
            software_token: None,
            email: Some((true, false)),
            settings: &[EMAIL_OTP_MFA],
            preferred: None,
        },
        Step {
            case: "activating a third factor as the preferred one moves the preference to it",
            sms: None,
            software_token: Some((true, true)),
            email: None,
            settings: &[EMAIL_OTP_MFA, SOFTWARE_TOKEN_MFA],
            preferred: Some(SOFTWARE_TOKEN_MFA),
        },
        Step {
            case: "deactivating the preferred factor takes the preference with it",
            sms: None,
            software_token: Some((false, false)),
            email: None,
            settings: &[EMAIL_OTP_MFA],
            preferred: None,
        },
    ];

    for step in &steps {
        let case = step.case;
        let row = Case {
            case,
            attributes: &[],
            software_token_registered: true,
            sms: step.sms,
            software_token: step.software_token,
            email: step.email,
            expected: Expect::Stored {
                settings: step.settings,
                preferred: step.preferred,
            },
        };
        let (status, payload) = dispatch(
            &instance.router,
            "SetUserMFAPreference",
            &preference_body(&user.access_token, &row),
            false,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the preference must be stored, got {status} with {payload}"
        );
        let expected = Preference {
            settings: step.settings.iter().map(|s| (*s).to_string()).collect(),
            preferred: step.preferred.map(str::to_string),
        };
        let stored = stored_preference(&instance.router, &user.access_token).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected {expected:?}, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn activated_sms_mfa_is_the_one_mfa_option_the_user_holds() {
    let instance = emulator();
    let user = fixture(&instance.router, "options-pool", VERIFIED_PHONE, None).await;

    // `MFAOptions` "provides information only about SMS MFA configurations"
    // (`GetUser` in this service's model), so the legacy view of the user follows
    // the one factor an MFA option configures.
    for (case, enabled, options) in [
        (
            "activating SMS MFA stores the SMS MFA option",
            true,
            vec![json!({ "DeliveryMedium": "SMS", "AttributeName": "phone_number" })],
        ),
        (
            "deactivating it leaves the user holding no MFA option",
            false,
            Vec::new(),
        ),
    ] {
        let (status, payload) = dispatch(
            &instance.router,
            "SetUserMFAPreference",
            &json!({
                "AccessToken": user.access_token,
                "SMSMfaSettings": { "Enabled": enabled, "PreferredMfa": enabled },
            }),
            false,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the preference must be stored, got {status} with {payload}"
        );
        let (status, read) = dispatch(
            &instance.router,
            "GetUser",
            &json!({ "AccessToken": user.access_token }),
            false,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetUser must succeed, got {status} with {read}"
        );
        let reported = read["MFAOptions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported, options,
            "case {case:?}: expected {options:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn deactivated_passkey_mfa_offers_the_user_no_passkey_mfa_factor() {
    let instance = emulator();
    let user = fixture(
        &instance.router,
        "passkey-pool",
        &[],
        Some(PASSKEY_MULTI_FACTOR),
    )
    .await;
    register_passkey(&instance, &user.user_pool_id, &user.username);

    // The pool enables passkey MFA and the user holds a passkey, so the factor is
    // available until the user deactivates it — and available again once the user
    // activates it.
    for (case, submitted, offered) in [
        ("a user that expressed no passkey preference", None, true),
        ("a user that deactivated passkey MFA", Some(false), false),
        ("a user that activated passkey MFA again", Some(true), true),
    ] {
        if let Some(enabled) = submitted {
            let (status, payload) = dispatch(
                &instance.router,
                "SetUserMFAPreference",
                &json!({
                    "AccessToken": user.access_token,
                    "WebAuthnMfaSettings": { "Enabled": enabled },
                }),
                false,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the preference must be stored, got {status} with {payload}"
            );
        }
        let factors = configured_factors(&instance.router, &user.access_token).await;
        assert_eq!(
            factors.iter().any(|factor| factor == WEB_AUTHN),
            offered,
            "case {case:?}: expected the passkey factor offered={offered}, got {factors:?}"
        );
    }
}

/// One row of the token table: a value submitted as the `AccessToken` that names
/// no signed-in user.
#[derive(Clone, Copy, Debug)]
struct TokenCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the row submits the sign-in's id token; otherwise it submits a
    /// value this emulator never issued at all.
    id_token: bool,
}

/// The rows differ along WHY the token names no signed-in user: it was never
/// issued at all, or it was issued as a token of another kind.
const TOKEN_CASES: &[TokenCase] = &[
    TokenCase {
        case: "a token this emulator never issued",
        id_token: false,
    },
    TokenCase {
        case: "the sign-in's id token, which is not the access token the member asks for",
        id_token: true,
    },
];

#[tokio::test]
async fn set_user_mfa_preference_refuses_a_token_that_names_no_signed_in_user() {
    let instance = emulator();
    let user = fixture(&instance.router, "token-pool", VERIFIED_PHONE, None).await;

    for row in TOKEN_CASES {
        let case = row.case;
        let token = if row.id_token {
            user.id_token.clone()
        } else {
            // Satisfies `TokenModelType`'s pattern, so the request reaches the
            // handler and the refusal is the handler's own.
            "not-a-token-this-emulator-issued".to_string()
        };
        let (status, payload) = dispatch(
            &instance.router,
            "SetUserMFAPreference",
            &json!({
                "AccessToken": token,
                "SMSMfaSettings": { "Enabled": true, "PreferredMfa": true },
            }),
            false,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the refusal is a 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("NotAuthorizedException"),
            "case {case:?}: expected NotAuthorizedException, got {code:?} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, "Invalid Access Token",
            "case {case:?}: expected the refusal to report \"Invalid Access Token\", got \
             {message:?}"
        );
        // The user the valid token names is untouched by a call no token
        // identified.
        let stored = stored_preference(&instance.router, &user.access_token).await;
        let expected = Preference {
            settings: Vec::new(),
            preferred: None,
        };
        assert_eq!(
            stored, expected,
            "case {case:?}: expected {expected:?}, got {stored:?}"
        );
    }
}
