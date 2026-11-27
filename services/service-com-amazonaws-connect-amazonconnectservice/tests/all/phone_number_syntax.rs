// SPDX-License-Identifier: CC0-1.0

//! The form a phone-number member must have, end to end over HTTP through the router
//! the emulator serves with.
//!
//! The model snapshot spells the `@pattern` of `com.amazonaws.connect#PhoneNumber` and
//! `com.amazonaws.connect#PhoneNumberPrefix` with every backslash of the published
//! expression escaped a second time, so the expression the trait holds asks for a
//! literal backslash and admits no phone number at all. Those two members are
//! therefore off the request boundary (`skip_validation` in `src/ops/types/`) and the
//! published expressions are applied by their operations
//! (`src/state/phone_number_syntax.rs`).
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * `ClaimPhoneNumber` takes a number the published expression admits — every
//!   advertised number, whatever the length of its country's calling code — and
//!   refuses a value that expression does not admit with the modelled
//!   `InvalidParameterException` (HTTP 400) NAMING the member, for every way a value
//!   can fail that form, including each side of the fourteen-digit bound;
//! * `SearchAvailablePhoneNumbers` takes a prefix the published expression admits,
//!   with or without the leading `+`, and refuses one it does not the same way;
//! * the two expressions this crate applies are the snapshot's own with the second
//!   escaping level removed — the form the snapshot's own `SensitivePhoneNumber`
//!   carries for the same E.164 number;
//! * the two hand-written input declarations cover the member list the snapshot
//!   declares for them, so a refreshed model that adds a member fails here rather than
//!   silently dropping it.

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::ops::types::{
    CLAIM_PHONE_NUMBER_INPUT_SCHEMA, SEARCH_AVAILABLE_PHONE_NUMBERS_INPUT_SCHEMA,
};
use service_com_amazonaws_connect_amazonconnectservice::state::{
    inventory_phone_number, is_phone_number, is_phone_number_prefix, PHONE_NUMBER_PARAMETER,
    PHONE_NUMBER_PATTERN, PHONE_NUMBER_PREFIX_PARAMETER, PHONE_NUMBER_PREFIX_PATTERN,
};

use super::harness::{connect_managed, Answer, Connect};

/// The published model snapshot this crate is generated from, embedded at compile time
/// so the expressions the service applies are checked against the model rather than
/// against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-connect-amazonconnectservice.json"
));

/// `SearchAvailablePhoneNumbers`: `POST /phone-number/search-available`, every input
/// member in the body. Each suite builds its own operation helpers, as `harness`
/// documents; only the fixture itself is shared.
async fn search(connect: &Connect, body: Value) -> Answer {
    connect
        .send_answer("POST", "/phone-number/search-available", Some(body))
        .await
}

/// `ClaimPhoneNumber`: `POST /phone-number/claim`.
async fn claim(connect: &Connect, body: Value) -> Answer {
    connect
        .send_answer("POST", "/phone-number/claim", Some(body))
        .await
}

/// The sentence this service states for a member whose value it cannot read
/// (`crate::state::invalid_parameter`).
fn invalid_parameter_message(member: &str) -> String {
    format!("Invalid parameter: {member}")
}

/// The `Message` of a refusal, or the whole body when it carries none.
fn message_of(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .map_or_else(|| answer.body.to_string(), str::to_string)
}

/// The shapes whose expression this crate applies itself, each paired with the
/// expression it applies: the snapshot has to spell it with one escaping level more,
/// and nothing else about it may differ.
const RESTATED: [(&str, &str); 2] = [
    ("com.amazonaws.connect#PhoneNumber", PHONE_NUMBER_PATTERN),
    (
        "com.amazonaws.connect#PhoneNumberPrefix",
        PHONE_NUMBER_PREFIX_PATTERN,
    ),
];

/// The `@pattern` the snapshot declares on `shape`.
fn snapshot_pattern(shape: &str) -> String {
    let model: Value = serde_json::from_str(SNAPSHOT).expect("the model snapshot parses");
    model["shapes"][shape]["traits"]["smithy.api#pattern"]
        .as_str()
        .unwrap_or_else(|| panic!("the snapshot declares a @pattern on {shape}"))
        .to_string()
}

