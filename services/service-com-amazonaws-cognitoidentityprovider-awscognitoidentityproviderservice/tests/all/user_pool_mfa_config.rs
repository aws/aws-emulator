// SPDX-License-Identifier: CC0-1.0

//! `SetUserPoolMfaConfig` and `GetUserPoolMfaConfig` over the wire, driven through
//! the router exactly as an SDK drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `SetUserPoolMfaConfig` SETS the MFA and passkey
//! configuration of the pool it names — each factor's configuration is stored as
//! sent and cleared when the request leaves it out — and both this call's response
//! and a later `GetUserPoolMfaConfig` render the STORED configuration. The one
//! exception is `SmsMfaConfiguration`, whose two members are the pool's own SMS
//! settings (`SmsConfiguration` is settable "in `CreateUserPool` and
//! `UpdateUserPool`, or in `SetUserPoolMfaConfig`", and `SmsAuthenticationMessage`
//! is a member of the pool itself): the pool keeps ONE copy of each, every reader
//! renders the same stored value, and a pool-level setting this call says nothing
//! about keeps the value the pool held.
//!
//! Each table row names the case it asserts and every assertion reports the actual
//! value.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The Amazon SNS caller role an SMS MFA factor is delivered through.
const SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/sms-caller";

/// A second SNS caller role, so a case can change the stored one.
const OTHER_SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/other-sms-caller";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
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
    let (status, payload) = call(router, operation, body).await;
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

/// A pool created with `extra` merged into the create request, and its minted id.
async fn pool_with(router: &MultiServiceRouter, case: &str, extra: Value) -> String {
    let mut request = json!({ "PoolName": "mfa-config-pool" });
    if let (Some(target), Some(members)) = (request.as_object_mut(), extra.as_object()) {
        for (name, value) in members {
            target.insert(name.clone(), value.clone());
        }
    }
    let created = ok(router, "CreateUserPool", &request).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted pool id: {created}"))
        .to_string()
}

/// A pool with no SMS settings of its own.
async fn plain_pool(router: &MultiServiceRouter, case: &str) -> String {
    pool_with(router, case, json!({})).await
}

/// `SetUserPoolMfaConfig` against `user_pool_id` with `members`.
fn set_request(user_pool_id: &str, members: &Value) -> Value {
    let mut request = members.clone();
    request["UserPoolId"] = json!(user_pool_id);
    request
}

/// The configuration `GetUserPoolMfaConfig` reports for `user_pool_id`.
async fn read_back(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "GetUserPoolMfaConfig",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await
}

/// The pool `user_pool_id` names, as `DescribeUserPool` renders it.
async fn described_pool(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone()
}

/// The value at a dotted path inside `value`, or `None` when any step is absent.
fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for step in path.split('.') {
        current = current.get(step)?;
    }
    Some(current)
}

/// One row of the table: the members a `SetUserPoolMfaConfig` request carries
/// beyond `UserPoolId`, and the configuration the pool must report afterwards.
/// Rows differ along what the rule generalises over — which factors the request
/// configures, how many members each factor's structure carries, and the MFA mode.
struct MfaCase {
    case: &'static str,
    request: Value,
    expected: Value,
}

/// An SMS MFA configuration carrying every `SmsConfigurationType` member this
/// emulator stores, as the modelled structure names them.
fn full_sms_configuration() -> Value {
    json!({
        "SnsCallerArn": SNS_CALLER_ARN,
        "ExternalId": "mfa-external-id",
        "SnsRegion": "us-east-1"
    })
}

