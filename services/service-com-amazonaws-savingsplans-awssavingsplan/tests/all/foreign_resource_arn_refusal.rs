// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every `resourceArn`-taking operation shares past the model's
//! `@pattern`: an ARN that is well formed but does not name a resource of THIS
//! service is a 400 `ValidationException` whose message names the ARN the
//! caller sent, identically on every one of those operations.
//!
//! The pattern `SavingsPlanArn` declares pins the partition to the literal
//! `aws`, the region, the account and the `savingsplan/<uuid>` resource, but
//! leaves the service field as `[a-z]+` (letters only — a namespace carrying a
//! digit never reaches a handler) — so the one thing left for a handler to
//! refuse is an ARN whose service namespace is another all-letter one.
//! `ListTagsForResource`, `TagResource` and `UntagResource` each used to carry
//! their own copy of that refusal; the rows below drive the shared path through
//! all three and assert they answer with the same status, the same code and the
//! same message.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Value};

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// Every operation whose input carries a `resourceArn`, i.e. every caller of
/// the shared refusal.
///
/// Source: `smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-savingsplans-awssavingsplan/\
/// smithy-model/com-amazonaws-savingsplans-awssavingsplan.json")`.
const RESOURCE_ARN_OPERATIONS: [&str; 3] = ["ListTagsForResource", "TagResource", "UntagResource"];

/// This service's ARN namespace — the service field an ARN of ours carries.
///
/// Source: `smithy_query(query="service_info", …)` → `awsService.arnNamespace`.
const ARN_NAMESPACE: &str = "savingsplans";

/// A request for `operation` pointed at `resource_arn`, carrying whatever else
/// that operation requires so the only thing under test is the ARN.
fn request(operation: &str, resource_arn: &str) -> Value {
    let mut request = json!({ "resourceArn": resource_arn });
    if operation == "TagResource" {
        request["tags"] = json!({ "k": "v" });
    }
    if operation == "UntagResource" {
        request["tagKeys"] = json!(["k"]);
    }
    request
}

/// An ARN that satisfies the model's pattern, assembled field by field so a row
/// can vary one field at a time. The partition is fixed, since `SavingsPlanArn`
/// pins it to the literal `aws`.
fn arn(service: &str, region: &str, account: &str, resource_id: &str) -> String {
    format!("arn:aws:{service}:{region}:{account}:savingsplan/{resource_id}")
}

/// The one refusal, run through every operation that takes a `resourceArn`:
/// same status, same code, same message, whatever the ARN's other fields say.
#[tokio::test]
async fn a_foreign_resource_arn_is_refused_identically_on_every_tagging_operation() {
    let emulator = Emulator::new();

    // (case, resourceArn) — each row satisfies `SavingsPlanArn`'s pattern and
    // differs from the next along a field the rule does NOT look at (region,
    // account, resource uuid) or along the one it does (the service namespace,
    // which is never ours here).
    let foreign: [(&str, String); 7] = [
        (
            "another service, no region",
            arn(
                "iam",
                "",
                "000000000000",
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
        (
            "another service, a region",
            arn(
                "sns",
                "eu-west-1",
                "123456789012",
                "11111111-2222-4333-8444-555555555555",
            ),
        ),
        (
            "another service, a China region",
            arn(
                "kms",
                "cn-north-1",
                "210987654321",
                "aaaaaaaa-bbbb-1ccc-9ddd-eeeeeeeeeeee",
            ),
        ),
        (
            "another service, no region and another account",
            arn(
                "lambda",
                "",
                "999999999999",
                "0f0f0f0f-1e1e-5d2d-b3c3-4b4b4b4b4b4b",
            ),
        ),
        (
            "a namespace that merely starts like ours",
            arn(
                "savingsplan",
                "us-east-1",
                "000000000000",
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
        (
            "a namespace that merely extends ours",
            arn(
                "savingsplansx",
                "",
                "000000000000",
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
        (
            "a single-letter namespace",
            arn(
                "q",
                "ap-southeast-2",
                "000000000000",
                "deadbeef-dead-3bee-8fee-f00df00df00d",
            ),
        ),
    ];

    for (case, resource_arn) in foreign {
        let expected = format!(
            "resourceArn '{resource_arn}' is not the ARN of a {ARN_NAMESPACE} savingsplan resource"
        );
        for operation in RESOURCE_ARN_OPERATIONS {
            let answer = emulator
                .call(operation, request(operation, &resource_arn))
                .await;
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

/// The other side of the same rule: an ARN that IS ours gets past the shared
/// refusal on every one of those operations, so the question becomes whether
/// the store holds the plan — a 404 when it does not.
#[tokio::test]
async fn one_of_our_arns_gets_past_the_refusal_on_every_tagging_operation() {
    let emulator = Emulator::new();

    // (case, resourceArn) — ours, with and without a region, across the account
    // ids and the uuid versions the pattern allows.
    let ours: [(&str, String); 4] = [
        (
            "no region",
            arn(
                ARN_NAMESPACE,
                "",
                "000000000000",
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
        (
            "a region",
            arn(
                ARN_NAMESPACE,
                "us-east-1",
                "123456789012",
                "11111111-2222-4333-8444-555555555555",
            ),
        ),
        (
            "a China region",
            arn(
                ARN_NAMESPACE,
                "cn-north-1",
                "210987654321",
                "aaaaaaaa-bbbb-1ccc-9ddd-eeeeeeeeeeee",
            ),
        ),
        (
            "another account, no region",
            arn(
                ARN_NAMESPACE,
                "",
                "999999999999",
                "0f0f0f0f-1e1e-5d2d-b3c3-4b4b4b4b4b4b",
            ),
        ),
    ];

    for (case, resource_arn) in ours {
        let expected = format!("Resource with resourceArn: {resource_arn} doesn't exist");
        for operation in RESOURCE_ARN_OPERATIONS {
            let answer = emulator
                .call(operation, request(operation, &resource_arn))
                .await;
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

/// And a plan that exists is still written, read and un-written through its own
/// ARN on all three operations, so the shared refusal swallows no success path.
#[tokio::test]
async fn an_existing_plans_arn_passes_the_refusal_on_every_tagging_operation() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let savings_plan_id = emulator
        .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.1" }))
        .await;
    let resource_arn = text(&emulator.plan(&savings_plan_id).await, "savingsPlanArn");

    let tagged = emulator
        .call(
            "TagResource",
            json!({ "resourceArn": resource_arn, "tags": { "k": "v" } }),
        )
        .await;
    assert_eq!(
        tagged.status, 200,
        "TagResource on {resource_arn}: expected 200, got {} {:?}",
        tagged.status, tagged.body
    );

    let listed = emulator
        .call("ListTagsForResource", json!({ "resourceArn": resource_arn }))
        .await;
    assert_eq!(
        listed.status, 200,
        "ListTagsForResource on {resource_arn}: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.get("tags"),
        &json!({ "k": "v" }),
        "ListTagsForResource on {resource_arn}: expected the tag written, got {:?}",
        listed.body
    );

    let untagged = emulator
        .call(
            "UntagResource",
            json!({ "resourceArn": resource_arn, "tagKeys": ["k"] }),
        )
        .await;
    assert_eq!(
        untagged.status, 200,
        "UntagResource on {resource_arn}: expected 200, got {} {:?}",
        untagged.status, untagged.body
    );
    let listed = emulator
        .call("ListTagsForResource", json!({ "resourceArn": resource_arn }))
        .await;
    assert_eq!(
        listed.get("tags"),
        &json!({}),
        "ListTagsForResource after UntagResource on {resource_arn}: expected no tags, got {:?}",
        listed.body
    );
}
