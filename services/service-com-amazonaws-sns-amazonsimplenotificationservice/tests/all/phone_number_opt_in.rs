// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SMS opt-out slice: `CheckIfPhoneNumberIsOptedOut` reports
//! whether a phone number has opted out of receiving SMS messages from this account,
//! and `OptInPhoneNumber` opts an opted-out number back in.
//!
//! The rules under test:
//!
//! 1. a phone-number parameter the model leaves an unconstrained string has to be a
//!    phone number in E.164 format — a `+`, a country code that does not begin with
//!    `0`, then digits only, between the published minimum and maximum digit counts —
//!    and a value that is not one is refused as bad input (400 `InvalidParameter`),
//!    before any state is read or written, by the operation that ACTS on the number
//!    (the opt-in). The opt-out CHECK only reports, so it answers for whatever number
//!    it is handed — `opt_out_check_reports_every_number.rs` is that rule;
//! 2. the opt-out state a check reports is the one the number's record HOLDS, and an
//!    opt-in is what clears it: a number with no record, and a number opted back in,
//!    both report `false`;
//! 3. an opt-in is available only once per the documented interval, so a number whose
//!    previous opt-in is younger than that stays opted out and the call is refused.
//!
//! The rows differ along what those rules generalise over: the phone-number FORM and
//! LENGTH (both published digit bounds and one past each), the opt-out state the
//! record holds, and the AGE of the previous opt-in. Every call goes over the wire
//! through the router the emulator serves in production, so nothing but persisted
//! state carries information between calls.
//!
//! A number opts OUT out of band (the recipient replies `STOP`), which no SNS API
//! performs, so the suite seeds that record the way the out-of-band event writes it —
//! the same seam the sandbox suite uses to put a record in a state no API mints.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    invalid_phone_number_message, opt_in_too_soon_message, PhoneNumberRecord, MAX_E164_DIGITS,
    MIN_E164_DIGITS, OPT_IN_INTERVAL,
};

use super::topics_subscriptions_publish::{Answer, Sns};

/// The name both opt-out operations spell their phone-number member with on the wire:
/// `smithy_query(query="operation", operation="CheckIfPhoneNumberIsOptedOut,
/// OptInPhoneNumber")` reports the member `phoneNumber` on both, and an awsQuery
/// member's wire name IS its member name. (The sandbox operations spell theirs
/// `PhoneNumber`, and SNS's refusal states that spelling whichever operation refused.)
const PHONE_NUMBER_PARAM: &str = "phoneNumber";

/// Phone numbers in E.164 format, spanning the forms the rule accepts: both published
/// digit bounds ([`MIN_E164_DIGITS`], [`MAX_E164_DIGITS`]) and numbers in between,
/// across country codes of one, two and three digits.
///
/// `pub(crate)` so the sibling opt-out LISTING suite drives the same accepted forms
/// rather than keeping a second catalog of them.
pub(crate) fn e164_numbers() -> Vec<(&'static str, String)> {
    vec![
        ("north american", "+12065550100".to_string()),
        ("united kingdom", "+447700900123".to_string()),
        ("three digit country code", "+3519100000000".to_string()),
        (
            "at the published digit minimum",
            format!("+{}", "9".repeat(MIN_E164_DIGITS)),
        ),
        (
            "at the published digit maximum",
            format!("+{}", "1".repeat(MAX_E164_DIGITS)),
        ),
    ]
}

/// Values that are NOT a phone number in E.164 format: one row per clause of the rule,
/// including one digit below the published minimum and one past the published maximum.
///
/// `pub(crate)` so the sibling suite that drives the opt-out CHECK over the same values
/// — which reports for every one of them rather than refusing it — reads them from this
/// one catalog.
pub(crate) fn not_e164_values() -> Vec<(&'static str, String)> {
    vec![
        ("no plus prefix", "12065550100".to_string()),
        ("national form with separators", "+1-206-555-0100".to_string()),
        ("spaces between groups", "+1 206 555 0100".to_string()),
        ("not a number at all", "not-a-phone-number".to_string()),
        ("parenthesised national form", "(206) 555-0100".to_string()),
        ("empty", String::new()),
        ("the plus alone", "+".to_string()),
        ("trailing extension text", "+12065550100x42".to_string()),
        ("country code beginning with zero", "+0123456789".to_string()),
        (
            "one digit below the published minimum",
            format!("+{}", "9".repeat(MIN_E164_DIGITS - 1)),
        ),
        (
            "one digit past the published maximum",
            format!("+{}", "1".repeat(MAX_E164_DIGITS + 1)),
        ),
    ]
}