/// The table. Every row runs against a freshly created pool with no SMS settings
/// of its own, so each expectation is exactly what the request configured.
fn mfa_cases() -> Vec<MfaCase> {
    vec![
        MfaCase {
            case: "the pool id alone: MFA is OFF and no factor is configured",
            request: json!({}),
            expected: json!({ "MfaConfiguration": "OFF" }),
        },
        MfaCase {
            case: "TOTP enabled, MFA required of every user",
            request: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
            expected: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
        },
        MfaCase {
            // `UserPoolMfaType` admits OFF | ON | OPTIONAL (this service's model);
            // this row is the third value, with TOTP configured but switched off.
            // Turning MFA on at all takes one enabled factor, so the email factor
            // beside it is what this pool would challenge with.
            case: "TOTP configured but disabled beside email OTP, MFA optional",
            request: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false },
                "EmailMfaConfiguration": { "Message": "Your code is {####}" }
            }),
            expected: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false },
                "EmailMfaConfiguration": { "Message": "Your code is {####}" }
            }),
        },
        MfaCase {
            case: "email OTP with both template members",
            request: json!({
                "MfaConfiguration": "ON",
                "EmailMfaConfiguration": {
                    "Message": "Your code is {####}",
                    "Subject": "Your sign-in code"
                }
            }),
            expected: json!({
                "MfaConfiguration": "ON",
                "EmailMfaConfiguration": {
                    "Message": "Your code is {####}",
                    "Subject": "Your sign-in code"
                }
            }),
        },
        MfaCase {
            case: "email OTP with the subject only: a partly filled structure is stored as sent",
            request: json!({
                "MfaConfiguration": "OPTIONAL",
                "EmailMfaConfiguration": { "Subject": "Subject only" }
            }),
            expected: json!({
                "MfaConfiguration": "OPTIONAL",
                "EmailMfaConfiguration": { "Subject": "Subject only" }
            }),
        },
        MfaCase {
            case: "SMS MFA with an SNS caller, an external id, a region and a message template",
            request: json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": {
                    "SmsAuthenticationMessage": "Your code is {####}",
                    "SmsConfiguration": full_sms_configuration()
                }
            }),
            expected: json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": {
                    "SmsAuthenticationMessage": "Your code is {####}",
                    "SmsConfiguration": full_sms_configuration()
                }
            }),
        },
        MfaCase {
            case: "SMS MFA with the SNS caller only, and no message template",
            request: json!({
                "MfaConfiguration": "OPTIONAL",
                "SmsMfaConfiguration": {
                    "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN }
                }
            }),
            expected: json!({
                "MfaConfiguration": "OPTIONAL",
                "SmsMfaConfiguration": {
                    "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN }
                }
            }),
        },
        MfaCase {
            // `UserVerificationType` admits preferred | required and
            // `WebAuthnFactorConfigurationType` admits
            // MULTI_FACTOR_WITH_USER_VERIFICATION | SINGLE_FACTOR (this service's
            // model); this row takes one value of each and the next row the other.
            // Passkeys are not one of the three factors an MFA challenge is
            // presented over, so a pool that requires MFA enables one of those
            // three beside them (`SetUserPoolMfaConfig`'s own refusal names them).
            case: "passkeys with required user verification, beside TOTP, satisfying MFA",
            request: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true },
                "WebAuthnConfiguration": {
                    "RelyingPartyId": "auth.example.com",
                    "UserVerification": "required",
                    "FactorConfiguration": "MULTI_FACTOR_WITH_USER_VERIFICATION"
                }
            }),
            expected: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true },
                "WebAuthnConfiguration": {
                    "RelyingPartyId": "auth.example.com",
                    "UserVerification": "required",
                    "FactorConfiguration": "MULTI_FACTOR_WITH_USER_VERIFICATION"
                }
            }),
        },
        MfaCase {
            case: "passkeys as a single factor with preferred user verification",
            request: json!({
                "MfaConfiguration": "OFF",
                "WebAuthnConfiguration": {
                    "RelyingPartyId": "localhost",
                    "UserVerification": "preferred",
                    "FactorConfiguration": "SINGLE_FACTOR"
                }
            }),
            expected: json!({
                "MfaConfiguration": "OFF",
                "WebAuthnConfiguration": {
                    "RelyingPartyId": "localhost",
                    "UserVerification": "preferred",
                    "FactorConfiguration": "SINGLE_FACTOR"
                }
            }),
        },
        MfaCase {
            case: "every factor at once",
            request: json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": {
                    "SmsAuthenticationMessage": "SMS code {####}",
                    "SmsConfiguration": full_sms_configuration()
                },
                "SoftwareTokenMfaConfiguration": { "Enabled": true },
                "EmailMfaConfiguration": {
                    "Message": "Email code {####}",
                    "Subject": "Email code"
                },
                "WebAuthnConfiguration": {
                    "RelyingPartyId": "auth.example.com",
                    "UserVerification": "required",
                    "FactorConfiguration": "MULTI_FACTOR_WITH_USER_VERIFICATION"
                }
            }),
            expected: json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": {
                    "SmsAuthenticationMessage": "SMS code {####}",
                    "SmsConfiguration": full_sms_configuration()
                },
                "SoftwareTokenMfaConfiguration": { "Enabled": true },
                "EmailMfaConfiguration": {
                    "Message": "Email code {####}",
                    "Subject": "Email code"
                },
                "WebAuthnConfiguration": {
                    "RelyingPartyId": "auth.example.com",
                    "UserVerification": "required",
                    "FactorConfiguration": "MULTI_FACTOR_WITH_USER_VERIFICATION"
                }
            }),
        },
    ]
}

