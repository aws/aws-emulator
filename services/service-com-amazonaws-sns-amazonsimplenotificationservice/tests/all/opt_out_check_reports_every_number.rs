// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `CheckIfPhoneNumberIsOptedOut` answers for a number whose
//! FORM is not the one the sandbox and publish members are constrained to.
//!
//! The RULE asserted here: the opt-out check REPORTS this account's opt-out record for
//! whatever number it is handed — its `phoneNumber` member targets an unconstrained
//! string (`smithy_query(query="shape", shape="PhoneNumber")` reports no `@length` and
//! no `@pattern`, where the `PhoneNumberString` of the sandbox and publish operations
//! carries both), so no value is refused for its form, and a number the account holds
//! no opt-out record for is reported as opted IN: `isOptedOut` is the modelled boolean
//! whose default is `false`, documented "false – The phone number is opted in". A
//! number the record says opted out reports `true` whatever form it takes, and the
//! read changes nothing.
//!
//! The rows differ along what the rule generalises over: the FORM of the number
//! (national forms with separators, a bare short code, a value that is no number at
//! all, both published E.164 digit bounds and one past each, a country code beginning
//! with zero) and the opt-out state the record holds. The forms come from the opt-in
//! suite's own catalogs, so the two suites cannot drift on which value is which.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use std::time::SystemTime;

use super::phone_number_opt_in::{e164_numbers, not_e164_values, seed_opted_out_at};
use super::topics_subscriptions_publish::{Answer, Sns};

/// The member both opt-out operations spell their phone number with on the wire.
const PHONE_NUMBER_PARAM: &str = "phoneNumber";

/// One `CheckIfPhoneNumberIsOptedOut` call.
async fn check(sns: &Sns, number: &str) -> Answer {
    sns.call(
        "CheckIfPhoneNumberIsOptedOut",
        &[(PHONE_NUMBER_PARAM, number)],
    )
    .await
}

/// The `isOptedOut` member of an answered check, asserting the call was served.
fn served(case: &str, answer: &Answer) -> bool {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the check to answer 200, got {} / {}",
        answer.status, answer.body
    );
    let reported = answer
        .element("isOptedOut")
        .or_else(|| answer.element("IsOptedOut"))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an isOptedOut member, got {}",
                answer.body
            )
        });
    assert!(
        reported == "true" || reported == "false",
        "case {case:?}: expected the modelled boolean, got {reported:?}"
    );
    reported == "true"
}

/// Every number FORM — the ones the constrained member's pattern admits and the ones it
/// does not — is answered by the check, and a number with no opt-out record reports
/// `false`.
#[tokio::test]
async fn a_number_with_no_opt_out_record_is_reported_opted_in_whatever_its_form() {
    let sns = Sns::new();

    let mut rows: Vec<(String, String)> = Vec::new();
    for (case, number) in e164_numbers() {
        rows.push((format!("inside the constrained pattern: {case}"), number));
    }
    for (case, value) in not_e164_values() {
        rows.push((format!("outside the constrained pattern: {case}"), value));
    }
    // A bare short code, which the constrained `PhoneNumberString` pattern's second
    // branch admits and this operation's unconstrained member says nothing about.
    rows.push(("a bare short code".to_string(), "12345".to_string()));

    for (case, value) in &rows {
        let answer = check(&sns, value).await;
        assert!(
            !served(case, &answer),
            "case {case:?}: a number with no opt-out record must report false, got {}",
            answer.body
        );
        // A read is a report: the second check answers the same, and neither wrote a
        // record for the number it was asked about.
        let again = check(&sns, value).await;
        assert!(
            !served(case, &again),
            "case {case:?}: a second check must report the same, got {}",
            again.body
        );
    }

    assert!(
        sns.state().lock().phone_numbers.is_empty(),
        "the checks must write no opt-out record, got {:?}",
        sns.state().lock().phone_numbers
    );
}

/// The state the check reports is the one the record HOLDS, whatever form the number
/// takes: the same values that report `false` with no record report `true` once this
/// account's out-of-band opt-out has written one for them.
#[tokio::test]
async fn an_opted_out_number_is_reported_opted_out_whatever_its_form() {
    let sns = Sns::new();
    let mut rows: Vec<(String, String)> = Vec::new();
    for (case, number) in e164_numbers() {
        rows.push((format!("inside the constrained pattern: {case}"), number));
    }
    for (case, value) in not_e164_values() {
        // The empty value names no number for a record to be keyed by.
        if value.is_empty() {
            continue;
        }
        rows.push((format!("outside the constrained pattern: {case}"), value));
    }

    for (case, value) in &rows {
        seed_opted_out_at(&sns, value, SystemTime::now(), None);
        let answer = check(&sns, value).await;
        assert!(
            served(case, &answer),
            "case {case:?}: the seeded opt-out must be reported, got {}",
            answer.body
        );
    }

    assert_eq!(
        sns.state().lock().phone_numbers.len(),
        rows.len(),
        "the checks must leave exactly the seeded records, got {:?}",
        sns.state().lock().phone_numbers
    );
}
