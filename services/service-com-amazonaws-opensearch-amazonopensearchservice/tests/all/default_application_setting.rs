// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service default-application setting:
//! `PutDefaultApplicationSetting` and `GetDefaultApplicationSetting`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the applications from
//! [`super::application_updates`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the setting designates an APPLICATION, so it round-trips the ARN of any application
//!   this account holds — over a table of applications differing in the shape and the
//!   length of their names, each ARN the one the create minted — with the read repeated to
//!   show it changes nothing and the next write replacing the previous value;
//! * while nothing is default the read is refused with the modelled
//!   `ResourceNotFoundException`, and a write either way answers with the
//!   `applicationArn` the request named — the application the call acted on —
//!   including the write that clears the setting;
//! * clearing is asked for by naming the ARN in effect: a clear naming any other
//!   application leaves the setting exactly as it was;
//! * a well-formed ARN this account holds no application under is refused with the
//!   modelled `ResourceNotFoundException`, naming the ARN the request carried, and the
//!   live default is left untouched — over a table of ARNs differing in partition,
//!   Region, account and resource form, including each published length bound and the ARN
//!   of an application that has been taken away;
//! * a value outside the expression the service validates an application ARN against,
//!   and one past each published length bound, are refused with `ValidationException`
//!   and leave the stored setting untouched — the first stated as the constraint
//!   report that names the member, echoes the value and quotes the expression.

use serde_json::{json, Value};

use super::application_updates::create_application;
use super::constraint_refusal_messages::{clause, expression, message, report};
use super::domain_lifecycle::{
    error_code, poll_until, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// The expression the service validates an application ARN against, as the refusal
/// quotes it — the one the wire refusal recorded for this operation states.
const ARN_EXPRESSION: &str = r"arn:aws[a-z\-]*:[a-z0-9-]+:[a-z0-9-]*:[0-9]+:[a-z0-9-]*/(.+)";

/// `GetDefaultApplicationSetting`: `GET /opensearch/defaultApplicationSetting`.
async fn get(emulator: &Emulator) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/defaultApplicationSetting"),
            None,
        )
        .await
}

/// `PutDefaultApplicationSetting`: `PUT /opensearch/defaultApplicationSetting`.
async fn put(emulator: &Emulator, application_arn: &str, set_as_default: bool) -> Answer {
    put_body(
        emulator,
        json!({ "applicationArn": application_arn, "setAsDefault": set_as_default }),
    )
    .await
}

/// `PutDefaultApplicationSetting` with a body the caller composes, so a case can
/// leave a member out.
async fn put_body(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/defaultApplicationSetting"),
            Some(body),
        )
        .await
}

/// One application of this account, created through `CreateApplication`, and the ARN the
/// create minted for it — the value the setting is written with, so the ARNs under test
/// are the ones the service itself hands out rather than values written here.
async fn created_application(emulator: &Emulator, name: &str) -> (String, String) {
    let created = create_application(emulator, json!({ "name": name })).await;
    assert_eq!(
        created.status, 200,
        "case CreateApplication({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    (
        string_member(&created.body, "id", name),
        string_member(&created.body, "arn", name),
    )
}

/// `DeleteApplication`, for the row whose ARN names an application that has been taken
/// away.
async fn delete_application(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/application/{id}"),
            None,
        )
        .await
}

