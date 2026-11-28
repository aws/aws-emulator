// SPDX-License-Identifier: CC0-1.0

//! What the shape `SavingsPlanArn` admits, and what it refuses, on EVERY
//! operation that carries a member of it.
//!
//! The rule is the shape's own: a member typed `SavingsPlanArn` is admitted
//! exactly when its value matches the expression the shape declares; a value
//! that does not is refused at the boundary with a 400 `ValidationException`
//! quoting that expression at the member's request-rooted path, whatever the
//! operation and whether the member sits at the top of the request or inside a
//! list. A value the shape admits reaches the handler, so what answers is the
//! store (nothing held, so nothing found) rather than the validator.
//!
//! The expression pins the partition field to the literal `aws` and leaves the
//! region alternative, the account id and the uuid version/variant groups open,
//! so the rows below span exactly those: an ARN in a suffixed partition
//! (`aws-cn`, `aws-us-gov`, `aws-iso-b`) is not a value of this shape at all.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Value};

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};
use crate::tagging_error_paths::SAVINGS_PLAN_ARN_PATTERN;

/// (case, value, whether the shape admits it) — the class the shape describes,
/// spanned along each group of its expression: the fields it leaves open
/// (region present or absent, any 12-digit account, every uuid version `1-5`
/// and variant `8-b`) and each way a value can fall outside it, the partition
/// field included.
const ARNS: [(&str, &str, bool); 19] = [
    // Admitted: the open groups, varied.
    (
        "no region",
        "arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
        true,
    ),
    (
        "a region",
        "arn:aws:savingsplans:us-east-1:000000000000:savingsplan/22222222-2222-2222-9222-222222222222",
        true,
    ),
    (
        "a longer region name",
        "arn:aws:savingsplans:ap-southeast-1:123456789012:savingsplan/33333333-3333-3333-a333-333333333333",
        true,
    ),
    (
        "the lowest uuid version and the highest variant",
        "arn:aws:savingsplans::999999999999:savingsplan/44444444-4444-1444-b444-444444444444",
        true,
    ),
    (
        "the highest uuid version",
        "arn:aws:savingsplans::000000000000:savingsplan/55555555-5555-5555-8555-555555555555",
        true,
    ),
    (
        "hexadecimal throughout the uuid",
        "arn:aws:savingsplans:eu-west-3:210987654321:savingsplan/deadbeef-dead-3bee-8fee-f00df00df00d",
        true,
    ),
    // Refused: each group of the expression, missed.
    ("not an ARN at all", "not-an-arn", false),
    ("empty", "", false),
    (
        "no arn prefix",
        "savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "the China partition",
        "arn:aws-cn:savingsplans:cn-north-1:000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "the GovCloud partition",
        "arn:aws-us-gov:savingsplans::123456789012:savingsplan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "an isolated partition",
        "arn:aws-iso-b:savingsplans:eu-west-1:999999999999:savingsplan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "an upper-case partition",
        "arn:AWS:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "a service field carrying a digit",
        "arn:aws:savingsplans2::000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "a region that is not region-shaped",
        "arn:aws:savingsplans:useast1:000000000000:savingsplan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "an account id that is not 12 digits",
        "arn:aws:savingsplans::12345:savingsplan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "another resource type",
        "arn:aws:savingsplans::000000000000:plan/11111111-1111-1111-8111-111111111111",
        false,
    ),
    (
        "a uuid version outside 1-5",
        "arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-6111-8111-111111111111",
        false,
    ),
    (
        "trailing text after the uuid",
        "arn:aws:savingsplans::000000000000:savingsplan/11111111-1111-1111-8111-111111111111 ",
        false,
    ),
];

/// The whole message for a request whose single violation is a value that is
/// not of this shape, at `path`.
fn refusal(value: &str, path: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {SAVINGS_PLAN_ARN_PATTERN}"
    )
}

