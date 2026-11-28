// SPDX-License-Identifier: CC0-1.0

//! How an MFA activation the user cannot receive is WORDED, over the wire, driven exactly
//! as an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.AdminSetUserMFAPreference`).
//!
//! The rule under test: **activating an MFA factor needs the delivery configuration that
//! factor reaches the user through, and a request asking to activate one for which no such
//! configuration is in place — in a pool that turns the factor on for nobody — is refused
//! `InvalidParameterException` (400) with "User does not have delivery config set to turn on
//! <the MFA type the request named>" — one sentence for every factor, whose only variable
//! part is the `UserMFASettingList` setting taken from the request.** Turning a factor OFF
//! needs no configuration, so it is served.
//!
//! Each row's pool stores no `SoftwareTokenMfaConfiguration`, which is what makes this the
//! sentence for the software token too: a pool that DOES turn TOTP MFA on has that
//! configuration in place and reports the user's own missing verification instead, which
//! `mfa_preference_software_token_precondition.rs` is the table for.
//!
//! The rows span what the rule generalises over: every `UserMFASettingList` value — "the
//! possible values in this list are `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`"
//! (`smithy_query(query="shape",shape="UserMFASettingListType")`) — including the software
//! token, whose configuration is a registered authenticator rather than a message medium,
//! and whether the request also marks the factor as the preferred one. The self-service
//! twin of this write states the same sentence, over the same three settings, in
//! `set_user_mfa_preference.rs`.
//!
//! The pools and users are real: they come from `CreateUserPool` and `AdminCreateUser`, and
//! what a refused request left behind is read back through `AdminGetUser`.
//!
//! Each table row names the case it asserts and every assertion reports the actual value.

use serde_json::{json, Map, Value};

use super::log_delivery_configuration::{call, error_code, error_message, ok, pool, router};

/// The error code a request asking for an activation the user cannot receive carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The user every row sets a preference for: created with no verified contact attribute and
/// no registered authenticator, so no factor has a delivery configuration.
const USERNAME: &str = "mfa-preference-user";

/// The sentence an activation the user has no delivery configuration for is refused with.
fn no_delivery_config(setting: &str) -> String {
    format!("User does not have delivery config set to turn on {setting}")
}

/// The request this write takes: the settings structure `member`, asking for `enabled` and
/// for the factor to be `preferred`, submitted for the one fixture user.
fn preference(user_pool_id: &str, member: &str, enabled: bool, preferred: bool) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Username".to_string(), json!(USERNAME));
    body.insert(
        member.to_string(),
        json!({ "Enabled": enabled, "PreferredMfa": preferred }),
    );
    Value::Object(body)
}

/// One row: the settings structure the request submits, the `UserMFASettingList` value it
/// names, and whether it also asks for that factor to be preferred.
struct Row {
    case: &'static str,
    /// The input member the factor is submitted on.
    member: &'static str,
    /// The `UserMFASettingList` value the factor is reported under, which the refusal names.
    setting: &'static str,
    preferred: bool,
}

#[tokio::test]
async fn an_activation_without_a_delivery_configuration_names_the_factor_it_asked_for() {
    let router = router();
    let user_pool_id = pool(&router, "a pool whose user receives no factor", None).await;
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "MessageAction": "SUPPRESS",
        }),
    )
    .await;

    let rows = [
        Row {
            case: "SMS message MFA for a user with no verified phone number",
            member: "SMSMfaSettings",
            setting: "SMS_MFA",
            preferred: false,
        },
        Row {
            case: "email message MFA for a user with no verified email address",
            member: "EmailMfaSettings",
            setting: "EMAIL_OTP",
            preferred: false,
        },
        Row {
            case: "software token MFA for a user with no registered authenticator",
            member: "SoftwareTokenMfaSettings",
            setting: "SOFTWARE_TOKEN_MFA",
            preferred: false,
        },
        Row {
            case: "software token MFA asked for as the preferred factor as well",
            member: "SoftwareTokenMfaSettings",
            setting: "SOFTWARE_TOKEN_MFA",
            preferred: true,
        },
        Row {
            case: "SMS message MFA asked for as the preferred factor as well",
            member: "SMSMfaSettings",
            setting: "SMS_MFA",
            preferred: true,
        },
    ];

    for row in rows {
        let body = preference(&user_pool_id, row.member, true, row.preferred);
        let (status, payload) = call(&router, "AdminSetUserMFAPreference", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, INVALID_PARAMETER),
            "case {:?}: expected a 400 {INVALID_PARAMETER}, got {status} with {payload}",
            row.case
        );
        let message = error_message(&payload);
        let expected = no_delivery_config(row.setting);
        assert_eq!(
            message, expected,
            "case {:?}: expected {expected:?}, got {message:?}",
            row.case
        );
        // A refused request writes nothing, so the user still has no activated factor.
        let read = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
        )
        .await;
        assert!(
            read["UserMFASettingList"]
                .as_array()
                .is_none_or(|activated| activated.is_empty()),
            "case {:?}: a refused request activates nothing, got {read}",
            row.case
        );
    }
}

#[tokio::test]
async fn turning_a_factor_off_needs_no_delivery_configuration() {
    let router = router();
    let user_pool_id = pool(&router, "a pool whose user turns a factor off", None).await;
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "MessageAction": "SUPPRESS",
        }),
    )
    .await;

    // The refusal above answers an ACTIVATION and not every write: the same user, asked to
    // turn each factor off, is served, because a factor that is off reaches nobody.
    for member in [
        "SMSMfaSettings",
        "EmailMfaSettings",
        "SoftwareTokenMfaSettings",
    ] {
        let body = preference(&user_pool_id, member, false, false);
        let (status, payload) = call(&router, "AdminSetUserMFAPreference", &body).await;
        assert_eq!(
            status, 200,
            "case {member:?}: turning a factor off is served, got {status} with {payload}"
        );
    }
    let read: Value = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": USERNAME }),
    )
    .await;
    assert!(
        read["UserMFASettingList"]
            .as_array()
            .is_none_or(|activated| activated.is_empty()),
        "a user that turned every factor off has none activated, got {read}"
    );
}
