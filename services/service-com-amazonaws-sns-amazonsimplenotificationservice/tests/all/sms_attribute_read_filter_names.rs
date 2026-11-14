// SPDX-License-Identifier: CC0-1.0

//! A read of the account's SMS settings that NAMES the settings it wants is held to the
//! settings Amazon SNS defines, driven over the wire through the router the emulator serves
//! in production.
//!
//! The RULE asserted here: the SMS setting names are a CLOSED set, so a request whose
//! `attributes` list carries a name SNS does not define asks for no setting of an account at
//! all and is REFUSED — `400`, `awsQueryError` code `InvalidParameter`
//! (`smithy_query(query="operation", operation="GetSMSAttributes")` reports
//! `InvalidParameterException {awsQueryError: InvalidParameter, httpResponseCode: 400}`),
//! stating the name the request sent — rather than answered with that entry simply absent,
//! which is what the report says of a setting SNS DOES define and the account holds nothing
//! for. A refused read reports nothing and writes nothing, so a later read of a defined name
//! answers exactly what it answered before.
//!
//! The rows differ along what the rule generalises over: the FORM the undefined name takes (a
//! name of no API at all, a typo of a defined one, a defined one in the wrong case, one with
//! surrounding space, one that is an attribute of another SNS resource), WHERE in the list it
//! sits (alone, before a defined name, after one, twice), and how many names the list
//! carries. The positive control covers EVERY name the closed set holds, not only the one the
//! recorded case named.

use std::collections::HashMap;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    unknown_sms_attribute_message, DEFAULT_MONTHLY_SPEND_LIMIT, DEFAULT_SENDER_ID_ATTRIBUTE,
    DEFAULT_SMS_TYPE_ATTRIBUTE, MONTHLY_SPEND_LIMIT_ATTRIBUTE, SMS_ATTRIBUTE_NAMES,
};

use super::platform_applications::entries_of;
use super::topics_subscriptions_publish::{Answer, Sns};

