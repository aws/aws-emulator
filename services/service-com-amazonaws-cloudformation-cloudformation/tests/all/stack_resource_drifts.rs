// SPDX-License-Identifier: CC0-1.0

//! What `DescribeStackResourceDrifts` reports about the resources a drift detection has
//! checked, end to end over the awsQuery wire.
//!
//! **The rule under test.** Once a drift detection has run over a stack, this read reports
//! one `StackResourceDrift` per resource that detection COMPARED, carrying the comparison
//! itself: the stack and resource identity, the expected configuration the template
//! declared, the actual one the owning service reported, the fully-qualified path of every
//! value the two disagree about with both values and the kind of difference, and the drift
//! status and the instant of that check. A resource no comparison was carried out over —
//! one nothing has checked yet, and one whose type drift detection is not supported for
//! here — is not reported at all. `StackResourceDriftStatusFilters` selects by drift status
//! and `MaxResults`/`NextToken` page the result.
//!
//! The table spans the class the rule generalises over rather than the one recorded case:
//! an untouched resource, one whose nested declared document was changed outside the stack,
//! one whose scalar property was, one deleted outside the stack and one of a type nothing
//! here provisions; one resource and several; two owning services (Amazon S3 and Amazon
//! SNS), so the difference paths that are asserted are ones each of those services really
//! reports.
//!
//! Everything goes through the one router `stack_lifecycle.rs` builds, over each service's
//! own wire — awsQuery for CloudFormation and Amazon SNS, REST for Amazon S3.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::provisioned_resources::s3;
use crate::stack_lifecycle::{
    call, deploy, describe_stack, list_members, member, router, sns, wait_for_the_detection,
    xml_text,
};
use crate::stack_resources::is_iso8601_instant;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    drift::difference_type, resource_drift_status,
};

/// One difference the report must carry: the path, the expected value, the actual value and
/// the kind of difference.
type Difference = (&'static str, &'static str, &'static str, &'static str);

/// Run a drift detection over one stack and wait past the instant its result is due.
async fn detect_drift(router: &MultiServiceRouter, stack: &str) {
    let (status, body) = call(router, "DetectStackDrift", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {stack:?}: DetectStackDrift answered {status} with {body}"
    );
    wait_for_the_detection().await;
}

/// `DescribeStackResourceDrifts` for one stack, with whatever extra members a case needs.
async fn describe_drifts(
    router: &MultiServiceRouter,
    stack: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("StackName", stack)];
    members.extend_from_slice(extra);
    call(router, "DescribeStackResourceDrifts", &members).await
}

/// The successful body of that read.
async fn drift_report(
    router: &MultiServiceRouter,
    stack: &str,
    extra: &[(String, String)],
) -> String {
    let (status, body) = describe_drifts(router, stack, extra).await;
    assert_eq!(
        status, 200,
        "case {stack:?}: DescribeStackResourceDrifts answered {status} with {body}"
    );
    body
}

/// The reported drift of each logical id in the report, in the order it was reported.
fn reported_drifts(report: &str) -> Vec<(String, String)> {
    list_members(report, "StackResourceDrifts")
        .into_iter()
        .map(|block| {
            (
                xml_text(&block, "LogicalResourceId").unwrap_or_default(),
                xml_text(&block, "StackResourceDriftStatus").unwrap_or_default(),
            )
        })
        .collect()
}

/// One logical id's whole reported record.
fn drift_of(report: &str, logical_id: &str) -> Option<String> {
    list_members(report, "StackResourceDrifts")
        .into_iter()
        .find(|block| xml_text(block, "LogicalResourceId").as_deref() == Some(logical_id))
}

/// The differences one reported record carries, as the table writes them.
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

/// The physical id the stack reports for one logical resource, which is the identity every
/// read of that resource agrees on.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResource({stack}, {logical_id}) answered {status} with {body}"
    );
    xml_text(&body, "PhysicalResourceId")
        .unwrap_or_else(|| panic!("case {logical_id:?}: expected a physical id, got {body}"))
}

