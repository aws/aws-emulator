// SPDX-License-Identifier: CC0-1.0

//! How this endpoint states a refusal of a value a request carried, end to end over HTTP.
//!
//! Two rules under test:
//!
//! 1. A required member the request supplied NOTHING for is refused by this endpoint's own
//!    validator, in the `Invalid <member>: …` voice it refuses its own rules in — not in
//!    the framework's schema-fact clause, which reports the absence as a constraint on a
//!    `null` value. The one member the model binds a LIST to keeps its own sentence, since
//!    on the query string an absent list and an empty one are the same request.
//! 2. A constraint clause QUOTES the value that broke the constraint as the request
//!    carried it, however long that value was — up to the one ceiling every echo is held
//!    to, so no request parameter can make the error body as large as itself. The
//!    framework's own walk renders a long value as a prefix plus `... [<n> bytes]`, which
//!    is wire text AWS never emits.
//!
//! The cases are TABLES. The rows differ along what each rule generalises over: which
//! member supplied nothing and whether it is the list one, how many members supplied
//! nothing at once, and — for the clause — the LENGTH of the refused value walked across
//! the framework's own echo ceiling and the ceiling this endpoint holds an echo to, on both
//! a member the model bounds by LENGTH and one it bounds by PATTERN.

use serde_json::Value;

use crate::revocation_status::{
    base_instant, certificate_hash, job_arn, params, profile_version_arn, timestamp_param,
    Emulator, PLATFORM,
};

/// The framework's own ceiling on the value it renders into a constraint clause
/// (`MAX_ECHOED_VALUE` in `vendor/smithy-schema-server/src/constraints.rs`): past it the
/// walk stores a prefix plus `... [<n> bytes]`, which is what this endpoint restores.
const FRAMEWORK_ECHO_CEILING: usize = 256;

/// The ceiling this endpoint holds ONE restored echo to (`service_common::validation`'s
/// `MAX_RESTORED_ECHO`), written out here so the bytes a client reads are pinned
/// independently of that module's own constant.
const RESTORED_ECHO_CEILING: usize = 16 * 1024;

/// The modelled maximum of an `Arn` member (`smithy_query(query="shape", shape="Arn")`:
/// `@length(min: 20, max: 2048)`), so a value past it is refused by the boundary validator.
const ARN_MAX_LENGTH: usize = 2048;

/// The `message` of a 400 answer.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the refusal must carry a message, got {body}"))
        .to_string()
}

/// A required member the request supplied nothing for is refused in this endpoint's own
/// voice, naming the member.
///
/// One row per required SINGLE-VALUE member, plus the LIST member — which keeps its own
/// sentence — plus the rows where SEVERAL members supplied nothing, which keep the
/// framework's enumerating envelope rather than reading as though one rule applied.
#[tokio::test]
async fn a_member_the_request_supplied_nothing_for_is_refused_in_this_endpoint_voice() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("absent");
    let job = job_arn("absent");
    let hash = certificate_hash("absent");
    let complete = params(
        base_instant(),
        PLATFORM,
        &profile,
        &job,
        std::slice::from_ref(&hash),
    );

    // The model marks all five input members `@required` + `@httpQuery`, and
    // `certificateHashes` is the one it targets at a list
    // (`smithy_query(query="operation", operation="GetRevocationStatus")`).
    let single_value = [
        "signatureTimestamp",
        "platformId",
        "profileVersionArn",
        "jobArn",
    ];
    let list_member = "certificateHashes";

    for omitted in single_value {
        let case = format!("omitted={omitted}");
        let partial: Vec<(&str, String)> = complete
            .iter()
            .filter(|(name, _)| *name != omitted)
            .cloned()
            .collect();
        let (status, body) = emulator.get(&partial).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            message(&body),
            format!("Invalid {omitted}: value must not be null"),
            "case {case}: expected this endpoint's absent-member sentence, got {body}"
        );
    }

    // The LIST member keeps the sentence it earns: absent and empty are the same request.
    let without_list: Vec<(&str, String)> = complete
        .iter()
        .filter(|(name, _)| *name != list_member)
        .cloned()
        .collect();
    let (status, body) = emulator.get(&without_list).await;
    assert_eq!(
        status, 400,
        "case omitted={list_member}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        message(&body),
        format!("{list_member} must not be null or empty"),
        "case omitted={list_member}: expected the list sentence, got {body}"
    );

    // Two members at once, and then every member: a refusal carrying several violations
    // keeps the framework's enumerating envelope, which names each of them.
    for omitted in [
        vec!["signatureTimestamp", "jobArn"],
        single_value.to_vec(),
    ] {
        let case = format!("omitted={omitted:?}");
        let partial: Vec<(&str, String)> = complete
            .iter()
            .filter(|(name, _)| !omitted.contains(name))
            .cloned()
            .collect();
        let (status, body) = emulator.get(&partial).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let stated = message(&body);
        for member in &omitted {
            assert!(
                stated.contains(member),
                "case {case}: expected the envelope to name {member}, got {stated:?}"
            );
        }
        assert!(
            stated.contains("validation error"),
            "case {case}: expected the framework's enumerating envelope, got {stated:?}"
        );
    }
}