/// The `applicationArn` an answer reports, with the case named on failure.
fn reported_arn(answer: &Answer, case: &str) -> String {
    answer.body["applicationArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a string applicationArn, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// A case label for a value that may be thousands of characters long: its first
/// characters, which is what tells the rows apart.
fn label(value: &str) -> String {
    value.chars().take(24).collect()
}

/// An ARN exactly as long as `ARN`'s published maximum (2048), built by padding the
/// resource of a well-formed application ARN.
fn longest_arn() -> String {
    let prefix = "arn:aws:es:us-east-1:123456789012:application/";
    format!("{prefix}{}", "a".repeat(2048 - prefix.len()))
}

/// The names the applications of the round-trip table are created under: one row per way
/// one application's name differs from another — the 3-character lower bound
/// `ApplicationName` publishes, a hyphenated name, a digit-carrying one and the
/// 30-character upper bound — so the rule is asserted over the application rather than
/// over one ARN.
fn application_names() -> Vec<&'static str> {
    vec![
        "dfl",
        "default-setting-probe",
        "default2setting3probe4",
        "abcdefghij-klmnopqrst-uvwxy1",
    ]
}

/// Well-formed ARNs this account holds no application under: inside the expression the
/// service validates an application ARN against and inside `ARN`'s published length
/// bounds, and differing in partition, Region, account, resource form and length
/// (including each published bound).
///
/// `ARN` is `@length(min: 20, max: 2048)` with pattern `.*`, from this service's Smithy
/// model (`smithy_query(query="operation",operation="PutDefaultApplicationSetting")`), so
/// every row is past the request boundary and what refuses it is the service resolving it
/// against the applications this account holds.
fn arns_naming_no_application() -> Vec<String> {
    vec![
        // The ordinary form an application ARN takes, for an id nothing minted.
        "arn:aws:opensearch:us-east-1:123456789012:application/abc123".to_string(),
        // Another partition, Region and account.
        "arn:aws-cn:es:cn-north-1:210987654321:application/d4t4".to_string(),
        // A resource carrying colons AFTER its type: the resource is the ARN's last
        // field, so the colons belong to it.
        "arn:aws:es:eu-west-1:123456789012:application/with:colons".to_string(),
        // Exactly the shortest ARN the constraint allows (20 characters).
        "arn:aws:es:a:1:a/abc".to_string(),
        // Exactly the longest (2048 characters).
        longest_arn(),
    ]
}

#[tokio::test]
async fn the_default_application_setting_round_trips_the_arn_of_an_application_of_this_account() {
    let emulator = Emulator::new();

    let mut previous: Option<String> = None;
    for name in application_names() {
        let (_, arn) = created_application(&emulator, name).await;
        let case = format!("PutDefaultApplicationSetting({}..)", label(&arn));

        let written = put(&emulator, &arn, true).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            reported_arn(&written, &case),
            arn,
            "case {case}: the write must report the setting it put in effect, got {:?}",
            written.body
        );

        // Read back through the implemented reader: the setting holds what was
        // written, and the previous row's value is gone — the setting is ONE value,
        // so the last write wins.
        let read = get(&emulator).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from the read, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            reported_arn(&read, &case),
            arn,
            "case {case}: expected the written ARN back, got {:?}",
            read.body
        );
        if let Some(previous) = previous.as_deref() {
            assert_ne!(
                reported_arn(&read, &case),
                previous,
                "case {case}: the write must replace the previous default, got {:?}",
                read.body
            );
        }

        // The read reported the setting and changed nothing: a second one answers
        // exactly the same.
        let again = get(&emulator).await;
        assert_eq!(
            again.body, read.body,
            "case {case}: a read must not change what the next caller sees, got {:?} then {:?}",
            read.body, again.body
        );

        previous = Some(arn);
    }
}

#[tokio::test]
async fn an_unset_default_application_setting_is_not_found_and_clearing_reports_the_arn() {
    for name in application_names() {
        let emulator = Emulator::new();
        let (_, arn) = created_application(&emulator, name).await;
        let case = format!("clear({}..)", label(&arn));

        // Nothing is default yet, so the setting resolves to no resource.
        let unset = get(&emulator).await;
        assert_eq!(
            unset.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected {RECORDED_NOT_FOUND_STATUS} while nothing is default, got {} \
             with {:?}",
            unset.status, unset.body
        );
        assert_eq!(
            error_code(&unset),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            unset.body
        );

        put(&emulator, &arn, true).await;

        // Clearing names the ARN in effect, as the operation documents.
        let cleared = put(&emulator, &arn, false).await;
        assert_eq!(
            cleared.status, 200,
            "case {case}: expected 200 from the clear, got {} with {:?}",
            cleared.status, cleared.body
        );
        // The answer names the application the call acted on, which is the one the
        // request named — the clear reports the ARN it cleared.
        assert_eq!(
            reported_arn(&cleared, &case),
            arn,
            "case {case}: the clear must report the ARN it was asked about, got {:?}",
            cleared.body
        );

        let after = get(&emulator).await;
        assert_eq!(
            after.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected {RECORDED_NOT_FOUND_STATUS} after the clear, got {} with {:?}",
            after.status, after.body
        );
        assert_eq!(
            error_code(&after),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException after the clear, got {:?}",
            after.body
        );

        // The setting can be put back: clearing released the value, not the setting.
        let reset = put(&emulator, &arn, true).await;
        assert_eq!(
            reported_arn(&reset, &case),
            arn,
            "case {case}: the setting must be writable again after a clear, got {:?}",
            reset.body
        );
    }
}