/// What a row does to the stack's resources, outside the stack, before the detection runs.
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

/// What the report must carry about one resource of a row.
struct Reported {
    logical_id: &'static str,
    resource_type: &'static str,
    status: &'static str,
    /// The whole `ExpectedProperties` document, or `None` when the member must be absent.
    expected_properties: Option<Value>,
    /// Members the `ActualProperties` document must carry — a SUBSET, because what an
    /// owning service reports is its own whole view of the resource, of which only the
    /// declared properties are compared. `None` when the member must be absent.
    actual_properties: Option<Value>,
    differences: Vec<Difference>,
}

/// One row of the drift-report table.
struct ReportCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    tamper: Tamper,
    reported: Vec<Reported>,
    /// Logical ids of the stack the report must NOT carry.
    omitted: Vec<&'static str>,
}

fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            case: "a provisioned bucket nothing touched",
            stack: "drifts-in-sync",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "drifts-in-sync-store"}
                    }
                }
            })
            .to_string(),
            tamper: Tamper::Nothing,
            reported: vec![Reported {
                logical_id: "Store",
                resource_type: "AWS::S3::Bucket",
                status: resource_drift_status::IN_SYNC,
                expected_properties: Some(json!({"BucketName": "drifts-in-sync-store"})),
                actual_properties: Some(json!({})),
                differences: vec![],
            }],
            omitted: vec![],
        },
        ReportCase {
            case: "a nested declared document changed outside the stack",
            stack: "drifts-modified-document",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "drifts-modified-document-store",
                            "VersioningConfiguration": {"Status": "Enabled"}
                        }
                    }
                }
            })
            .to_string(),
            tamper: Tamper::SuspendVersioning("Store"),
            reported: vec![Reported {
                logical_id: "Store",
                resource_type: "AWS::S3::Bucket",
                status: resource_drift_status::MODIFIED,
                expected_properties: Some(json!({
                    "BucketName": "drifts-modified-document-store",
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
            }],
            omitted: vec![],
        },
        ReportCase {
            case: "a scalar property another service reports changed",
            stack: "drifts-modified-scalar",
            template: json!({
                "Resources": {
                    "Notify": {
                        "Type": "AWS::SNS::Topic",
                        "Properties": {
                            "TopicName": "drifts-modified-scalar-notify",
                            "DisplayName": "the stack's own"
                        }
                    }
                }
            })
            .to_string(),
            tamper: Tamper::RenameTopicDisplay("Notify", "changed outside the stack"),
            reported: vec![Reported {
                logical_id: "Notify",
                resource_type: "AWS::SNS::Topic",
                status: resource_drift_status::MODIFIED,
                expected_properties: Some(json!({
                    "TopicName": "drifts-modified-scalar-notify",
                    "DisplayName": "the stack's own"
                })),
                actual_properties: Some(json!({"DisplayName": "changed outside the stack"})),
                differences: vec![(
                    "/DisplayName",
                    "the stack's own",
                    "changed outside the stack",
                    difference_type::NOT_EQUAL,
                )],
            }],
            omitted: vec![],
        },
        ReportCase {
            case: "a resource deleted outside the stack carries neither configuration",
            stack: "drifts-deleted",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "drifts-deleted-store"}
                    }
                }
            })
            .to_string(),
            tamper: Tamper::DeleteBucket("Store"),
            reported: vec![Reported {
                logical_id: "Store",
                resource_type: "AWS::S3::Bucket",
                status: resource_drift_status::DELETED,
                expected_properties: None,
                actual_properties: None,
                differences: vec![],
            }],
            omitted: vec![],
        },
        ReportCase {
            case: "several resources, one deleted outside the stack and one unchecked",
            stack: "drifts-mixed",
            template: concat!(
                "Resources:\n",
                "  Gone:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: drifts-mixed-gone\n",
                "  Kept:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: drifts-mixed-kept\n",
                "  Signal:\n",
                "    Type: AWS::CloudWatch::CompositeAlarm\n",
                "    Properties:\n",
                "      AlarmName: drifts-mixed-signal\n",
                "      AlarmRule: ALARM(drifts-mixed-other)\n",
            )
            .to_string(),
            tamper: Tamper::DeleteBucket("Gone"),
            reported: vec![
                Reported {
                    logical_id: "Gone",
                    resource_type: "AWS::S3::Bucket",
                    status: resource_drift_status::DELETED,
                    expected_properties: None,
                    actual_properties: None,
                    differences: vec![],
                },
                Reported {
                    logical_id: "Kept",
                    resource_type: "AWS::S3::Bucket",
                    status: resource_drift_status::IN_SYNC,
                    expected_properties: Some(json!({"BucketName": "drifts-mixed-kept"})),
                    actual_properties: Some(json!({})),
                    differences: vec![],
                },
            ],
            // "Resources that don't currently support drift detection aren't checked, and
            // so not included": nothing here provisions a CloudWatch COMPOSITE alarm, so no
            // service reports a configuration for one.
            omitted: vec!["Signal"],
        },
    ]
}

