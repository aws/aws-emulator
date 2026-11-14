// SPDX-License-Identifier: CC0-1.0

//! `DetectStackResourceDrift` checks ONE resource of a stack, end to end over the awsQuery
//! wire.
//!
//! **The rule under test.** The operation reads the resource its `LogicalResourceId` names
//! from the service that OWNS it, compares that configuration with the one the stack's
//! template declared for the resource, and answers that comparison as a
//! `StackResourceDrift` straight away: the stack and resource identity, the expected and
//! actual property documents, the fully-qualified path of every value the two disagree about
//! with both values and the kind of difference, the resulting drift status and the instant of
//! the check. What it found is RECORDED on that resource, so the reads of the stack report
//! the same result afterwards — and on that resource only: a check of one resource says
//! nothing about the resources it never looked at, so every other resource of the stack, and
//! the stack's own drift information, are left as they were. A `StackName` that resolves to
//! no stack, and a `LogicalResourceId` the stack has not deployed, are refused with HTTP 400
//! `ValidationError`.
//!
//! A resource whose TYPE drift detection is not supported for is refused the same way:
//! "resources that don't currently support drift detection can't be checked", and the one
//! status that would report such a non-check in a `StackResourceDrift` is the one the model
//! records CloudFormation "does not currently return", so there is no record to answer with.
//!
//! The rows span the class the rule generalises over rather than one recorded case: a
//! resource nothing touched, one whose nested declared document was changed outside the
//! stack, one whose scalar property was, and one deleted outside the stack; one resource in
//! the stack and several, with the check aimed at one of them; and two owning services
//! (Amazon S3 and Amazon SNS), so the difference paths asserted are ones each of those
//! services really reports. The refusal rows span the types nothing here reports a
//! configuration for, over two namespaces.
//!
//! Everything goes through the one router `stack_lifecycle.rs` builds, over each service's
//! own wire — awsQuery for CloudFormation and Amazon SNS, REST for Amazon S3.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::provisioned_resources::s3;
use crate::stack_lifecycle::{
    call, deploy, describe_stack, list_members, member, router, sns, xml_text,
};
use crate::stack_resources::is_iso8601_instant;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    drift::difference_type, resource_drift_status, DRIFT_STATUS_NOT_CHECKED,
};

/// One difference the answer must carry: the path, the expected value, the actual value and
/// the kind of difference.
type Difference = (&'static str, &'static str, &'static str, &'static str);