#[tokio::test]
async fn a_set_stores_the_configuration_and_answers_what_was_stored() {
    let router = router();
    for case in mfa_cases() {
        let label = case.case;
        let user_pool_id = plain_pool(&router, label).await;
        let request = set_request(&user_pool_id, &case.request);

        let answered = ok(&router, "SetUserPoolMfaConfig", &request).await;
        assert_eq!(
            answered, case.expected,
            "case {label:?}: SetUserPoolMfaConfig must answer the stored configuration, got \
             {answered}"
        );

        // The configuration is PERSISTED: a later read renders the same thing.
        let read = read_back(&router, &user_pool_id).await;
        assert_eq!(
            read, case.expected,
            "case {label:?}: GetUserPoolMfaConfig must read back the stored configuration, got \
             {read}"
        );

        // The pool's own view agrees about the MFA mode, which is one stored value.
        let described = described_pool(&router, &user_pool_id).await;
        assert_eq!(
            described.get("MfaConfiguration"),
            case.expected.get("MfaConfiguration"),
            "case {label:?}: DescribeUserPool must render the same MFA mode, got {described}"
        );

        // Setting the same configuration again is idempotent.
        let again = ok(&router, "SetUserPoolMfaConfig", &request).await;
        assert_eq!(
            again, case.expected,
            "case {label:?}: the same set applied twice must store the same configuration, got \
             {again}"
        );
    }
}

#[tokio::test]
async fn a_set_replaces_the_factors_the_request_leaves_out() {
    let router = router();
    let case = "every factor, then a set that names none";
    let user_pool_id = plain_pool(&router, case).await;

    let everything = json!({
        "MfaConfiguration": "ON",
        "SmsMfaConfiguration": {
            "SmsAuthenticationMessage": "SMS code {####}",
            "SmsConfiguration": full_sms_configuration()
        },
        "SoftwareTokenMfaConfiguration": { "Enabled": true },
        "EmailMfaConfiguration": { "Message": "Email code {####}", "Subject": "Email code" },
        "WebAuthnConfiguration": {
            "RelyingPartyId": "auth.example.com",
            "UserVerification": "required",
            "FactorConfiguration": "MULTI_FACTOR_WITH_USER_VERIFICATION"
        }
    });
    ok(
        &router,
        "SetUserPoolMfaConfig",
        &set_request(&user_pool_id, &everything),
    )
    .await;

    // A set that carries only the MFA mode clears every factor whose
    // configuration belongs to the MFA configuration alone. The mode it carries is
    // `OFF`, because a pool that requires or offers MFA needs a factor to challenge
    // with and this request enables none.
    let narrowed = ok(
        &router,
        "SetUserPoolMfaConfig",
        &set_request(&user_pool_id, &json!({ "MfaConfiguration": "OFF" })),
    )
    .await;
    for factor in [
        "SoftwareTokenMfaConfiguration",
        "EmailMfaConfiguration",
        "WebAuthnConfiguration",
    ] {
        assert!(
            narrowed.get(factor).is_none_or(Value::is_null),
            "case {case:?}: {factor} must be cleared by a set that leaves it out, got {narrowed}"
        );
    }
    assert_eq!(
        narrowed.get("MfaConfiguration"),
        Some(&json!("OFF")),
        "case {case:?}: got {narrowed}"
    );

    // The pool's own SMS settings are NOT MFA-only, so they survive: one copy,
    // which `DescribeUserPool` renders too.
    assert_eq!(
        at(&narrowed, "SmsMfaConfiguration.SmsConfiguration.SnsCallerArn"),
        Some(&json!(SNS_CALLER_ARN)),
        "case {case:?}: the pool's SMS settings must survive an MFA set that names no SMS \
         factor, got {narrowed}"
    );
    let described = described_pool(&router, &user_pool_id).await;
    assert_eq!(
        at(&described, "SmsConfiguration.SnsCallerArn"),
        Some(&json!(SNS_CALLER_ARN)),
        "case {case:?}: DescribeUserPool must render the same stored SNS caller, got {described}"
    );

    // And the read agrees with the answer the set gave.
    let read = read_back(&router, &user_pool_id).await;
    assert_eq!(
        read, narrowed,
        "case {case:?}: GetUserPoolMfaConfig must agree with the set that stored it, got {read}"
    );

    // A pool-level SMS member the request carries replaces the stored one, and the
    // one it does not carry keeps the value the pool held.
    let replaced = ok(
        &router,
        "SetUserPoolMfaConfig",
        &set_request(
            &user_pool_id,
            &json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": {
                    "SmsConfiguration": { "SnsCallerArn": OTHER_SNS_CALLER_ARN }
                }
            }),
        ),
    )
    .await;
    assert_eq!(
        at(&replaced, "SmsMfaConfiguration"),
        Some(&json!({
            "SmsAuthenticationMessage": "SMS code {####}",
            "SmsConfiguration": { "SnsCallerArn": OTHER_SNS_CALLER_ARN }
        })),
        "case {case:?}: a carried SMS member replaces the stored one and an absent one \
         survives, got {replaced}"
    );
}