/// Carry out a row's out-of-band change, through the owning service's own wire.
async fn tamper_with(router: &MultiServiceRouter, case: &ReportCase) {
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

/// A reported properties document as JSON.
fn document(case: &str, member_name: &str, text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("case {case:?}: expected {member_name} to be a JSON document, got {text:?}: {error}")
    })
}

#[tokio::test]
async fn a_drift_report_carries_the_comparison_each_checked_resource_was_put_through() {
    let router = router();

    for case in report_cases() {
        deploy(&router, case.stack, &case.template).await;

        // "Resources that haven't yet been checked for drift aren't included": nothing has
        // checked this stack yet, so the report is empty rather than a guess.
        let before = drift_report(&router, case.stack, &[]).await;
        assert!(
            reported_drifts(&before).is_empty(),
            "case {:?}: expected no drift records before any detection, got {:?} from {before}",
            case.case,
            reported_drifts(&before)
        );

        tamper_with(&router, &case).await;
        detect_drift(&router, case.stack).await;

        let stack_body = describe_stack(&router, case.stack).await.1;
        let stack_id = xml_text(&stack_body, "StackId").unwrap_or_default();
        let last_check = xml_text(&stack_body, "LastCheckTimestamp").unwrap_or_default();

        let report = drift_report(&router, case.stack, &[]).await;
        let reported = reported_drifts(&report);
        let expected: Vec<(String, String)> = case
            .reported
            .iter()
            .map(|resource| {
                (
                    resource.logical_id.to_string(),
                    resource.status.to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {reported:?} from {report}",
            case.case
        );
        for omitted in &case.omitted {
            assert!(
                drift_of(&report, omitted).is_none(),
                "case {:?}: expected {omitted:?} to be left out of the report, got {report}",
                case.case
            );
        }

        for resource in &case.reported {
            let block = drift_of(&report, resource.logical_id).unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a record for {:?}, got {report}",
                    case.case, resource.logical_id
                )
            });
            let physical = physical_id(&router, case.stack, resource.logical_id).await;
            for (tag, want) in [
                ("StackId", stack_id.clone()),
                ("PhysicalResourceId", physical),
                ("ResourceType", resource.resource_type.to_string()),
                ("StackResourceDriftStatus", resource.status.to_string()),
                // "Time at which CloudFormation performed drift detection on the stack
                // resource" — the instant of the check the stack reports as its last.
                ("Timestamp", last_check.clone()),
            ] {
                let got = xml_text(&block, tag);
                assert_eq!(
                    got.as_deref(),
                    Some(want.as_str()),
                    "case {:?}: expected {:?}'s <{tag}> to be {want:?}, got {got:?} from {block}",
                    case.case,
                    resource.logical_id
                );
            }
            let timestamp = xml_text(&block, "Timestamp").unwrap_or_default();
            assert!(
                is_iso8601_instant(&timestamp),
                "case {:?}: expected {:?}'s Timestamp to be an instant, got {timestamp:?}",
                case.case,
                resource.logical_id
            );

            let expected_properties = xml_text(&block, "ExpectedProperties");
            match &resource.expected_properties {
                Some(want) => {
                    let text = expected_properties.unwrap_or_else(|| {
                        panic!(
                            "case {:?}: expected {:?} to report ExpectedProperties, got {block}",
                            case.case, resource.logical_id
                        )
                    });
                    let got = document(case.case, "ExpectedProperties", &text);
                    assert_eq!(
                        &got, want,
                        "case {:?}: expected {:?}'s ExpectedProperties to be {want}, got {got}",
                        case.case, resource.logical_id
                    );
                }
                None => assert!(
                    expected_properties.is_none(),
                    "case {:?}: expected {:?} to report no ExpectedProperties, got {:?}",
                    case.case,
                    resource.logical_id,
                    expected_properties
                ),
            }

            let actual_properties = xml_text(&block, "ActualProperties");
            match &resource.actual_properties {
                Some(want) => {
                    let text = actual_properties.unwrap_or_else(|| {
                        panic!(
                            "case {:?}: expected {:?} to report ActualProperties, got {block}",
                            case.case, resource.logical_id
                        )
                    });
                    let got = document(case.case, "ActualProperties", &text);
                    for (property, value) in want.as_object().expect("a document of members") {
                        assert_eq!(
                            got.get(property),
                            Some(value),
                            "case {:?}: expected {:?}'s ActualProperties to carry \
                             {property}={value}, got {got}",
                            case.case,
                            resource.logical_id
                        );
                    }
                }
                None => assert!(
                    actual_properties.is_none(),
                    "case {:?}: expected {:?} to report no ActualProperties, got {:?}",
                    case.case,
                    resource.logical_id,
                    actual_properties
                ),
            }

            let differences = differences_of(&block);
            let want: Vec<(String, String, String, String)> = resource
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
                "case {:?}: expected {:?}'s PropertyDifferences to be {want:?}, got \
                 {differences:?} from {block}",
                case.case, resource.logical_id
            );
        }
    }
}