/// The `isOptedOut` member the check reports, or `None` when the response carries no
/// such member at all.
fn reported_opt_out(answer: &Answer) -> Option<bool> {
    let text = answer
        .element("isOptedOut")
        .or_else(|| answer.element("IsOptedOut"))?;
    Some(text == "true")
}

/// One `CheckIfPhoneNumberIsOptedOut` call.
async fn check(sns: &Sns, number: &str) -> Answer {
    sns.call(
        "CheckIfPhoneNumberIsOptedOut",
        &[(PHONE_NUMBER_PARAM, number)],
    )
    .await
}

/// One `OptInPhoneNumber` call.
///
/// `pub(crate)` so the sibling opt-out listing suite clears an opt-out through the
/// same wire call.
pub(crate) async fn opt_in(sns: &Sns, number: &str) -> Answer {
    sns.call("OptInPhoneNumber", &[(PHONE_NUMBER_PARAM, number)])
        .await
}

/// The opt-out state the check reports, asserting the call was served.
///
/// `pub(crate)` so the sibling opt-out listing suite reads one number's state back
/// through the SAME call rather than writing a second reader.
pub(crate) async fn reported(sns: &Sns, case: &str, number: &str) -> bool {
    let answer = check(sns, number).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the check to answer 200, got {} / {}",
        answer.status, answer.body
    );
    reported_opt_out(&answer).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected an isOptedOut member, got {}",
            answer.body
        )
    })
}

/// Record `number` as opted out, as the out-of-band opt-out writes it, with the
/// instant of its previous opt-in (if any).
fn seed_opted_out(sns: &Sns, number: &str, last_opt_in: Option<SystemTime>) {
    seed_opted_out_at(sns, number, SystemTime::now(), last_opt_in);
}

/// Record `number` as having opted out AT `opted_out_at`, as the out-of-band opt-out
/// writes it, with the instant of its previous opt-in (if any).
///
/// `pub(crate)` so the sibling opt-out listing suite — whose rule is about the ORDER
/// the opt-out instants put the numbers in — seeds a record the same way rather than
/// writing a second seeder.
pub(crate) fn seed_opted_out_at(
    sns: &Sns,
    number: &str,
    opted_out_at: SystemTime,
    last_opt_in: Option<SystemTime>,
) {
    let state = sns.state();
    let mut state = state.lock();
    state.phone_numbers.insert(
        number.to_string(),
        PhoneNumberRecord {
            phone_number: number.to_string(),
            opted_out_at: Some(opted_out_at),
            last_opt_in,
        },
    );
}

/// An opted-out number is reported as opted out until it is opted in, for every
/// phone-number form the rule accepts — and the opt-in is what changed the record.
#[tokio::test]
async fn opting_in_an_opted_out_number_clears_the_opt_out_the_check_reports() {
    let sns = Sns::new();
    let numbers = e164_numbers();

    for (case, number) in &numbers {
        seed_opted_out(&sns, number, None);
        assert!(
            reported(&sns, case, number).await,
            "case {case:?}: expected the seeded opt-out to be reported"
        );
        // A read is a report: the second check answers the same as the first.
        assert!(
            reported(&sns, case, number).await,
            "case {case:?}: a second check must report the same opt-out"
        );
    }

    for (case, number) in &numbers {
        let answer = opt_in(&sns, number).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the opt-in to answer 200, got {} / {}",
            answer.status, answer.body
        );
        assert!(
            !reported(&sns, case, number).await,
            "case {case:?}: expected the opted-in number to report false"
        );

        let state = sns.state();
        let state = state.lock();
        let record = state
            .phone_numbers
            .get(number.as_str())
            .unwrap_or_else(|| panic!("case {case:?}: the opt-in must keep the record"));
        assert!(
            !record.is_opted_out(),
            "case {case:?}: expected the record's opt-out cleared, got {record:?}"
        );
        assert!(
            record.last_opt_in.is_some(),
            "case {case:?}: expected the opt-in instant recorded, got {record:?}"
        );
    }

    // Every other number's record is untouched by the opt-ins above: each call
    // changed exactly the number it named.
    let state = sns.state();
    let state = state.lock();
    assert_eq!(
        state.phone_numbers.len(),
        numbers.len(),
        "expected one record per seeded number, got {:?}",
        state.phone_numbers
    );
}