#[tokio::test]
async fn the_sms_settings_a_pool_already_holds_are_the_ones_reported() {
    let router = router();
    let case = "SMS settings created with the pool";
    // The pool is created with the SMS settings `CreateUserPool` models, which are
    // the SAME stored values `SmsMfaConfiguration` is a view over.
    let user_pool_id = pool_with(
        &router,
        case,
        json!({
            "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN, "ExternalId": "created" },
            "SmsAuthenticationMessage": "Created code {####}"
        }),
    )
    .await;

    let read = read_back(&router, &user_pool_id).await;
    assert_eq!(
        at(&read, "SmsMfaConfiguration"),
        Some(&json!({
            "SmsAuthenticationMessage": "Created code {####}",
            "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN, "ExternalId": "created" }
        })),
        "case {case:?}: the SMS MFA view must render the pool's stored SMS settings, got {read}"
    );

    // An update of those pool members moves the same stored value, so the MFA
    // read follows it instead of reporting a second copy.
    ok(
        &router,
        "UpdateUserPool",
        &json!({
            "UserPoolId": user_pool_id,
            "SmsConfiguration": { "SnsCallerArn": OTHER_SNS_CALLER_ARN },
            "SmsAuthenticationMessage": "Updated code {####}"
        }),
    )
    .await;
    let after_update = read_back(&router, &user_pool_id).await;
    assert_eq!(
        at(&after_update, "SmsMfaConfiguration"),
        Some(&json!({
            "SmsAuthenticationMessage": "Updated code {####}",
            "SmsConfiguration": { "SnsCallerArn": OTHER_SNS_CALLER_ARN }
        })),
        "case {case:?}: the MFA read must follow the pool's stored SMS settings, got \
         {after_update}"
    );

    // A pool with no SMS settings at all reports no SMS MFA configuration.
    let bare = plain_pool(&router, case).await;
    let bare_read = read_back(&router, &bare).await;
    assert!(
        bare_read
            .get("SmsMfaConfiguration")
            .is_none_or(Value::is_null),
        "case {case:?}: a pool with no SMS settings reports no SMS MFA configuration, got \
         {bare_read}"
    );
}