/// The template the filter and paging cases run against: two checked resources of different
/// drift statuses and one resource no check is carried out over.
fn filter_template() -> String {
    json!({
        "Resources": {
            "Changed": {
                "Type": "AWS::S3::Bucket",
                "Properties": {
                    "BucketName": "drifts-filtered-changed",
                    "VersioningConfiguration": {"Status": "Enabled"}
                }
            },
            "Kept": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": "drifts-filtered-kept"}
            },
            "Signal": {
                "Type": "AWS::CloudWatch::CompositeAlarm",
                "Properties": {
                    "AlarmName": "drifts-filtered-signal",
                    "AlarmRule": "ALARM(drifts-filtered-other)"
                }
            }
        }
    })
    .to_string()
}

/// The `StackResourceDriftStatusFilters` members of a request.
fn filters(statuses: &[&str]) -> Vec<(String, String)> {
    statuses
        .iter()
        .enumerate()
        .map(|(index, status)| {
            member(
                &format!("StackResourceDriftStatusFilters.member.{}", index + 1),
                status,
            )
        })
        .collect()
}

#[tokio::test]
async fn the_report_is_selected_by_drift_status_and_carried_one_page_at_a_time() {
    let router = router();
    let stack = "drifts-filtered";
    deploy(&router, stack, &filter_template()).await;

    let bucket = physical_id(&router, stack, "Changed").await;
    let body = b"<VersioningConfiguration><Status>Suspended</Status></VersioningConfiguration>";
    let (status, _, text) = s3(&router, "PUT", &format!("/{bucket}?versioning"), body).await;
    assert!(
        (200..300).contains(&status),
        "suspending versioning on {bucket:?} answered {status} with {text}"
    );
    detect_drift(&router, stack).await;

    // Each filter selects exactly the checked resources whose drift status it names, and a
    // filter for a status no reported resource carries selects none — including the status
    // of the resources the report leaves out entirely.
    for (case, statuses, expected) in [
        (
            "no filter",
            vec![],
            vec![
                ("Changed", resource_drift_status::MODIFIED),
                ("Kept", resource_drift_status::IN_SYNC),
            ],
        ),
        (
            "one status",
            vec![resource_drift_status::MODIFIED],
            vec![("Changed", resource_drift_status::MODIFIED)],
        ),
        (
            "the other status",
            vec![resource_drift_status::IN_SYNC],
            vec![("Kept", resource_drift_status::IN_SYNC)],
        ),
        (
            "both statuses",
            vec![
                resource_drift_status::IN_SYNC,
                resource_drift_status::MODIFIED,
            ],
            vec![
                ("Changed", resource_drift_status::MODIFIED),
                ("Kept", resource_drift_status::IN_SYNC),
            ],
        ),
        (
            "a status no checked resource carries",
            vec![resource_drift_status::DELETED],
            vec![],
        ),
        (
            "the status of a resource no check was carried out over",
            vec![resource_drift_status::NOT_CHECKED],
            vec![],
        ),
    ] {
        let report = drift_report(&router, stack, &filters(&statuses)).await;
        let reported = reported_drifts(&report);
        let want: Vec<(String, String)> = expected
            .iter()
            .map(|(logical_id, status)| ((*logical_id).to_string(), (*status).to_string()))
            .collect();
        assert_eq!(
            reported, want,
            "case {case:?}: expected {want:?}, got {reported:?} from {report}"
        );
    }

    // "The maximum number of results to be returned with a single call. If the number of
    // available results exceeds this maximum, the response includes a NextToken value that
    // you can assign to the NextToken request parameter to get the next set of results."
    let first = drift_report(&router, stack, &[member("MaxResults", "1")]).await;
    assert_eq!(
        reported_drifts(&first),
        vec![(
            "Changed".to_string(),
            resource_drift_status::MODIFIED.to_string()
        )],
        "case {:?}: expected the first page to carry one record, got {first}",
        "one record per page"
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case {:?}: expected a NextToken, got {first}", "one record per page")
    });

    let second = drift_report(
        &router,
        stack,
        &[member("MaxResults", "1"), member("NextToken", &token)],
    )
    .await;
    assert_eq!(
        reported_drifts(&second),
        vec![(
            "Kept".to_string(),
            resource_drift_status::IN_SYNC.to_string()
        )],
        "case {:?}: expected the token to resume just past the first page, got {second}",
        "the second page"
    );
    assert_eq!(
        xml_text(&second, "NextToken"),
        None,
        "case {:?}: expected no token past the last record, got {second}",
        "the second page"
    );

    // A token this read never issued is a bad request, not an empty page.
    let (status, body) = describe_drifts(&router, stack, &[member("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "case {:?}: expected a refusal, got {status} with {body}",
        "a token from nowhere"
    );
}

#[tokio::test]
async fn a_stack_the_region_does_not_carry_is_refused() {
    let router = router();
    for (case, stack) in [
        ("a name nothing created", "drifts-no-such-stack"),
        (
            "an id nothing minted",
            "arn:aws:cloudformation:us-east-1:000000000000:stack/drifts-none/00000000-0000-4000-8000-000000000000",
        ),
    ] {
        let (status, body) = describe_drifts(&router, stack, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {body}"
        );
        for fragment in [
            "ValidationError",
            &format!("Stack [{stack}] does not exist"),
        ] {
            assert!(
                body.contains(fragment),
                "case {case:?}: expected {fragment:?} in the refusal, got {body}"
            );
        }
    }
}