/// A constraint clause quotes the value the request carried AS IT CARRIED IT — no prefix of
/// it — and stops at the one ceiling an echo is held to, so no request parameter can make
/// the error body as large as itself.
///
/// The rows walk the length of the refused value across the framework's own echo ceiling
/// (under it, on it, one byte past it), past the member's modelled maximum, and past the
/// restored-echo ceiling; and they do it on a member the model bounds by LENGTH and on one
/// it bounds by PATTERN, since the clause is the same for both classes.
#[tokio::test]
async fn a_constraint_clause_quotes_the_whole_submitted_value() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("quoted");
    let job = job_arn("quoted");
    let hash = certificate_hash("quoted");

    // (member, the lengths its rows walk, how a value of `length` characters that BREAKS
    // that member's constraint is built). `platformId` targets `PlatformId`, bounded by
    // @pattern ^[a-zA-Z0-9_-]+$ and by no length at all — so a value of ANY length outside
    // that class is a violation, which is what lets its rows walk the echo ceilings
    // themselves. `jobArn` targets `Arn`, bounded by @length(20, 2048) — so only a value
    // past that maximum is a violation, and its rows walk from there.
    type Rows = (&'static str, &'static [usize], fn(usize) -> String);
    let members: [Rows; 2] = [
        (
            "platformId",
            &[
                FRAMEWORK_ECHO_CEILING - 1,
                FRAMEWORK_ECHO_CEILING,
                FRAMEWORK_ECHO_CEILING + 1,
                RESTORED_ECHO_CEILING,
                RESTORED_ECHO_CEILING + 1,
            ],
            |length| format!("{}!", "a".repeat(length.saturating_sub(1))),
        ),
        (
            "jobArn",
            &[
                ARN_MAX_LENGTH + 1,
                ARN_MAX_LENGTH * 2,
                RESTORED_ECHO_CEILING,
                RESTORED_ECHO_CEILING + 1,
            ],
            |length| "x".repeat(length),
        ),
    ];

    for (member, lengths, build) in members {
        for &length in lengths {
            let case = format!("{member} of {length} characters");
            let value = build(length);
            assert_eq!(
                value.len(),
                length,
                "case {case}: the row's value must be {length} characters"
            );
            let request: Vec<(&str, String)> = params(
                base_instant(),
                PLATFORM,
                &profile,
                &job,
                std::slice::from_ref(&hash),
            )
            .into_iter()
            .map(|(name, carried)| {
                if name == member {
                    (name, value.clone())
                } else {
                    (name, carried)
                }
            })
            .collect();

            let (status, body) = emulator.get(&request).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {body}"
            );
            let stated = message(&body);
            assert!(
                stated.contains(member),
                "case {case}: the clause must name the member that carried the value, got \
                 {stated:?}"
            );
            if length <= RESTORED_ECHO_CEILING {
                assert!(
                    stated.contains(&value),
                    "case {case}: the clause must quote the whole value the request carried, \
                     got a message of {} characters",
                    stated.len()
                );
            } else {
                // Past the ceiling the clause states a prefix and the value's size: an
                // error body is never sized by the request.
                assert!(
                    !stated.contains(&value) && stated.len() < value.len(),
                    "case {case}: the clause must be bounded past the echo ceiling, got a \
                     message of {} characters",
                    stated.len()
                );
            }
        }
    }
}

/// The timestamp a request carries is not quoted into a clause by the restoring walk when
/// the request is otherwise valid: a well-formed read still answers 200 with the documented
/// empty list, so the wrapper that restores a clause cannot refuse what the walk accepts.
#[tokio::test]
async fn a_request_that_breaks_no_constraint_is_still_served() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("served");
    let job = job_arn("served");
    let hash = certificate_hash("served");
    let (status, body) = emulator
        .get(&params(
            base_instant(),
            PLATFORM,
            &profile,
            &job,
            std::slice::from_ref(&hash),
        ))
        .await;
    assert_eq!(
        status, 200,
        "a request inside every modelled bound must be served, got {status} with {body}"
    );
    assert_eq!(
        body.get("revokedEntities").and_then(Value::as_array),
        Some(&Vec::new()),
        "nothing named is revoked, so the documented empty list is the answer, got {body}"
    );
}

/// The `signatureTimestamp` of a request whose OTHER member breaks a bound is read first,
/// so the refusal names the member at fault rather than the timestamp — whichever form the
/// timestamp was sent in.
#[tokio::test]
async fn a_timestamp_in_any_form_still_lets_another_member_decide_the_refusal() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("ordering");
    let hash = certificate_hash("ordering");
    let base = timestamp_param(base_instant());
    let epoch_seconds = base_instant()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the base instant is after the epoch")
        .as_secs()
        .to_string();

    for (case, timestamp) in [
        ("date-time with the Z zone", base.clone()),
        ("epoch seconds", epoch_seconds),
        (
            "date-time with a numeric offset",
            base.replace('Z', "%2B05:30"),
        ),
    ] {
        let request: Vec<(&str, String)> = vec![
            ("signatureTimestamp", timestamp),
            ("platformId", PLATFORM.to_string()),
            ("profileVersionArn", profile.clone()),
            ("jobArn", "short".to_string()),
            ("certificateHashes", hash.clone()),
        ];
        let (status, body) = emulator.get(&request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            message(&body),
            "1 validation error detected: Value 'short' at 'jobArn' failed to satisfy \
             constraint: Member must have length greater than or equal to 20",
            "case {case}: expected the refusal to name the member whose modelled bound the \
             request violated, got {body}"
        );
    }
}
