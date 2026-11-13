// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the DESTINATION NUMBER the SMS sandbox operations are held to.
//!
//! The rule under test: every SNS operation whose input member names a destination phone
//! number takes only a number in E.164 format — a `+`, a country code that does not begin
//! with zero, and digits — and refuses anything else with the modelled
//! `InvalidParameterException` (awsQuery `InvalidParameter`, HTTP 400) before it reads any
//! state. The sandbox trio names one: "The destination phone number to verify … that you
//! can send SMS messages to"
//! (`smithy_query(query="doc",shape="CreateSMSSandboxPhoneNumberInput",member="PhoneNumber")`),
//! and the modelled pattern of the shape they share
//! (`smithy_query(query="shape",shape="PhoneNumberString")`:
//! `^(\+[0-9]{8,}|[0-9]{0,9})$`) also admits a bare national or short-code form, which is
//! not a destination number, so the operation refuses it although the request passed the
//! boundary validator.
//!
//! The rows differ along what the rule generalises over: the operation the member is carried
//! by, and the way the number fails to be one (no country code at all, at the bare branch's
//! length bound, empty, a country code E.164 assigns to no country). The accepted forms of
//! the sibling catalog are driven through the same calls, so the refusal is of the form and
//! not of the call.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::invalid_phone_number_message;

use super::sms_sandbox_phone_numbers::{
    accepted_number_forms, given_an_origination_number, refused_number_forms,
};
use super::topics_subscriptions_publish::Sns;

/// The one-time password a verification call carries: five digits, the shortest the modelled
/// `OTPCode` takes (`smithy_query(query="operation",operation="VerifySMSSandboxPhoneNumber")`
/// reports `length {min: 5, max: 8}`, pattern `^[0-9]+$`), so the request passes the boundary
/// validator and the number is what is judged.
const SOME_OTP: &str = "12345";

/// The operations that carry a destination phone number, and the parameters each needs
/// beside it.
fn operations() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("CreateSMSSandboxPhoneNumber", vec![]),
        (
            "VerifySMSSandboxPhoneNumber",
            vec![("OneTimePassword", SOME_OTP)],
        ),
        ("DeleteSMSSandboxPhoneNumber", vec![]),
    ]
}

#[tokio::test]
async fn a_destination_number_that_is_not_e164_is_refused_by_every_sandbox_operation() {
    let sns = Sns::new();

    for (action, extra) in operations() {
        for (case, number) in refused_number_forms() {
            let mut params: Vec<(&str, &str)> = vec![("PhoneNumber", number.as_str())];
            params.extend(extra.iter().copied());
            let answer = sns.call(action, &params).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {action}: expected HTTP 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?} on {action}: expected the InvalidParameter code, got {}",
                answer.body
            );
            let stated = answer.element("Message");
            assert_eq!(
                stated.as_deref(),
                Some(invalid_phone_number_message().as_str()),
                "case {case:?} on {action}: expected the parameter refusal sentence, got \
                 {stated:?}"
            );
            assert!(
                !answer.body.contains(number.as_str()) || number.is_empty(),
                "case {case:?} on {action}: the number is a sensitive member, so the refusal \
                 must not echo it, got {}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn an_e164_destination_number_reaches_the_sandbox_through_every_operation() {
    let sns = Sns::new();

    for (case, number) in accepted_number_forms() {
        // The registration SENDS a one-time password, so the account needs something to send
        // it from before the number itself can be what the call is judged on.
        given_an_origination_number(&sns);
        // The add is served, so the number is one the sandbox holds …
        let added = sns
            .call("CreateSMSSandboxPhoneNumber", &[("PhoneNumber", &number)])
            .await;
        assert_eq!(
            added.status, 200,
            "case {case:?}: CreateSMSSandboxPhoneNumber expected HTTP 200, got {} / {}",
            added.status, added.body
        );
        // … so a verification of it reaches the stored record and is judged on the code it
        // carries (the wrong one here), never on the form of the number …
        let verified = sns
            .call(
                "VerifySMSSandboxPhoneNumber",
                &[("PhoneNumber", &number), ("OneTimePassword", SOME_OTP)],
            )
            .await;
        assert_eq!(
            verified.code(),
            "VerificationException",
            "case {case:?}: a held number with the wrong code must be refused on the code, \
             got {} / {}",
            verified.status,
            verified.body
        );
        // … and the delete removes it.
        let deleted = sns
            .call("DeleteSMSSandboxPhoneNumber", &[("PhoneNumber", &number)])
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteSMSSandboxPhoneNumber expected HTTP 200, got {} / {}",
            deleted.status, deleted.body
        );
    }
}