/// What a row does to the stack's resources, outside the stack, before the check runs.
#[derive(Debug, Clone, Copy)]
enum Tamper {
    /// Nothing: the resources are as the deployment left them.
    Nothing,
    /// Delete this logical id's bucket in Amazon S3.
    DeleteBucket(&'static str),
    /// Suspend the versioning the template declared for this bucket.
    SuspendVersioning(&'static str),
    /// Change the display name the template declared for this topic, in Amazon SNS.
    RenameTopicDisplay(&'static str, &'static str),
}

/// One row of the single-resource drift table.
struct DetectCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    tamper: Tamper,
    /// The logical id the check is aimed at.
    detect: &'static str,
    resource_type: &'static str,
    status: &'static str,
    /// The whole `ExpectedProperties` document, or `None` when the member must be absent.
    expected_properties: Option<Value>,
    /// Members the `ActualProperties` document must carry — a SUBSET, because what an owning
    /// service reports is its own whole view of the resource, of which only the declared
    /// properties are compared. `None` when the member must be absent.
    actual_properties: Option<Value>,
    differences: Vec<Difference>,
    /// Whether the checked resource is one `DescribeStackResourceDrifts` reports afterwards:
    /// false for a resource no comparison was carried out over, which that read leaves out.
    reported_afterwards: bool,
    /// Logical ids of the same stack the check must leave untouched.
    untouched: Vec<&'static str>,
}

fn detect_cases() -> Vec<DetectCase> {
    vec![
        DetectCase {
            case: "a provisioned bucket nothing touched",
            stack: "detect-one-in-sync",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "detect-one-in-sync-store"}
                    }
                }
            })
            .to_string(),
            tamper: Tamper::Nothing,
            detect: "Store",
            resource_type: "AWS::S3::Bucket",
            status: resource_drift_status::IN_SYNC,
            expected_properties: Some(json!({"BucketName": "detect-one-in-sync-store"})),
            actual_properties: Some(json!({})),
            differences: vec![],
            reported_afterwards: true,
            untouched: vec![],
        },
        DetectCase {
            case: "a nested declared document changed outside the stack",
            stack: "detect-one-document",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "detect-one-document-store",
                            "VersioningConfiguration": {"Status": "Enabled"}
                        }
                    }
                }
            })
            .to_string(),
            tamper: Tamper::SuspendVersioning("Store"),
            detect: "Store",
            resource_type: "AWS::S3::Bucket",
            status: resource_drift_status::MODIFIED,
            expected_properties: Some(json!({
                "BucketName": "detect-one-document-store",
                "VersioningConfiguration": {"Status": "Enabled"}
            })),
            actual_properties: Some(json!({
                "VersioningConfiguration": {"Status": "Suspended"}
            })),
            differences: vec![(
                "/VersioningConfiguration/Status",
                "Enabled",
                "Suspended",
                difference_type::NOT_EQUAL,
            )],
            reported_afterwards: true,
            untouched: vec![],
        },
        DetectCase {
            case: "a scalar property another service reports changed",
            stack: "detect-one-scalar",
            template: json!({
                "Resources": {
                    "Notify": {
                        "Type": "AWS::SNS::Topic",
                        "Properties": {
                            "TopicName": "detect-one-scalar-notify",
                            "DisplayName": "the stack's own"
                        }
                    }
                }
            })
            .to_string(),
            tamper: Tamper::RenameTopicDisplay("Notify", "changed outside the stack"),
            detect: "Notify",
            resource_type: "AWS::SNS::Topic",
            status: resource_drift_status::MODIFIED,
            expected_properties: Some(json!({
                "TopicName": "detect-one-scalar-notify",
                "DisplayName": "the stack's own"
            })),
            actual_properties: Some(json!({"DisplayName": "changed outside the stack"})),
            differences: vec![(
                "/DisplayName",
                "the stack's own",
                "changed outside the stack",
                difference_type::NOT_EQUAL,
            )],
            reported_afterwards: true,
            untouched: vec![],
        },
        DetectCase {
            case: "the named resource deleted outside the stack carries neither configuration",
            stack: "detect-one-deleted",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "detect-one-deleted-store"}
                    }
                }
            })
            .to_string(),
            tamper: Tamper::DeleteBucket("Store"),
            detect: "Store",
            resource_type: "AWS::S3::Bucket",
            status: resource_drift_status::DELETED,
            expected_properties: None,
            actual_properties: None,
            differences: vec![],
            reported_afterwards: true,
            untouched: vec![],
        },
        DetectCase {
            case: "one resource of several: the others are not checked",
            stack: "detect-one-of-several",
            template: concat!(
                "Resources:\n",
                "  Gone:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: detect-one-of-several-gone\n",
                "  Kept:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: detect-one-of-several-kept\n",
                "  Notify:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: detect-one-of-several-notify\n",
            )
            .to_string(),
            tamper: Tamper::DeleteBucket("Gone"),
            detect: "Kept",
            resource_type: "AWS::S3::Bucket",
            status: resource_drift_status::IN_SYNC,
            expected_properties: Some(json!({"BucketName": "detect-one-of-several-kept"})),
            actual_properties: Some(json!({})),
            differences: vec![],
            reported_afterwards: true,
            // The resource deleted outside the stack is NOT reported as drifted: this check
            // never looked at it.
            untouched: vec!["Gone", "Notify"],
        },
    ]
}

/// The physical id the stack reports for one logical resource, which is the identity every
/// read of that resource agrees on.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status, body) = describe_stack_resource(router, stack, logical_id).await;
    assert_eq!(
        status, 200,
        "DescribeStackResource({stack}, {logical_id}) answered {status} with {body}"
    );
    xml_text(&body, "PhysicalResourceId")
        .unwrap_or_else(|| panic!("case {logical_id:?}: expected a physical id, got {body}"))
}

/// `DescribeStackResource` for one resource of one stack.
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

/// `DetectStackResourceDrift` for one resource of one stack.
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