#[tokio::test]
async fn an_sms_factor_without_an_sns_caller_is_refused_while_mfa_is_on() {
    let router = router();
    // Rows differ along the MFA mode: the two modes that offer MFA refuse an SMS
    // factor nothing could deliver, and the mode that offers none accepts it.
    let cases = [
        ("MFA required", "ON", 400),
        ("MFA optional", "OPTIONAL", 400),
        ("MFA off", "OFF", 200),
    ];
    for (label, mode, expected_status) in cases {
        let user_pool_id = plain_pool(&router, label).await;
        let (status, payload) = call(
            &router,
            "SetUserPoolMfaConfig",
            &set_request(
                &user_pool_id,
                &json!({
                    "MfaConfiguration": mode,
                    "SmsMfaConfiguration": { "SmsAuthenticationMessage": "Code {####}" }
                }),
            ),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: got {status} with {payload}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&payload),
                "InvalidParameterException",
                "case {label:?}: got {payload}"
            );
            assert_eq!(
                payload.get("message").or_else(|| payload.get("Message")),
                Some(&json!(
                    "Invalid SMS MFA configuration given, external id and a valid caller arn are required."
                )),
                "case {label:?}: got {payload}"
            );
            // The refusal stored nothing.
            let read = read_back(&router, &user_pool_id).await;
            assert_eq!(
                read,
                json!({ "MfaConfiguration": "OFF" }),
                "case {label:?}: a refused set must change nothing, got {read}"
            );
        } else {
            let read = read_back(&router, &user_pool_id).await;
            assert_eq!(
                at(&read, "SmsMfaConfiguration.SmsAuthenticationMessage"),
                Some(&json!("Code {####}")),
                "case {label:?}: got {read}"
            );
        }
    }

    // And a pool-level SMS setting this call says nothing about survives it: the
    // SNS caller a create stored is still the one reported after an MFA call that
    // carries only the message template.
    let case = "an MFA call silent about the SNS caller";
    let user_pool_id = pool_with(
        &router,
        case,
        json!({ "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN } }),
    )
    .await;
    let answered = ok(
        &router,
        "SetUserPoolMfaConfig",
        &set_request(
            &user_pool_id,
            &json!({
                "MfaConfiguration": "OFF",
                "SmsMfaConfiguration": { "SmsAuthenticationMessage": "Code {####}" }
            }),
        ),
    )
    .await;
    assert_eq!(
        at(&answered, "SmsMfaConfiguration"),
        Some(&json!({
            "SmsAuthenticationMessage": "Code {####}",
            "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN }
        })),
        "case {case:?}: the stored SNS caller must survive, got {answered}"
    );
    let read = read_back(&router, &user_pool_id).await;
    assert_eq!(
        read, answered,
        "case {case:?}: the read must agree with the set, got {read}"
    );
}

#[tokio::test]
async fn an_absent_pool_is_a_400_for_both_operations() {
    let router = router();
    // Ids no pool was created under, each satisfying the modelled
    // `UserPoolIdType` pattern `^[\w-]+_[0-9a-zA-Z]+$` in a different form.
    let absent_ids = [
        ("another region", "us-west-2_39e6f417e"),
        ("an upper-case suffix", "eu-central-1_ABCDEFGHI"),
        ("a single-character suffix", "us-east-1_0"),
        ("an underscore-only prefix", "pool_abc123"),
    ];
    // A live pool, so the refusal is about the id and not about an empty store.
    let live = plain_pool(&router, "live pool").await;

    for (label, user_pool_id) in absent_ids {
        for (operation, body) in [
            (
                "SetUserPoolMfaConfig",
                json!({ "UserPoolId": user_pool_id, "MfaConfiguration": "ON",
                        "SoftwareTokenMfaConfiguration": { "Enabled": true } }),
            ),
            (
                "GetUserPoolMfaConfig",
                json!({ "UserPoolId": user_pool_id }),
            ),
            // An absent pool is a 400 whatever else the body carries: the lookup
            // is answered before any question about the factors it configures,
            // so a body that would be refused on a live pool is still a 400 here.
            (
                "SetUserPoolMfaConfig",
                json!({ "UserPoolId": user_pool_id, "MfaConfiguration": "ON",
                        "SmsMfaConfiguration": { "SmsAuthenticationMessage": "Code {####}" } }),
            ),
        ] {
            let (status, payload) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {label:?}: {operation} on an absent pool must answer 400, got {status} \
                 with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {label:?}: {operation} got {payload}"
            );
        }
    }

    // The refused calls changed nothing about the pool that does exist.
    let read = read_back(&router, &live).await;
    assert_eq!(
        read,
        json!({ "MfaConfiguration": "OFF" }),
        "case \"live pool untouched\": got {read}"
    );
}

#[tokio::test]
async fn a_set_advances_the_pools_last_modified_instant_and_a_read_does_not() {
    let router = router();
    let case = "last modified";
    let user_pool_id = plain_pool(&router, case).await;
    let before = last_modified(case, &described_pool(&router, &user_pool_id).await);

    tokio::time::sleep(Duration::from_millis(5)).await;
    ok(
        &router,
        "SetUserPoolMfaConfig",
        &set_request(
            &user_pool_id,
            &json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
        ),
    )
    .await;
    let after_set = last_modified(case, &described_pool(&router, &user_pool_id).await);
    assert!(
        after_set > before,
        "case {case:?}: a set must advance LastModifiedDate past {before}, got {after_set}"
    );

    // Reading the configuration twice answers the same thing and moves nothing:
    // a read must not change what a later caller sees.
    let first = read_back(&router, &user_pool_id).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let second = read_back(&router, &user_pool_id).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads must answer the same configuration, got {second}"
    );
    let after_reads = last_modified(case, &described_pool(&router, &user_pool_id).await);
    assert_eq!(
        after_reads, after_set,
        "case {case:?}: GetUserPoolMfaConfig must not move LastModifiedDate, got {after_reads}"
    );
}

/// `pool`'s `LastModifiedDate`, as the seconds-since-epoch number the modelled
/// timestamp serialises to.
fn last_modified(case: &str, pool: &Value) -> f64 {
    pool["LastModifiedDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a pool must carry a LastModifiedDate: {pool}"))
}
