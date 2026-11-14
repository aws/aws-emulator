// SPDX-License-Identifier: CC0-1.0

//! What the tag family answers when its `ResourceARN` names no registered scalable target.
//!
//! The rule under test, one sentence: `ResourceARN` "identifies the Application Auto Scaling
//! scalable target" the call addresses, so a well-formed ARN that identifies no registered
//! target is the resource reported ABSENT by every operation of the family — `TagResource`,
//! `UntagResource` and `ListTagsForResource`, each of which models
//! `ResourceNotFoundException`
//! (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`)
//! — with the fixed sentence "Scalable Target not found" and the ARN asked about carried in
//! the shape's `ResourceName` member, at the client-error status this service answers it on.
//!
//! Asserted over a TABLE whose rows differ along HOW the ARN fails to resolve (an id this
//! service never minted, ids of other forms, another account, another Region, another
//! partition, and the ARN of a target that WAS registered and has been deregistered) and
//! along the mutation's own payload (how many tags are written, how many keys are removed),
//! with a registered target standing beside them so each row is a rule rather than a blanket
//! refusal.

use serde_json::{json, Value};

use crate::emulator::{error_code, error_message, Emulator, ACCOUNT, REGION};

/// The sentence every operation of the tagging family carries for an ARN that names no
/// registered scalable target. Spelled out here rather than read from the implementation: the
/// TEXT is what the rule owes a caller.
const TARGET_NOT_FOUND: &str = "Scalable Target not found";

/// The status both answer with: the client-error status a request that is not going to be
/// served carries.
const CLIENT_ERROR: u16 = 400;

/// One way an ARN can satisfy the modelled `@pattern` on `ResourceARN`
/// (`^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$`) and still
/// name no registered scalable target.
struct ArnRow {
    /// What makes this ARN one that resolves to nothing.
    case: &'static str,
    /// The ARN itself.
    arn: String,
}

/// The ARNs the rows are built from, each well-formed and none of them naming a registered
/// target: `deregistered` is the ARN this service minted for a target that has since been
/// deregistered, so the table covers an ARN that WAS real beside ones that never were.
fn unresolvable_arns(deregistered: &str) -> Vec<ArnRow> {
    vec![
        ArnRow {
            case: "the ARN of a target that has been deregistered",
            arn: deregistered.to_string(),
        },
        ArnRow {
            case: "an id this service never minted",
            arn: format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 00000000-0000-0000-0000-000000000000"
            ),
        },
        ArnRow {
            case: "an id of another form",
            arn: format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/nothing-here"
            ),
        },
        ArnRow {
            case: "an id of a single character",
            arn: format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/a"),
        },
        ArnRow {
            case: "an ARN of another account",
            arn: format!(
                "arn:aws:application-autoscaling:{REGION}:111122223333:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        },
        ArnRow {
            case: "an ARN of another Region",
            arn: format!(
                "arn:aws:application-autoscaling:eu-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        },
        ArnRow {
            case: "an ARN of another partition",
            arn: format!(
                "arn:aws-us-gov:application-autoscaling:us-gov-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        },
    ]
}

/// The mutations aimed at `resource_arn`, each with a payload of its own: the refusal is about
/// the resource the ARN names, so neither how many tags are written nor how many keys are
/// removed changes it. Every tag key and value here is a legal one, so no row is refused for
/// its payload.
fn mutations(resource_arn: &str) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "one tag written",
            "TagResource",
            json!({"ResourceARN": resource_arn, "Tags": {"env": "sim2real"}}),
        ),
        (
            "several tags written",
            "TagResource",
            json!({
                "ResourceARN": resource_arn,
                "Tags": {"env": "sim2real", "owner": "orders-team", "cost-center": "1234"},
            }),
        ),
        (
            "one key removed",
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["env"]}),
        ),
        (
            "several keys removed",
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["env", "owner", "cost-center"]}),
        ),
        (
            "no key named at all",
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": []}),
        ),
    ]
}