/// One `GetSMSAttributes` naming `requested`, as the awsQuery form spells a list: one
/// numbered `member` per name.
async fn read(sns: &Sns, requested: &[&str]) -> Answer {
    let params: Vec<(String, String)> = requested
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                format!("attributes.member.{}", index + 1),
                (*name).to_string(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call("GetSMSAttributes", &borrowed).await
}

/// One `SetSMSAttributes` writing `name` to `value`, asserted answered — the settings a
/// refused read must leave exactly as they were.
async fn write(sns: &Sns, name: &str, value: &str) {
    let answer = sns
        .call(
            "SetSMSAttributes",
            &[
                ("attributes.entry.1.key", name),
                ("attributes.entry.1.value", value),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "writing {name}={value}: expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// The settings a read naming `requested` reports, asserted answered.
async fn reported(sns: &Sns, case: &str, requested: &[&str]) -> HashMap<String, String> {
    let answer = read(sns, requested).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the read of {requested:?} to answer 200, got {} / {}",
        answer.status, answer.body
    );
    entries_of(&answer.body)
}

/// Names Amazon SNS defines no SMS setting for, each in a different FORM.
///
/// Source: the complement of `SMS_ATTRIBUTE_NAMES`, which is the set
/// `SetSMSAttributesInput$attributes` documents
/// (`smithy_query(query="doc", shape="SetSMSAttributesInput", member="attributes")`). None of
/// these is in it — the rows are a name of no API at all, a plural typo of a defined name, a
/// defined name in the wrong case (the names are case-sensitive), a defined name with
/// surrounding space, and an attribute name of another SNS resource (a topic's).
fn undefined_names() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a name of no API at all", "NotARealSmsAttribute"),
        ("a plural typo of a defined name", "MonthlySpendLimits"),
        ("a defined name in the wrong case", "monthlyspendlimit"),
        ("a defined name with surrounding space", " MonthlySpendLimit "),
        ("an attribute of another resource", "DisplayName"),
    ]
}

/// A read naming an undefined setting is refused with the modelled `InvalidParameter`,
/// whatever form that name takes and wherever in the list it sits — and the refusal states
/// the name the request sent.
#[tokio::test]
async fn a_read_naming_a_setting_sns_does_not_define_is_refused() {
    let sns = Sns::new();
    write(&sns, MONTHLY_SPEND_LIMIT_ATTRIBUTE, "25").await;
    write(&sns, DEFAULT_SMS_TYPE_ATTRIBUTE, "Transactional").await;

    // (case, the names the read asks for, the name the refusal must state — the first
    // undefined one in the order the request listed them)
    let mut rows: Vec<(String, Vec<&str>, &str)> = Vec::new();
    for (form, name) in undefined_names() {
        rows.push((format!("{form} alone"), vec![name], name));
        rows.push((
            format!("{form} after a defined name"),
            vec![MONTHLY_SPEND_LIMIT_ATTRIBUTE, name],
            name,
        ));
        rows.push((
            format!("{form} before a defined name"),
            vec![name, MONTHLY_SPEND_LIMIT_ATTRIBUTE],
            name,
        ));
        rows.push((format!("{form} twice"), vec![name, name], name));
    }
    // Two undefined names at once: the refusal names the one the request asked for first,
    // so the answer does not depend on how the list happens to be walked.
    rows.push((
        "two undefined names".to_string(),
        vec!["MonthlySpendLimits", "NotARealSmsAttribute"],
        "MonthlySpendLimits",
    ));
    rows.push((
        "two undefined names the other way round".to_string(),
        vec!["NotARealSmsAttribute", "MonthlySpendLimits"],
        "NotARealSmsAttribute",
    ));
    // An undefined name among every defined one: one bad entry refuses the whole read.
    let mut every_name: Vec<&str> = SMS_ATTRIBUTE_NAMES.to_vec();
    every_name.push("NotARealSmsAttribute");
    rows.push((
        "an undefined name among every defined one".to_string(),
        every_name,
        "NotARealSmsAttribute",
    ));

    for (case, requested, blamed) in &rows {
        let answer = read(&sns, requested).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            answer.status, answer.body
        );
        let code = answer.code();
        assert_eq!(
            code, "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {code:?} in {}",
            answer.body
        );
        let message = answer.element("Message");
        assert_eq!(
            message.as_deref(),
            Some(unknown_sms_attribute_message(blamed).as_str()),
            "case {case:?}: expected the unknown-attribute sentence for {blamed:?}, got \
             {message:?}"
        );

        // The refusal changed nothing: the settings the account holds read back as they
        // were, through a read naming them and through a read naming none.
        let named = reported(&sns, case, &[MONTHLY_SPEND_LIMIT_ATTRIBUTE]).await;
        assert_eq!(
            named.get(MONTHLY_SPEND_LIMIT_ATTRIBUTE).map(String::as_str),
            Some("25"),
            "case {case:?}: the refused read must leave the spend limit as it was, got {named:?}"
        );
        let whole = reported(&sns, case, &[]).await;
        assert_eq!(
            whole.get(DEFAULT_SMS_TYPE_ATTRIBUTE).map(String::as_str),
            Some("Transactional"),
            "case {case:?}: the refused read must leave every setting as it was, got {whole:?}"
        );
    }
}

/// Every name the closed set holds is a name a read may ask for: each is answered, the ones
/// the account holds with their values and the ones it holds nothing for with no entry at
/// all — which is what an absent entry means, and what an undefined name is NOT answered
/// with.
#[tokio::test]
async fn every_defined_setting_name_is_a_name_a_read_may_ask_for() {
    let sns = Sns::new();
    write(&sns, DEFAULT_SENDER_ID_ATTRIBUTE, "Brand7").await;

    for name in SMS_ATTRIBUTE_NAMES {
        let case = format!("the defined name {name}");
        let reported = reported(&sns, &case, &[name]).await;
        let expected: Option<&str> = match *name {
            DEFAULT_SENDER_ID_ATTRIBUTE => Some("Brand7"),
            // The spend limit is a setting an account always has, so it is reported
            // whether or not a caller set one; every other unset setting has no entry.
            MONTHLY_SPEND_LIMIT_ATTRIBUTE => Some(DEFAULT_MONTHLY_SPEND_LIMIT),
            _ => None,
        };
        assert_eq!(
            reported.get(*name).map(String::as_str),
            expected,
            "case {case:?}: expected {expected:?} for the setting, got {reported:?}"
        );
    }

    // Every defined name at once is answered too, and names no entry the set does not hold.
    let whole = reported(&sns, "every defined name at once", SMS_ATTRIBUTE_NAMES).await;
    for name in whole.keys() {
        assert!(
            SMS_ATTRIBUTE_NAMES.contains(&name.as_str()),
            "case \"every defined name at once\": {name:?} is no setting SNS defines, got \
             {whole:?}"
        );
    }
}