/// The drift records `DescribeStackResourceDrifts` reports for one stack, by logical id.
async fn reported_drifts(router: &MultiServiceRouter, stack: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "DescribeStackResourceDrifts",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {stack:?}: DescribeStackResourceDrifts answered {status} with {body}"
    );
    list_members(&body, "StackResourceDrifts")
        .into_iter()
        .map(|block| {
            (
                xml_text(&block, "LogicalResourceId").unwrap_or_default(),
                xml_text(&block, "StackResourceDriftStatus").unwrap_or_default(),
            )
        })
        .collect()
}

/// The differences one answered record carries, as the table writes them.
fn differences_of(block: &str) -> Vec<(String, String, String, String)> {
    list_members(block, "PropertyDifferences")
        .into_iter()
        .map(|difference| {
            (
                xml_text(&difference, "PropertyPath").unwrap_or_default(),
                xml_text(&difference, "ExpectedValue").unwrap_or_default(),
                xml_text(&difference, "ActualValue").unwrap_or_default(),
                xml_text(&difference, "DifferenceType").unwrap_or_default(),
            )
        })
        .collect()
}

/// A reported properties document as JSON.
fn document(case: &str, member_name: &str, text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("case {case:?}: expected {member_name} to be a JSON document, got {text:?}: {error}")
    })
}