/// The member names the snapshot declares for `shape`.
fn snapshot_members(shape: &str) -> Vec<String> {
    let model: Value = serde_json::from_str(SNAPSHOT).expect("the model snapshot parses");
    model["shapes"][shape]["members"]
        .as_object()
        .unwrap_or_else(|| panic!("the snapshot declares members on {shape}"))
        .keys()
        .cloned()
        .collect()
}

#[test]
fn the_applied_expressions_are_the_snapshot_s_own_with_one_escaping_level_removed() {
    for (shape, applied) in RESTATED {
        let declared = snapshot_pattern(shape);
        let doubled = applied.replace('\\', "\\\\");
        assert_eq!(
            declared, doubled,
            "case {shape:?}: the applied expression must be the declared one with the \
             second escaping level removed; declared {declared:?}, applied {applied:?}"
        );
        assert_ne!(
            declared, applied,
            "case {shape:?}: the snapshot now spells the expression as published, so \
             the local check should give way to the request boundary's"
        );
    }

    // The same E.164 number, in the same model, spelled with one level of escaping:
    // the expression `PhoneNumber` publishes, independently of the arithmetic above.
    let sibling = snapshot_pattern("com.amazonaws.connect#SensitivePhoneNumber");
    assert_eq!(
        sibling, PHONE_NUMBER_PATTERN,
        "case \"SensitivePhoneNumber\": expected the published E.164 expression \
         {PHONE_NUMBER_PATTERN:?}, got {sibling:?}"
    );
}

#[test]
fn the_hand_written_input_declarations_cover_the_snapshot_member_lists() {
    let cases: [(&str, &str, Vec<String>); 2] = [
        (
            "ClaimPhoneNumber",
            "com.amazonaws.connect#ClaimPhoneNumberRequest",
            CLAIM_PHONE_NUMBER_INPUT_SCHEMA
                .members()
                .iter()
                .filter_map(|member| member.member_name())
                .map(str::to_string)
                .collect(),
        ),
        (
            "SearchAvailablePhoneNumbers",
            "com.amazonaws.connect#SearchAvailablePhoneNumbersRequest",
            SEARCH_AVAILABLE_PHONE_NUMBERS_INPUT_SCHEMA
                .members()
                .iter()
                .filter_map(|member| member.member_name())
                .map(str::to_string)
                .collect(),
        ),
    ];
    for (case, shape, mut declared) in cases {
        let mut modelled = snapshot_members(shape);
        declared.sort();
        modelled.sort();
        assert_eq!(
            declared, modelled,
            "case {case:?}: the hand-written declaration must carry exactly the members \
             {shape} declares; declared {declared:?}, modelled {modelled:?}"
        );
    }
}