#[tokio::test]
async fn clearing_an_application_that_is_not_the_default_leaves_the_setting_in_effect() {
    let emulator = Emulator::new();
    let mut arns: Vec<String> = Vec::new();
    for name in application_names() {
        let (_, arn) = created_application(&emulator, name).await;
        arns.push(arn);
    }
    let in_effect = arns[0].clone();

    put(&emulator, &in_effect, true).await;

    for other in arns.iter().skip(1) {
        let case = format!("clear-other({}..)", label(other));
        let answer = put(&emulator, other, false).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        // The answer reports the application the request named, while the SETTING —
        // read back through the operation that reports it — is the one the request did
        // not name and so did not change.
        assert_eq!(
            &reported_arn(&answer, &case),
            other,
            "case {case}: expected the ARN the request named back, got {:?}",
            answer.body
        );
        let read = get(&emulator).await;
        assert_eq!(
            reported_arn(&read, &case),
            in_effect,
            "case {case}: clearing another application must leave the default alone, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_application_of_this_account_is_refused_and_the_default_stands() {
    let emulator = Emulator::new();
    let (held_id, in_effect) = created_application(&emulator, "default-setting-held").await;
    put(&emulator, &in_effect, true).await;

    // The ARN of an application that has been taken away is one of the rows: the setting
    // designates an application, so an ARN whose application is gone names none either.
    let (removed_id, removed_arn) = created_application(&emulator, "default-setting-gone").await;
    let dropped = delete_application(&emulator, &removed_id).await;
    assert_eq!(
        dropped.status, 200,
        "case the removed application: expected 200 from DeleteApplication, got {} with {:?}",
        dropped.status, dropped.body
    );
    // The removal the clock owed the record falls due on its own; the case waits for the
    // application to be gone — read through the operation that reports one — rather than
    // for a stretch of time.
    poll_until("the removed application", || async {
        emulator
            .call(
                "GET",
                &format!("{PREFIX}/opensearch/application/{removed_id}"),
                None,
            )
            .await
            .status
            != 200
    })
    .await;

    let mut rows = arns_naming_no_application();
    rows.push(removed_arn);
    for arn in rows {
        let case = format!("PutDefaultApplicationSetting({}..)", label(&arn));
        for set_as_default in [true, false] {
            let answer = put(&emulator, &arn, set_as_default).await;
            assert_eq!(
                answer.status,
                RECORDED_NOT_FOUND_STATUS,
                "case {case} with setAsDefault={set_as_default}: expected \
                 {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case} with setAsDefault={set_as_default}: expected \
                 ResourceNotFoundException, got {:?}",
                answer.body
            );
            let expected = format!("Application not found for ARN: {arn}");
            let stated = message(&answer, &case);
            assert_eq!(
                stated, expected,
                "case {case} with setAsDefault={set_as_default}: expected {expected:?}, got \
                 {stated:?}"
            );
        }

        // The refused write left the live default where it was.
        let read = get(&emulator).await;
        assert_eq!(
            reported_arn(&read, &case),
            in_effect,
            "case {case}: a refused write must leave the setting as it was, got {:?}",
            read.body
        );
    }

    // ... and the application this account DOES hold is still the one the setting
    // designates, read back through its own operation.
    let read = get(&emulator).await;
    assert!(
        reported_arn(&read, "the held application").ends_with(&format!("application/{held_id}")),
        "case the held application: expected the setting to designate it, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_value_outside_the_arn_expression_is_refused_and_the_stored_setting_stands() {
    let emulator = Emulator::new();
    let (_, in_effect) = created_application(&emulator, "default-setting-shape").await;
    put(&emulator, &in_effect, true).await;

    // Inside `ARN`'s length bound (20..2048) and so past the boundary validator, yet
    // outside the expression the service validates an application ARN against: no `arn`
    // literal, too few fields, an empty partition or service field, an account that is
    // not digits, a resource naming no value under its type, or the literal in another
    // case. Each is refused with the constraint report that echoes the value, names the
    // member and quotes the expression.
    let outside: Vec<&str> = vec![
        "not-an-arn-at-all-xx",
        "arn:aws:es:us-east-1:application/abc",
        "arn:aws:es:us-east-1:123456789012:",
        "arn:aws:es:us-east-1:123456789012:application",
        "arn::es:us-east-1:123456789012:application/abc",
        "arn:aws::us-east-1:123456789012:application/abc",
        "arn:aws:es:us-east-1:not-an-account:application/abc",
        "ARN:aws:es:us-east-1:123456789012:application/abc",
    ];

    for value in outside {
        let case = format!("PutDefaultApplicationSetting({:?}..)", label(value));
        let answer = put(&emulator, value, true).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 for a value outside the expression, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );
        let expected = report(&[clause(
            value,
            "applicationArn",
            &expression(ARN_EXPRESSION),
        )]);
        let stated = message(&answer, &case);
        assert_eq!(
            stated, expected,
            "case {case}: expected {expected:?}, got {stated:?}"
        );

        // A refused write stored nothing: the setting still holds what the accepted
        // write put in effect.
        let read = get(&emulator).await;
        assert_eq!(
            reported_arn(&read, &case),
            in_effect,
            "case {case}: a refused write must leave the setting as it was, got {:?}",
            read.body
        );
    }

    // One past each published length bound: refused at the request boundary, against
    // `ARN`'s own `@length`, rather than against the expression above.
    let too_long = format!("{}x", longest_arn());
    for value in ["arn:aws:es:a:1:a/ab", too_long.as_str()] {
        let case = format!("PutDefaultApplicationSetting({:?}..)", label(value));
        let answer = put(&emulator, value, true).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 for a value past a published length bound, got {} with \
             {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );
        let read = get(&emulator).await;
        assert_eq!(
            reported_arn(&read, &case),
            in_effect,
            "case {case}: a refused write must leave the setting as it was, got {:?}",
            read.body
        );
    }

    // `setAsDefault` is required, so a write that leaves it out is refused too, and
    // changes nothing either.
    let incomplete = put_body(&emulator, json!({ "applicationArn": in_effect })).await;
    assert_eq!(
        incomplete.status, 400,
        "case PutDefaultApplicationSetting(no setAsDefault): expected 400, got {} with {:?}",
        incomplete.status, incomplete.body
    );
    let read = get(&emulator).await;
    assert_eq!(
        reported_arn(&read, "no setAsDefault"),
        in_effect,
        "case PutDefaultApplicationSetting(no setAsDefault): the setting must stand, got {:?}",
        read.body
    );
}