/// Carry out a row's out-of-band change, through the owning service's own wire.
async fn tamper_with(router: &MultiServiceRouter, case: &DetectCase) {
    match case.tamper {
        Tamper::Nothing => {}
        Tamper::DeleteBucket(logical_id) => {
            let bucket = physical_id(router, case.stack, logical_id).await;
            let (status, _, body) = s3(router, "DELETE", &format!("/{bucket}"), b"").await;
            assert!(
                (200..300).contains(&status),
                "case {:?}: deleting {bucket:?} outside the stack answered {status} with {body}",
                case.case
            );
        }
        Tamper::SuspendVersioning(logical_id) => {
            let bucket = physical_id(router, case.stack, logical_id).await;
            let body = b"<VersioningConfiguration><Status>Suspended</Status>\
                         </VersioningConfiguration>";
            let (status, _, text) = s3(router, "PUT", &format!("/{bucket}?versioning"), body).await;
            assert!(
                (200..300).contains(&status),
                "case {:?}: suspending versioning on {bucket:?} answered {status} with {text}",
                case.case
            );
        }
        Tamper::RenameTopicDisplay(logical_id, display_name) => {
            let topic = physical_id(router, case.stack, logical_id).await;
            let (status, body) = sns(
                router,
                "SetTopicAttributes",
                &[
                    ("TopicArn", topic.as_str()),
                    ("AttributeName", "DisplayName"),
                    ("AttributeValue", display_name),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: renaming {topic:?} outside the stack answered {status} with {body}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_single_resource_check_answers_and_records_that_resource_s_comparison() {
    let router = router();

    for case in detect_cases() {
        deploy(&router, case.stack, &case.template).await;
        tamper_with(&router, &case).await;

        // Nothing has checked this stack yet, so nothing is reported for it.
        let before = reported_drifts(&router, case.stack).await;
        assert!(
            before.is_empty(),
            "case {:?}: expected no drift records before the check, got {before:?}",
            case.case
        );

        let physical = physical_id(&router, case.stack, case.detect).await;
        let stack_body = describe_stack(&router, case.stack).await.1;
        let stack_id = xml_text(&stack_body, "StackId").unwrap_or_default();

        // The comparison is answered by this call itself — there is no detection id to poll
        // and nothing to wait for.
        let (status, body) = detect_resource_drift(&router, case.stack, case.detect).await;
        assert_eq!(
            status, 200,
            "case {:?}: DetectStackResourceDrift answered {status} with {body}",
            case.case
        );

        for (tag, want) in [
            ("StackId", stack_id.clone()),
            ("LogicalResourceId", case.detect.to_string()),
            ("PhysicalResourceId", physical.clone()),
            ("ResourceType", case.resource_type.to_string()),
            ("StackResourceDriftStatus", case.status.to_string()),
        ] {
            let got = xml_text(&body, tag);
            assert_eq!(
                got.as_deref(),
                Some(want.as_str()),
                "case {:?}: expected <{tag}> to be {want:?}, got {got:?} from {body}",
                case.case
            );
        }

        let timestamp = xml_text(&body, "Timestamp").unwrap_or_default();
        assert!(
            is_iso8601_instant(&timestamp),
            "case {:?}: expected the check's Timestamp to be an instant, got {timestamp:?}",
            case.case
        );

        let expected_properties = xml_text(&body, "ExpectedProperties");
        match &case.expected_properties {
            Some(want) => {
                let text = expected_properties.unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected ExpectedProperties to be reported, got {body}",
                        case.case
                    )
                });
                let got = document(case.case, "ExpectedProperties", &text);
                assert_eq!(
                    &got, want,
                    "case {:?}: expected ExpectedProperties to be {want}, got {got}",
                    case.case
                );
            }
            None => assert!(
                expected_properties.is_none(),
                "case {:?}: expected no ExpectedProperties, got {expected_properties:?}",
                case.case
            ),
        }

        let actual_properties = xml_text(&body, "ActualProperties");
        match &case.actual_properties {
            Some(want) => {
                let text = actual_properties.unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected ActualProperties to be reported, got {body}",
                        case.case
                    )
                });
                let got = document(case.case, "ActualProperties", &text);
                for (property, value) in want.as_object().expect("a document of members") {
                    assert_eq!(
                        got.get(property),
                        Some(value),
                        "case {:?}: expected ActualProperties to carry {property}={value}, \
                         got {got}",
                        case.case
                    );
                }
            }
            None => assert!(
                actual_properties.is_none(),
                "case {:?}: expected no ActualProperties, got {actual_properties:?}",
                case.case
            ),
        }

        let differences = differences_of(&body);
        let want: Vec<(String, String, String, String)> = case
            .differences
            .iter()
            .map(|(path, expected, actual, kind)| {
                (
                    (*path).to_string(),
                    (*expected).to_string(),
                    (*actual).to_string(),
                    (*kind).to_string(),
                )
            })
            .collect();
        assert_eq!(
            differences, want,
            "case {:?}: expected PropertyDifferences {want:?}, got {differences:?} from {body}",
            case.case
        );

        // What the check found is RECORDED on the resource it was carried out over: the
        // reads of that resource report the same status and the same instant afterwards.
        let (status, detail) = describe_stack_resource(&router, case.stack, case.detect).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeStackResource answered {status} with {detail}",
            case.case
        );
        for (tag, want) in [
            ("StackResourceDriftStatus", case.status.to_string()),
            ("LastCheckTimestamp", timestamp.clone()),
        ] {
            let got = xml_text(&detail, tag);
            assert_eq!(
                got.as_deref(),
                Some(want.as_str()),
                "case {:?}: expected the resource's <{tag}> to be {want:?}, got {got:?} \
                 from {detail}",
                case.case
            );
        }

        // …and on that resource ONLY: every other resource of the stack is still unchecked,
        // whatever happened to it outside the stack.
        let reported = reported_drifts(&router, case.stack).await;
        let expected: Vec<(String, String)> = if case.reported_afterwards {
            vec![(case.detect.to_string(), case.status.to_string())]
        } else {
            vec![]
        };
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?} to be the stack's checked resources, got \
             {reported:?}",
            case.case
        );
        for untouched in &case.untouched {
            let (status, detail) = describe_stack_resource(&router, case.stack, untouched).await;
            assert_eq!(
                status, 200,
                "case {:?}: DescribeStackResource({untouched}) answered {status} with {detail}",
                case.case
            );
            let got = xml_text(&detail, "StackResourceDriftStatus");
            assert_eq!(
                got.as_deref(),
                Some(resource_drift_status::NOT_CHECKED),
                "case {:?}: expected {untouched:?} to be left unchecked, got {got:?}",
                case.case
            );
            assert!(
                xml_text(&detail, "LastCheckTimestamp").is_none(),
                "case {:?}: expected {untouched:?} to carry no check instant, got {detail}",
                case.case
            );
        }

        // The STACK's own drift information is a statement about every resource it holds, so
        // a check of one resource leaves it alone.
        let stack_body = describe_stack(&router, case.stack).await.1;
        let got = xml_text(&stack_body, "StackDriftStatus");
        assert_eq!(
            got.as_deref(),
            Some(DRIFT_STATUS_NOT_CHECKED),
            "case {:?}: expected the stack's own drift status to be untouched, got {got:?}",
            case.case
        );
    }
}