/// The values the published expressions admit and refuse, decided by the service's own
/// predicates: the rows below are what the wire cases are built from, so a row cannot
/// claim to be malformed while the expression admits it.
#[test]
fn the_applied_expressions_admit_the_published_form_and_nothing_else() {
    // Numbers: `+`, a non-zero leading digit, then one to fourteen more digits. The
    // rows differ along what the expression generalises over — the leading digit, the
    // digit count at both ends of the bound, and the characters a value may not carry.
    let numbers: Vec<(&str, String, bool)> = vec![
        ("the shortest published number", "+12".to_string(), true),
        ("a familiar number", "+15551234567".to_string(), true),
        (
            "the longest published number",
            format!("+9{}", "8".repeat(14)),
            true,
        ),
        ("no leading plus", "15551234567".to_string(), false),
        ("a zero leading digit", "+05551234567".to_string(), false),
        ("the plus alone", "+".to_string(), false),
        ("one digit", "+1".to_string(), false),
        (
            "one digit past the published bound",
            format!("+9{}", "8".repeat(15)),
            false,
        ),
        ("grouped with hyphens", "+1-555-123-4567".to_string(), false),
        ("grouped with spaces", "+1 555 123 4567".to_string(), false),
        ("a letter among the digits", "+1555123456x".to_string(), false),
        ("empty", String::new(), false),
        (
            "a number with trailing text",
            "+15551234567 ext 9".to_string(),
            false,
        ),
    ];
    for (case, value, admitted) in numbers {
        assert_eq!(
            is_phone_number(&value),
            admitted,
            "case {case:?} ({value:?}): expected admitted={admitted}, got {}",
            is_phone_number(&value)
        );
    }

    // Prefixes: one to eleven digits, with or without the leading `+`.
    let prefixes: Vec<(&str, String, bool)> = vec![
        ("one digit", "1".to_string(), true),
        ("one digit with the plus", "+1".to_string(), true),
        ("a zero leading digit", "0".to_string(), true),
        ("eleven digits", "1".repeat(11), true),
        ("eleven digits with the plus", format!("+{}", "1".repeat(11)), true),
        ("twelve digits", "1".repeat(12), false),
        ("the plus alone", "+".to_string(), false),
        ("empty", String::new(), false),
        ("letters", "abc".to_string(), false),
        ("a grouped prefix", "+1-555".to_string(), false),
        ("two pluses", "++1".to_string(), false),
    ];
    for (case, value, admitted) in prefixes {
        assert_eq!(
            is_phone_number_prefix(&value),
            admitted,
            "case {case:?} ({value:?}): expected admitted={admitted}, got {}",
            is_phone_number_prefix(&value)
        );
    }
}

#[tokio::test]
async fn a_number_the_published_expression_admits_is_claimable_and_another_is_refused() {
    let connect = Connect::new();
    let (instance_id, _arn) = connect
        .create_instance(connect_managed("phone-number-syntax"))
        .await;

    // Every advertised number is of the published form, whatever the length of its
    // country's calling code (`US` is 1, `GB` is 2, `MX` is 52) and whatever the
    // `PhoneNumberType`: each is claimed, so the member reaches the handler at all.
    for (country, number_type) in [("US", "DID"), ("GB", "TOLL_FREE"), ("MX", "DID")] {
        let number = inventory_phone_number(country, number_type, 0)
            .unwrap_or_else(|| panic!("case ({country}, {number_type}): an inventory entry"));
        assert!(
            is_phone_number(&number),
            "case ({country}, {number_type}): the advertised number {number} must be of \
             the published form"
        );
        let answer = claim(
            &connect,
            json!({"InstanceId": instance_id, "PhoneNumber": number}),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case ({country}, {number_type}): expected HTTP 200 claiming {number}, got {} {}",
            answer.status,
            message_of(&answer)
        );
    }

    // A value the published expression does not admit is refused as the bad parameter
    // it is, naming the member — not as a number that happens not to be advertised.
    let malformed: Vec<(&str, String)> = vec![
        ("no leading plus", "15551234567".to_string()),
        ("a zero leading digit", "+05551234567".to_string()),
        ("one digit", "+1".to_string()),
        ("grouped with hyphens", "+1-555-123-4567".to_string()),
        ("a letter among the digits", "+1555123456x".to_string()),
        ("empty", String::new()),
        (
            "one digit past the published bound",
            format!("+9{}", "8".repeat(15)),
        ),
    ];
    for (case, number) in malformed {
        let answer = claim(
            &connect,
            json!({"InstanceId": instance_id, "PhoneNumber": number}),
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?} ({number:?}): expected HTTP 400, got {} {}",
            answer.status,
            message_of(&answer)
        );
        assert_eq!(
            answer.error_code, "InvalidParameterException",
            "case {case:?} ({number:?}): expected the modelled InvalidParameterException, \
             got {:?} {}",
            answer.error_code,
            message_of(&answer)
        );
        assert_eq!(
            message_of(&answer),
            invalid_parameter_message(PHONE_NUMBER_PARAMETER),
            "case {case:?} ({number:?}): expected the refusal to name the member"
        );
    }

    // The other side of the same bound: a number of the published form that the
    // inventory simply does not offer is refused for being unavailable, in this
    // operation's own sentence about the number rather than about the member.
    let longest = format!("+9{}", "8".repeat(14));
    let answer = claim(
        &connect,
        json!({"InstanceId": instance_id, "PhoneNumber": longest}),
    )
    .await;
    assert_eq!(
        answer.status,
        400,
        "case \"the longest published number\": expected HTTP 400, got {} {}",
        answer.status,
        message_of(&answer)
    );
    assert_eq!(
        message_of(&answer),
        format!("The phone number {longest} is not available to claim"),
        "case \"the longest published number\": expected the unavailable-number sentence"
    );
}

