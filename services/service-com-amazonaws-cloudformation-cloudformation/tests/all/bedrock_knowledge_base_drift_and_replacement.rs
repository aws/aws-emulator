// SPDX-License-Identifier: CC0-1.0

//! The two halves of an `AWS::Bedrock::KnowledgeBase` resource CloudFormation cannot carry
//! out, end to end over the awsQuery wire.
//!
//! RULE 1 (drift): drift detection is supported per resource TYPE, and CloudFormation
//! publishes which types it supports — so a resource whose type is NOT among them cannot be
//! checked at all, however well its owning service reports its configuration:
//! `DetectStackResourceDrift` is refused with `ValidationError` naming the type, nothing is
//! recorded on the resource, and a resource of a SUPPORTED type in the same stack is checked
//! and reported as usual.
//!
//! RULE 2 (replacement): CloudFormation carries a change to a create-only property out by
//! REPLACING the resource — the replacement is created before the resource it replaces is
//! deleted — so the create goes to the owning service while the old resource still exists,
//! and a declaration that keeps the resource's NAME asks that service for a second resource
//! under a name it already holds. That service refuses it, and CloudFormation reports the
//! refusal on the LOGICAL resource as `UPDATE_FAILED` — the logical resource is being
//! updated — in the `AlreadyExists` class a client branches on, with the stack rolling back
//! and both knowledge bases keeping the ids and the embedding models they had.
//!
//! The rows span what each rule generalises over: for the refusal, the knowledge base of
//! this estate against the `AWS::IAM::Role` beside it (a type drift detection IS supported
//! for, read from its own service) and the stack's own drift information; for the
//! replacement, the create-only value inside `KnowledgeBaseConfiguration` changed on one
//! knowledge base of a pair and on both of them, so the stack-level summary names every
//! resource that failed.
//!
//! Everything goes through the one router `stack_lifecycle.rs` builds, over each service's
//! own wire — awsQuery for CloudFormation, Amazon Bedrock Agent's `restJson1` for the
//! knowledge-base reads.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::bedrock_knowledge_base_identity::{
    knowledge_base_resource, model_arn, opensearch_storage, published_outputs, TITAN_G1, TITAN_V2,
};
use crate::stack_lifecycle::{
    call, deploy, describe_stack, member, resource_event_reason, resource_event_statuses, router,
    xml_blocks, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_drift_status;

/// The sentence CloudFormation answers for a resource whose type drift detection is not
/// supported for, with the type as its one variable part.
fn unsupported_sentence(resource_type: &str) -> String {
    format!("Drift detection is not supported for ResourceType {resource_type}")
}

/// The `HandlerErrorCode` class a resource handler puts a name already in use in — the one a
/// client branches on to rename or stage the change.
const ALREADY_EXISTS: &str = "AlreadyExists";

/// How long a rolled-back update is given to settle, and how often the stack is polled while
/// it does.
///
/// Generous on purpose: the walk this exercises asks the owning service for a create, for a
/// delete and for the reads that conclude both, and every one of those is advanced by a READ
/// of the stack. The poll waits for the CONDITION — a terminal status — rather than for a
/// fixed time.
const SETTLE_POLLS: usize = 80;
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// `DetectStackResourceDrift` for one resource of a stack.
async fn detect_resource_drift(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> (u16, String) {
    call(
        router,
        "DetectStackResourceDrift",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await
}

/// `DescribeStackResource` for one resource of a stack.
async fn describe_stack_resource(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await
}

/// One Amazon Bedrock Agent `GetKnowledgeBase` over the `restJson1` wire its model declares.
async fn get_knowledge_base(
    router: &MultiServiceRouter,
    knowledge_base_id: &str,
) -> (u16, Value) {
    let region = crate::stack_lifecycle::REGION;
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://bedrock.{region}.amazonaws.com/knowledgebases/{knowledge_base_id}"
        ))
        .header("host", format!("bedrock.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/bedrock/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed Amazon Bedrock Agent request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The physical id the stack reports for one of its resources.
async fn physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status, body) = describe_stack_resource(router, stack, logical_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResource({logical_id}) answered {status} with {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: the stack reports a physical id for {logical_id}, got {body}")
    })
}

/// Poll the stack until it reports a status that is not an in-progress one, and report it.
///
/// Every read is what advances the owning service's side of the walk, so the poll is what a
/// client's own waiter is: `DescribeStacks` until the operation has concluded.
async fn settled_status(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    for _ in 0..SETTLE_POLLS {
        let (status, body) = describe_stack(router, stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks({stack}) answered {status} with {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            return reported;
        }
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the stack {stack} never left an in-progress status");
}

/// The `(name, embeddingModelArn)` Amazon Bedrock Agent reports for one knowledge base.
async fn reported_knowledge_base(
    router: &MultiServiceRouter,
    case: &str,
    knowledge_base_id: &str,
) -> (String, String) {
    let (status, body) = get_knowledge_base(router, knowledge_base_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetKnowledgeBase({knowledge_base_id}) answered {status} with {body}"
    );
    let knowledge_base = &body["knowledgeBase"];
    (
        knowledge_base["name"].as_str().unwrap_or_default().to_string(),
        knowledge_base["knowledgeBaseConfiguration"]["vectorKnowledgeBaseConfiguration"]
            ["embeddingModelArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// RULE 1: a check of a resource whose TYPE drift detection is not supported for is refused,
/// naming the type, and records nothing — while a resource of a supported type in the same
/// stack is checked and reported.
#[tokio::test]
async fn a_knowledge_base_cannot_be_checked_for_drift_while_the_role_beside_it_can() {
    let case = "a knowledge base and the role beside it";
    let router = router();
    let stack = "kbdrift-unsupported";
    let template = json!({
        "Resources": {
            "Access": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "kbdrift-unsupported-access",
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "bedrock.amazonaws.com"},
                            "Action": "sts:AssumeRole"
                        }]
                    }
                }
            },
            "Escalation": knowledge_base_resource(
                Some("kbdrift-unsupported-escalation"),
                None,
                "kbdrift-unsupported",
                TITAN_G1,
                opensearch_storage("kbdrift-unsupported", "unsupported-index"),
            )
        },
        "Outputs": published_outputs(&["Escalation"])
    })
    .to_string();
    deploy(&router, stack, &template).await;

    // The knowledge base: refused, naming the type, with nothing recorded on the resource.
    let (status, body) = detect_resource_drift(&router, stack, "Escalation").await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the check of the knowledge base to be refused, got {status} \
         {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected CloudFormation's ValidationError code, got {body}"
    );
    let expected = unsupported_sentence("AWS::Bedrock::KnowledgeBase");
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected the refusal {expected:?}, got {body}"
    );
    let (status, detail) = describe_stack_resource(&router, stack, "Escalation").await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResource(Escalation) answered {status} with {detail}"
    );
    let got = xml_text(&detail, "StackResourceDriftStatus");
    assert_eq!(
        got.as_deref(),
        Some(resource_drift_status::NOT_CHECKED),
        "case {case:?}: expected the knowledge base to be left unchecked, got {got:?}"
    );
    assert!(
        xml_text(&detail, "LastCheckTimestamp").is_none(),
        "case {case:?}: expected no check instant on the knowledge base, got {detail}"
    );

    // The role beside it: a type drift detection IS supported for, checked and reported.
    let (status, body) = detect_resource_drift(&router, stack, "Access").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the check of the role to be served, got {status} {body}"
    );
    let got = xml_text(&body, "StackResourceDriftStatus");
    assert_eq!(
        got.as_deref(),
        Some(resource_drift_status::IN_SYNC),
        "case {case:?}: expected the untouched role to be reported IN_SYNC, got {body}"
    );
    let (status, detail) = describe_stack_resource(&router, stack, "Access").await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResource(Access) answered {status} with {detail}"
    );
    let got = xml_text(&detail, "StackResourceDriftStatus");
    assert_eq!(
        got.as_deref(),
        Some(resource_drift_status::IN_SYNC),
        "case {case:?}: expected the check of the role to be recorded, got {got:?}"
    );
}