/// RULE: a tag mutation whose `ResourceARN` names no registered scalable target reports the
/// RESOURCE as absent — HTTP 400, `ResourceNotFoundException`, the fixed sentence and the ARN
/// asked about in `ResourceName` — exactly as the tag READ reports it.
#[tokio::test]
async fn a_tag_mutation_naming_no_registered_target_reports_the_resource_absent() {
    let emulator = Emulator::empty();

    // Two targets: one that stays registered, so a refusal below is about the ARN and not
    // about the operations themselves, and one that is deregistered to give the table the ARN
    // of a target that was real.
    let resident = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resident_arn = emulator.target_arn(&resident).await;
    let withdrawn = emulator
        .given_target("dynamodb:table:WriteCapacityUnits", 1, 10)
        .await;
    let withdrawn_arn = emulator.target_arn(&withdrawn).await;
    let (status, body) = emulator
        .call("DeregisterScalableTarget", withdrawn.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the deregistration\": expected 200, got {status} {body:?}"
    );

    for row in unresolvable_arns(&withdrawn_arn) {
        for (payload, operation, request) in mutations(&row.arn) {
            let case = format!("{} on {operation} with {payload}", row.case);
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, CLIENT_ERROR,
                "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} {body:?}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {code:?} in {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, TARGET_NOT_FOUND,
                "case {case:?}: expected the message {TARGET_NOT_FOUND:?}, got {message:?}"
            );
            // The ARN asked about is reported in the modelled member rather than spliced into
            // the sentence, which states the condition and nothing else.
            assert_eq!(
                body["ResourceName"],
                Value::String(row.arn.clone()),
                "case {case:?}: expected the ARN asked about in ResourceName, got {body:?}"
            );
            assert!(
                !message.contains(&row.arn) && !message.contains("scalable-target"),
                "case {case:?}: expected the sentence to carry nothing of the ARN, got \
                 {message:?}"
            );
        }
    }

    // The refusals changed nothing: the target that IS registered still takes a tag and
    // reports it back, so none of the rows above was a blanket refusal of the two mutations.
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": resident_arn, "Tags": {"env": "sim2real"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the resident target\": expected TagResource to answer 200, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": resident_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case \"the resident target\": expected ListTagsForResource to answer 200, got {status} \
         {body:?}"
    );
    assert_eq!(
        body["Tags"],
        json!({"env": "sim2real"}),
        "case \"the resident target\": expected the tag just written, got {body:?}"
    );
}

/// RULE: the tag READ answers the same ARNs the same way the two mutations do — the RESOURCE
/// reported absent, with its fixed sentence and the ARN asked about in `ResourceName` — so one
/// condition reads alike across the family.
#[tokio::test]
async fn the_tag_read_reports_the_same_arns_as_an_absent_resource() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let arn = emulator.target_arn(&target).await;
    let (status, body) = emulator
        .call("DeregisterScalableTarget", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the deregistration\": expected 200, got {status} {body:?}"
    );

    for row in unresolvable_arns(&arn) {
        let case = row.case;
        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": row.arn}))
            .await;
        assert_eq!(
            status, CLIENT_ERROR,
            "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {code:?} in {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, TARGET_NOT_FOUND,
            "case {case:?}: expected the message {TARGET_NOT_FOUND:?}, got {message:?}"
        );
        assert_eq!(
            body["ResourceName"],
            json!(row.arn),
            "case {case:?}: expected the ARN asked about in ResourceName, got {body:?}"
        );
    }
}

/// RULE: the refusal is about the ARN resolving to no registered target, so a mutation on an
/// ARN that DOES resolve is served — the written tags are readable, and the removed ones are
/// gone — whatever the batch looks like.
#[tokio::test]
async fn a_tag_mutation_on_a_registered_target_round_trips() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("ecs:service:DesiredCount", 1, 10)
        .await;
    let arn = emulator.target_arn(&target).await;

    // Rows differ along the batch: the keys written, whether a key already present is
    // overwritten, and which keys are removed. Each row states the whole tag set it expects
    // the target to carry afterwards, read back through the implemented read.
    let rows: &[(&str, &str, Value, Value)] = &[
        (
            "one tag written",
            "TagResource",
            json!({"ResourceARN": arn, "Tags": {"env": "sim2real"}}),
            json!({"env": "sim2real"}),
        ),
        (
            "a second tag beside it",
            "TagResource",
            json!({"ResourceARN": arn, "Tags": {"owner": "orders-team"}}),
            json!({"env": "sim2real", "owner": "orders-team"}),
        ),
        (
            "an existing key taking a new value",
            "TagResource",
            json!({"ResourceARN": arn, "Tags": {"env": "staging"}}),
            json!({"env": "staging", "owner": "orders-team"}),
        ),
        (
            "one key removed",
            "UntagResource",
            json!({"ResourceARN": arn, "TagKeys": ["env"]}),
            json!({"owner": "orders-team"}),
        ),
        (
            "a key the target does not carry removed",
            "UntagResource",
            json!({"ResourceARN": arn, "TagKeys": ["env"]}),
            json!({"owner": "orders-team"}),
        ),
        (
            "every remaining key removed",
            "UntagResource",
            json!({"ResourceARN": arn, "TagKeys": ["owner"]}),
            json!({}),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {body:?}"
        );
        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            &body["Tags"], expected,
            "case {case:?}: expected the tag set {expected:?}, got {body:?}"
        );
    }
}