#[tokio::test]
async fn a_prefix_the_published_expression_admits_is_searched_and_another_is_refused() {
    let connect = Connect::new();
    let (instance_id, _arn) = connect
        .create_instance(connect_managed("phone-number-prefix-syntax"))
        .await;
    let advertised = inventory_phone_number("US", "DID", 0).expect("an inventory entry");

    // A prefix the expression admits is SEARCHED: written with or without the leading
    // `+`, it keeps the same numbers, and one no number starts keeps none.
    let digits = advertised
        .strip_prefix('+')
        .expect("an advertised number is E.164");
    let accepted: Vec<(&str, String, bool)> = vec![
        ("the calling code", digits[..1].to_string(), true),
        ("the calling code with the plus", format!("+{}", &digits[..1]), true),
        ("the first six digits", digits[..6].to_string(), true),
        (
            "the first six digits with the plus",
            format!("+{}", &digits[..6]),
            true,
        ),
        ("a prefix no number starts", "999".to_string(), false),
        ("a prefix no number starts, with the plus", "+999".to_string(), false),
    ];
    for (case, prefix, expect_matches) in accepted {
        let answer = search(
            &connect,
            json!({
                "InstanceId": instance_id,
                "PhoneNumberCountryCode": "US",
                "PhoneNumberType": "DID",
                "PhoneNumberPrefix": prefix,
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?} ({prefix:?}): expected HTTP 200, got {} {}",
            answer.status,
            message_of(&answer)
        );
        let numbers: Vec<String> = answer.body["AvailableNumbersList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: an AvailableNumbersList, got {}", answer.body))
            .iter()
            .map(|entry| entry["PhoneNumber"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            !numbers.is_empty(),
            expect_matches,
            "case {case:?} ({prefix:?}): expected matches={expect_matches}, got {numbers:?}"
        );
        for number in &numbers {
            assert!(
                number
                    .trim_start_matches('+')
                    .starts_with(prefix.trim_start_matches('+')),
                "case {case:?} ({prefix:?}): expected every number to start with the \
                 prefix, got {number}"
            );
        }
    }

    // A prefix the expression does not admit is refused, naming the member.
    let refused: Vec<(&str, String)> = vec![
        ("letters", "abc".to_string()),
        ("the plus alone", "+".to_string()),
        ("empty", String::new()),
        ("twelve digits", "1".repeat(12)),
        ("a grouped prefix", "+1-555".to_string()),
    ];
    for (case, prefix) in refused {
        let answer = search(
            &connect,
            json!({
                "InstanceId": instance_id,
                "PhoneNumberCountryCode": "US",
                "PhoneNumberType": "DID",
                "PhoneNumberPrefix": prefix,
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?} ({prefix:?}): expected HTTP 400, got {} {}",
            answer.status,
            message_of(&answer)
        );
        assert_eq!(
            answer.error_code, "InvalidParameterException",
            "case {case:?} ({prefix:?}): expected the modelled InvalidParameterException, \
             got {:?} {}",
            answer.error_code,
            message_of(&answer)
        );
        assert_eq!(
            message_of(&answer),
            invalid_parameter_message(PHONE_NUMBER_PREFIX_PARAMETER),
            "case {case:?} ({prefix:?}): expected the refusal to name the member"
        );
    }
}
