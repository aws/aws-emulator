// SPDX-License-Identifier: CC0-1.0

//! A repository policy's CONDITION values, as every reader of the policy reports them,
//! driven end to end over the wire.
//!
//! Every case goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"). The document is submitted to
//! `SetRepositoryPolicy`, read back with `GetRepositoryPolicy` and parsed as JSON — the
//! way a client that diffs its policy against what the service reports reads it — so the
//! assertions are about the VALUES the document carries and not about its layout, which
//! `tests/all/repository_policy_canonical_form.rs` pins byte for byte.
//!
//! The rule: a repository policy is re-serialized by the policy engine, and the policy
//! language defines a condition's values as strings — so a value submitted as a JSON
//! number or boolean is reported back in its string form, while everything else about the
//! document (its version, its statements and their order, every other element's own type)
//! is reported as it was submitted. The table spans which JSON type the value was written
//! as, whether it stands alone or in a list, which operator carries it, and the same value
//! written outside a condition block.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SpencerFrontendService";

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not what these cases are about; the credential scope below
    // still routes the request to `ecr-public`.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call that must succeed, returning the parsed response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDECRPUBLIC/20260101/us-east-1\
             /ecr-public/aws4_request, SignedHeaders=host;x-amz-target, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// The `policyText` a response carries, parsed as the JSON document a client reads.
fn document(body: &Value, case: &str) -> Value {
    let text = body["policyText"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a policyText, got {body}"));
    serde_json::from_str(text)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON document, got {text:?}"))
}

/// One row: what a failing assertion calls it, the `Condition` element as submitted, and
/// the element as every reader must report it.
type Row = (&'static str, Value, Value);

/// The rows span what the rule generalises over: which JSON type the value was written as,
/// whether it stands alone or in a list, which operator carries it, and a value that was
/// already a string.
fn rows() -> Vec<Row> {
    vec![
        (
            "an integer under a numeric operator",
            json!({"NumericLessThanEquals": {"aws:MultiFactorAuthAge": 300}}),
            json!({"NumericLessThanEquals": {"aws:MultiFactorAuthAge": "300"}}),
        ),
        (
            "a fractional number, with the digits it was written with",
            json!({"NumericGreaterThan": {"aws:MultiFactorAuthAge": 1.5}}),
            json!({"NumericGreaterThan": {"aws:MultiFactorAuthAge": "1.5"}}),
        ),
        (
            "a boolean under the boolean operator",
            json!({"Bool": {"aws:SecureTransport": true}}),
            json!({"Bool": {"aws:SecureTransport": "true"}}),
        ),
        (
            "a list of numbers",
            json!({"NumericEquals": {"aws:MultiFactorAuthAge": [60, 300]}}),
            json!({"NumericEquals": {"aws:MultiFactorAuthAge": ["60", "300"]}}),
        ),
        (
            "a value already written as a string",
            json!({"StringEquals": {"aws:PrincipalAccount": "000000000000"}}),
            json!({"StringEquals": {"aws:PrincipalAccount": "000000000000"}}),
        ),
        (
            "several operators in one condition block",
            json!({
                "NumericLessThan": {"aws:MultiFactorAuthAge": 900},
                "Bool": {"aws:SecureTransport": false},
                "StringLike": {"aws:PrincipalArn": "arn:aws:iam::*:role/*"},
            }),
            json!({
                "NumericLessThan": {"aws:MultiFactorAuthAge": "900"},
                "Bool": {"aws:SecureTransport": "false"},
                "StringLike": {"aws:PrincipalArn": "arn:aws:iam::*:role/*"},
            }),
        ),
    ]
}

#[tokio::test]
async fn every_condition_value_is_reported_as_a_string() {
    let router = service();

    for (index, (case, submitted, expected)) in rows().into_iter().enumerate() {
        let name = format!("policy/condition-{index}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": name}),
            case,
        )
        .await;

        // Two statements, the second carrying the condition: the first is the control that
        // the rest of the document is reported as it was submitted.
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Sid": "AllowPull",
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": [
                        "ecr-public:BatchGetImage",
                        "ecr-public:GetDownloadUrlForLayer",
                    ],
                },
                {
                    "Sid": "Conditional",
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "ecr-public:DescribeImages",
                    "Condition": submitted,
                },
            ],
        });
        let written = ok(
            &router,
            "SetRepositoryPolicy",
            json!({"repositoryName": name, "policyText": policy.to_string()}),
            case,
        )
        .await;
        let read = ok(
            &router,
            "GetRepositoryPolicy",
            json!({"repositoryName": name}),
            case,
        )
        .await;

        for (reader, body) in [("SetRepositoryPolicy", &written), ("GetRepositoryPolicy", &read)] {
            let reported = document(body, case);
            let condition = &reported["Statement"][1]["Condition"];
            assert_eq!(
                condition, &expected,
                "case {case:?}: expected {reader} to report {expected}, got {condition}"
            );

            // Everything else about the document is reported as it was submitted.
            assert_eq!(
                reported["Version"], policy["Version"],
                "case {case:?}: expected {reader} to keep the policy version, got {reported}"
            );
            assert_eq!(
                reported["Statement"].as_array().map(Vec::len),
                Some(2),
                "case {case:?}: expected {reader} to keep both statements, got {reported}"
            );
            assert_eq!(
                reported["Statement"][0], policy["Statement"][0],
                "case {case:?}: expected {reader} to keep the unconditional statement, \
                 got {reported}"
            );
            for member in ["Sid", "Effect", "Principal", "Action"] {
                assert_eq!(
                    reported["Statement"][1][member], policy["Statement"][1][member],
                    "case {case:?}: expected {reader} to keep {member}, got {reported}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_document_with_no_condition_is_reported_as_submitted() {
    let router = service();
    let name = "policy/no-condition";
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": name}),
        "no condition",
    )
    .await;

    // The rule is about a CONDITION's values, so a document carrying none is reported
    // exactly as it was submitted — including the strings that merely LOOK like numbers
    // and booleans, which are not retyped either.
    let policy = json!({
        "Version": "2012-10-17",
        "Id": "300",
        "Statement": [{
            "Sid": "true",
            "Effect": "Allow",
            "Principal": "*",
            "Action": ["ecr-public:BatchGetImage", "ecr-public:DescribeImages"],
        }],
    });
    let written = ok(
        &router,
        "SetRepositoryPolicy",
        json!({"repositoryName": name, "policyText": policy.to_string()}),
        "no condition",
    )
    .await;
    let reported = document(&written, "no condition");
    assert_eq!(
        reported, policy,
        "case \"no condition\": expected the document reported as submitted, got {reported}"
    );
}