/// One row of the replacement table: which of the stack's knowledge bases the update changes
/// the embedding model of.
struct ReplacementCase {
    case: &'static str,
    stack: &'static str,
    /// The logical ids whose declaration the update re-points at another embedding model, and
    /// which therefore have to be replaced.
    replaced: &'static [&'static str],
}

/// RULE 2: a replacement whose create the owning service refuses because the NAME is still
/// held is reported on the logical resource as `UPDATE_FAILED` in the `AlreadyExists` class,
/// the stack rolls back, and nothing of either knowledge base changed.
#[tokio::test]
async fn a_same_name_replacement_fails_as_already_exists_and_the_stack_rolls_back() {
    // The rows differ along how many of the pair the update replaces: one of them, and both.
    for ReplacementCase {
        case,
        stack,
        replaced,
    } in [
        ReplacementCase {
            case: "one knowledge base of the pair is replaced",
            stack: "kbreplace-one",
            replaced: &["Escalation"],
        },
        ReplacementCase {
            case: "both knowledge bases are replaced",
            stack: "kbreplace-both",
            replaced: &["Escalation", "Runbooks"],
        },
    ] {
        let router = router();
        // Both knowledge bases keep their declared NAME across the revisions, and the
        // revision changes only the create-only value inside
        // `KnowledgeBaseConfiguration.VectorKnowledgeBaseConfiguration`.
        let template = |models: &[(&str, &str)]| {
            let mut resources = serde_json::Map::new();
            for (logical_id, model) in models {
                resources.insert(
                    (*logical_id).to_string(),
                    knowledge_base_resource(
                        Some(&format!("{stack}-{}", logical_id.to_lowercase())),
                        None,
                        stack,
                        model,
                        opensearch_storage(stack, &format!("{logical_id}-index")),
                    ),
                );
            }
            json!({
                "Resources": Value::Object(resources),
                "Outputs": published_outputs(&["Escalation", "Runbooks"])
            })
            .to_string()
        };
        let before = template(&[("Escalation", TITAN_G1), ("Runbooks", TITAN_G1)]);
        deploy(&router, stack, &before).await;

        let mut deployed: Vec<(&str, String)> = Vec::new();
        for logical_id in ["Escalation", "Runbooks"] {
            let created = physical_id(&router, case, stack, logical_id).await;
            deployed.push((logical_id, created));
        }

        // The update re-points the named knowledge bases at another embedding model, which is
        // the type's create-only value: CloudFormation has to create a replacement, and the
        // name the declaration keeps is still held by the knowledge base it replaces.
        let after = template(&[
            (
                "Escalation",
                if replaced.contains(&"Escalation") {
                    TITAN_V2
                } else {
                    TITAN_G1
                },
            ),
            (
                "Runbooks",
                if replaced.contains(&"Runbooks") {
                    TITAN_V2
                } else {
                    TITAN_G1
                },
            ),
        ]);
        let (status, body) = call(
            &router,
            "UpdateStack",
            &[member("StackName", stack), member("TemplateBody", &after)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateStack({stack}) answered {status}: {body}"
        );
        let settled = settled_status(&router, case, stack).await;
        assert_eq!(
            settled, "UPDATE_ROLLBACK_COMPLETE",
            "case {case:?}: expected the update to roll back, got {settled:?}"
        );

        for logical_id in replaced {
            // ONE failure event per replaced logical resource, reported as an UPDATE of that
            // resource, carrying the class a client branches on.
            let statuses = resource_event_statuses(&router, case, stack, logical_id).await;
            let failures = statuses
                .iter()
                .filter(|status| *status == "UPDATE_FAILED")
                .count();
            assert_eq!(
                failures, 1,
                "case {case:?}: expected one UPDATE_FAILED event for {logical_id}, got \
                 {statuses:?}"
            );
            let reason = resource_event_reason(&router, stack, logical_id, "UPDATE_FAILED").await;
            assert!(
                reason.contains(&format!("HandlerErrorCode: {ALREADY_EXISTS}")),
                "case {case:?}: expected {logical_id}'s failure to carry HandlerErrorCode: \
                 {ALREADY_EXISTS}, got {reason:?}"
            );
            assert!(
                reason.contains("already exists"),
                "case {case:?}: expected {logical_id}'s failure to report the name as taken, \
                 got {reason:?}"
            );
        }

        // The stack-level summary names every resource that failed, as a failure to UPDATE.
        let (status, body) = describe_stack(&router, stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks({stack}) answered {status} with {body}"
        );
        let reason = stack_reason(&router, case, stack).await;
        for logical_id in replaced {
            assert!(
                reason.contains(logical_id),
                "case {case:?}: expected the stack's reason to name {logical_id}, got \
                 {reason:?}"
            );
        }

        // Nothing of either knowledge base changed: each keeps the id its create minted, its
        // name, and the embedding model it was created with.
        for (logical_id, created) in &deployed {
            let now = physical_id(&router, case, stack, logical_id).await;
            assert_eq!(
                &now, created,
                "case {case:?}: expected {logical_id} to keep the id its create minted, got \
                 {now:?}"
            );
            let (name, model) = reported_knowledge_base(&router, case, created).await;
            assert_eq!(
                name,
                format!("{stack}-{}", logical_id.to_lowercase()),
                "case {case:?}: expected {logical_id} to keep its declared name, got {name:?}"
            );
            assert_eq!(
                model,
                model_arn(TITAN_G1),
                "case {case:?}: expected {logical_id} to keep the embedding model it was \
                 created with, got {model:?}"
            );
        }
    }
}

/// The stack's own `StackStatusReason`.
async fn stack_reason(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackEvents({stack}) answered {status} with {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(stack))
        .filter_map(|entry| xml_text(entry, "ResourceStatusReason"))
        .collect::<Vec<String>>()
        .join(" | ")
}