/// Assert a refusal: HTTP 400 with CloudFormation's own `ValidationError` code and a message
/// built from what the request named.
fn assert_refused(case: &str, status: u16, body: &str, names: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the request to be refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected CloudFormation's ValidationError code, got {body}"
    );
    let message = xml_text(body, "Message").unwrap_or_default();
    for named in names {
        assert!(
            message.contains(named),
            "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
        );
    }
}

/// A resource whose TYPE drift detection is not supported for cannot be checked at all, so
/// the request for its drift is REFUSED — naming the type — and records nothing: the one
/// status that would report such a non-check in a `StackResourceDrift` is the one the model
/// records CloudFormation "does not currently return".
///
/// The rows are the types nothing here reports a configuration for, over two namespaces.
#[tokio::test]
async fn a_check_of_a_type_drift_detection_is_not_supported_for_is_refused() {
    let router = router();
    let stack = "detect-one-unsupported";
    deploy(
        &router,
        stack,
        concat!(
            "Resources:\n",
            "  Signal:\n",
            "    Type: AWS::CloudWatch::CompositeAlarm\n",
            "    Properties:\n",
            "      AlarmName: detect-one-unsupported-signal\n",
            "      AlarmRule: ALARM(detect-one-unsupported-other)\n",
            "  Escalation:\n",
            "    Type: AWS::Bedrock::Agent\n",
            "    Properties:\n",
            "      AgentName: detect-one-unsupported-agent\n",
            "      AgentResourceRoleArn: arn:aws:iam::000000000000:role/detect-one-unsupported\n",
        ),
    )
    .await;

    for (case, logical_id, resource_type) in [
        (
            "a composite alarm, which no owning service here reports",
            "Signal",
            "AWS::CloudWatch::CompositeAlarm",
        ),
        (
            "an agent, which nothing here provisions",
            "Escalation",
            "AWS::Bedrock::Agent",
        ),
    ] {
        let (status, body) = detect_resource_drift(&router, stack, logical_id).await;
        assert_refused(case, status, &body, &[resource_type]);
        // CloudFormation's own sentence for it, with the resource's TYPE as its one
        // variable part.
        let message = xml_text(&body, "Message").unwrap_or_default();
        let expected = format!("Drift detection is not supported for ResourceType {resource_type}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );

        // The refused request checked nothing, so the resource is still unchecked and
        // carries no check instant.
        let (status, detail) = describe_stack_resource(&router, stack, logical_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStackResource answered {status} with {detail}"
        );
        let got = xml_text(&detail, "StackResourceDriftStatus");
        assert_eq!(
            got.as_deref(),
            Some(resource_drift_status::NOT_CHECKED),
            "case {case:?}: expected the resource to be left unchecked, got {got:?}"
        );
        assert!(
            xml_text(&detail, "LastCheckTimestamp").is_none(),
            "case {case:?}: expected no check instant, got {detail}"
        );
    }

    // …and the stack reports no checked resource at all.
    let reported = reported_drifts(&router, stack).await;
    assert!(
        reported.is_empty(),
        "case \"after the refusals\": expected no drift records, got {reported:?}"
    );
}

/// A request that names no resource of a stack of this Region is refused, and records
/// nothing: a stack that was never created, one addressed by an id that names nothing, and a
/// logical id the stack has not deployed.
#[tokio::test]
async fn a_check_of_a_resource_the_stack_does_not_hold_is_refused() {
    let router = router();
    let stack = "detect-one-refusals";
    deploy(
        &router,
        stack,
        &json!({
            "Resources": {
                "Store": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {"BucketName": "detect-one-refusals-store"}
                }
            }
        })
        .to_string(),
    )
    .await;

    for (case, stack_name, logical_id) in [
        ("a stack that was never created", "detect-one-absent", "Store"),
        (
            "a stack id that names nothing",
            "arn:aws:cloudformation:us-east-1:000000000000:stack/detect-one-absent/0",
            "Store",
        ),
        ("a logical id the stack has not deployed", stack, "Missing"),
    ] {
        let (status, body) = detect_resource_drift(&router, stack_name, logical_id).await;
        assert_refused(case, status, &body, &[stack_name]);
    }

    // The refused requests checked nothing, so the stack still reports no checked resource.
    let reported = reported_drifts(&router, stack).await;
    assert!(
        reported.is_empty(),
        "case \"after the refusals\": expected no drift records, got {reported:?}"
    );
}