/// A number this account holds no opt-out for reports `false` and asks for what
/// already is: the opt-in is answered and writes nothing.
#[tokio::test]
async fn opting_in_a_number_that_is_not_opted_out_changes_nothing() {
    let sns = Sns::new();

    for (case, number) in e164_numbers() {
        assert!(
            !reported(&sns, case, &number).await,
            "case {case:?}: a number with no record must report false"
        );
        let answer = opt_in(&sns, &number).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        assert!(
            !reported(&sns, case, &number).await,
            "case {case:?}: the number must still report false"
        );
        assert!(
            sns.state().lock().phone_numbers.is_empty(),
            "case {case:?}: a number that is not opted out must leave the store empty, \
             got {:?}",
            sns.state().lock().phone_numbers
        );
    }

    // A number whose record is already opted in: the opt-in has nothing to clear, so
    // it neither refuses nor moves the instant the interval is measured from.
    let number = "+12065550111";
    let earlier = SystemTime::now() - Duration::from_secs(60);
    {
        let state = sns.state();
        let mut state = state.lock();
        state.phone_numbers.insert(
            number.to_string(),
            PhoneNumberRecord {
                phone_number: number.to_string(),
                opted_out_at: None,
                last_opt_in: Some(earlier),
            },
        );
    }
    let answer = opt_in(&sns, number).await;
    assert_eq!(
        answer.status, 200,
        "an already opted-in number: expected 200, got {} / {}",
        answer.status, answer.body
    );
    let state = sns.state();
    let state = state.lock();
    let record = &state.phone_numbers[number];
    assert_eq!(
        record.last_opt_in,
        Some(earlier),
        "an already opted-in number: the opt-in instant must not move, got {record:?}"
    );
}

/// An opt-in is available once per documented interval: a number whose previous
/// opt-in is younger than the interval stays opted out, and one whose previous opt-in
/// is older is opted in.
#[tokio::test]
async fn a_second_opt_in_inside_the_documented_interval_is_refused() {
    let sns = Sns::new();
    let margin = Duration::from_secs(60);
    // Ages of the previous opt-in, spanning the interval bound: just inside it, far
    // inside it, and just past it.
    let cases: &[(&str, Duration, bool)] = &[
        ("opted in a minute ago", margin, false),
        ("opted in just inside the interval", OPT_IN_INTERVAL - margin, false),
        ("opted in just past the interval", OPT_IN_INTERVAL + margin, true),
        (
            "opted in long past the interval",
            OPT_IN_INTERVAL * 4,
            true,
        ),
    ];

    for (at, (case, age, available)) in cases.iter().enumerate() {
        let number = format!("+1206555{at:04}");
        seed_opted_out(&sns, &number, Some(SystemTime::now() - *age));

        let answer = opt_in(&sns, &number).await;
        if *available {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert!(
                !reported(&sns, case, &number).await,
                "case {case:?}: expected the number opted in"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(opt_in_too_soon_message().as_str()),
                "case {case:?}: expected the documented-interval sentence, got {}",
                answer.body
            );
            assert!(
                reported(&sns, case, &number).await,
                "case {case:?}: a refused opt-in must leave the number opted out"
            );
        }
        assert!(
            !answer.body.contains(number.as_str()),
            "case {case:?}: the response must not echo the sensitive number, got {}",
            answer.body
        );
    }
}

/// A value that is not a phone number in E.164 format is bad input to the operation
/// that ACTS on the number: the refusal names the member and the constraint, echoes
/// none of the (sensitive) value, and leaves the store alone.
#[tokio::test]
async fn a_phone_number_that_is_not_e164_is_refused_by_the_opt_in() {
    let sns = Sns::new();
    let opted_out = "+12065550100";
    seed_opted_out(&sns, opted_out, None);

    let action = "OptInPhoneNumber";
    for (case, value) in not_e164_values() {
        let answer = sns.call(action, &[(PHONE_NUMBER_PARAM, &value)]).await;
        assert_eq!(
            answer.status, 400,
            "{action} case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "{action} case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message"),
            Some(invalid_phone_number_message()),
            "{action} case {case:?}: expected the fixed badly-formatted-number \
             sentence, got {}",
            answer.body
        );
        if !value.is_empty() {
            assert!(
                !answer.body.contains(value.as_str()),
                "{action} case {case:?}: the refusal must not echo the sensitive \
                 value, got {}",
                answer.body
            );
        }
        assert!(
            reported_opt_out(&answer).is_none(),
            "{action} case {case:?}: a refused request must report no opt-out \
             state, got {}",
            answer.body
        );
    }

    {
        // The refusals touched nothing: the seeded record is exactly as it was.
        let state = sns.state();
        let state = state.lock();
        assert_eq!(
            state.phone_numbers.len(),
            1,
            "{action}: the refused calls must not add a record, got {:?}",
            state.phone_numbers
        );
        assert!(
            state.phone_numbers[opted_out].is_opted_out(),
            "{action}: the refused calls must leave the seeded opt-out standing"
        );
    }

    // And the number the refusals named nothing about is still opted out over the
    // wire, which is the state a served call reads.
    assert!(
        reported(&sns, "the seeded number", opted_out).await,
        "expected the seeded opt-out to survive every refusal"
    );
}