/// A request for `operation` carrying `arn` in its `SavingsPlanArn` member,
/// plus whatever else that operation requires, and the request-rooted path of
/// that member.
fn request(operation: &str, arn: &str) -> (Value, String) {
    if operation == "DescribeSavingsPlans" {
        return (
            json!({ "savingsPlanArns": [arn] }),
            "originalRequest.savingsPlanArns.1.member".to_string(),
        );
    }
    let mut request = json!({ "resourceArn": arn });
    if operation == "TagResource" {
        request["tags"] = json!({ "k": "v" });
    }
    if operation == "UntagResource" {
        request["tagKeys"] = json!(["k"]);
    }
    (request, "originalRequest.resourceArn".to_string())
}

/// Every operation whose input carries a member typed `SavingsPlanArn`: the
/// three `resourceArn` operations and the `savingsPlanArns` list of the plan
/// reader.
///
/// Source: `smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-savingsplans-awssavingsplan/\
/// smithy-model/com-amazonaws-savingsplans-awssavingsplan.json")`.
const OPERATIONS: [&str; 4] = [
    "ListTagsForResource",
    "TagResource",
    "UntagResource",
    "DescribeSavingsPlans",
];

/// A value the shape refuses is refused the same way on every operation that
/// carries a member of it, with the clause naming the member from the request
/// object inwards.
#[tokio::test]
async fn a_value_outside_the_shape_is_refused_on_every_operation_that_carries_it() {
    let emulator = Emulator::new();

    for (case, arn, admitted) in ARNS {
        if admitted {
            continue;
        }
        for operation in OPERATIONS {
            let (body, path) = request(operation, arn);
            let answer = emulator.call(operation, body).await;
            let expected = refusal(arn, &path);
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {operation}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code, "ValidationException",
                "case {case:?} on {operation}: expected ValidationException, got {:?}",
                answer.code
            );
            assert_eq!(
                text(&answer.body, "message"),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// A value the shape admits is not refused by the validator on any of those
/// operations: it reaches the handler, which answers from the store — a 404 for
/// an operation pointed at one resource, an empty 200 for the reader that
/// filters by ARN.
#[tokio::test]
async fn a_value_inside_the_shape_reaches_the_handler_on_every_operation_that_carries_it() {
    let emulator = Emulator::new();

    for (case, arn, admitted) in ARNS {
        if !admitted {
            continue;
        }
        for operation in OPERATIONS {
            let (body, _) = request(operation, arn);
            let answer = emulator.call(operation, body).await;
            if operation == "DescribeSavingsPlans" {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} on {operation}: expected 200, got {} {:?}",
                    answer.status, answer.body
                );
                assert!(
                    answer.list("savingsPlans").is_empty(),
                    "case {case:?} on {operation}: expected no plan, got {:?}",
                    answer.body
                );
                continue;
            }
            let expected = format!("Resource with resourceArn: {arn} doesn't exist");
            assert_eq!(
                answer.status, 404,
                "case {case:?} on {operation}: expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code, "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {:?}",
                answer.code
            );
            assert_eq!(
                text(&answer.body, "message"),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// The control: the ARN this emulator MINTS is a value of the shape, so a plan
/// it sold round-trips through every one of those operations.
#[tokio::test]
async fn the_arn_this_emulator_mints_is_a_value_of_the_shape() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let savings_plan_id = emulator
        .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.1" }))
        .await;
    let arn = text(&emulator.plan(&savings_plan_id).await, "savingsPlanArn");

    for operation in OPERATIONS {
        let (body, _) = request(operation, &arn);
        let answer = emulator.call(operation, body).await;
        assert_eq!(
            answer.status, 200,
            "minted ARN {arn} on {operation}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }
    let listed = emulator
        .call("DescribeSavingsPlans", json!({ "savingsPlanArns": [arn.clone()] }))
        .await;
    assert_eq!(
        listed.list("savingsPlans").len(),
        1,
        "minted ARN {arn}: DescribeSavingsPlans must report the one plan, got {:?}",
        listed.body
    );
}
