// SPDX-License-Identifier: CC0-1.0

//! Drift detection, the resource read paths' agreement on a physical id, and what a
//! registry record reports about an extension version — end to end over the wire.
//!
//! Three rules are under test here, each with its own table.
//!
//! 1. **Drift detection reports what the owning service holds.** Once a drift detection on
//!    a stack has completed, the stack reports the detected result as
//!    `DriftInformation.StackDriftStatus` with `LastCheckTimestamp` set to the instant the
//!    detection was initiated, and each resource the detection covered reports its own
//!    result — a resource the service that owns it no longer holds is `DELETED`, one whose
//!    declared configuration that service reports differently is `MODIFIED`, one it reports
//!    unchanged is `IN_SYNC`, and one no service here provisions is `NOT_CHECKED`. A stack
//!    with a drifted resource is `DRIFTED`; one with none is `IN_SYNC`.
//!
//! 2. **One physical id per logical resource.** Every read path of a stack's resources
//!    reports the SAME physical id for one logical id — `DescribeStackResources`,
//!    `DescribeStackResource` and `ListStackResources`, and the stack output a `Ref` to
//!    that resource resolves to.
//!
//! 3. **A registry record says when its version entered the registry and where its source
//!    is.** `DescribeType` reports `TimeCreated`, `LastUpdated` and `SourceUrl` for a
//!    registered extension version, the instants stable across reads.
//!
//! Each table spans the class its rule generalises over rather than one recorded case: both
//! template spellings, a resource type an owning service here creates and one it does not,
//! named and generated physical ids, one resource and several, an untouched resource and
//! one changed or deleted outside the stack, and several of the published extension types.
//!
//! Everything goes over the wire each service speaks — awsQuery for CloudFormation, REST
//! for Amazon S3 — through the one router `stack_lifecycle.rs` builds.

use std::time::Instant;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::provisioned_resources::s3;
use crate::stack_lifecycle::{
    call, deploy, describe_stack, member, router, wait_for_the_detection, xml_text, ACCOUNT_ID,
    REGION,
};
use crate::stack_resources::is_iso8601_instant;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    drift_detection_status, resource_drift_status, DRIFT_DETECTION_DELAY, DRIFT_STATUS_DRIFTED,
    DRIFT_STATUS_IN_SYNC, DRIFT_STATUS_NOT_CHECKED,
};

/// Start a drift detection on one stack, optionally filtered to some logical ids, and
/// answer the `StackDriftDetectionId` it hands back.
async fn detect_drift(
    router: &MultiServiceRouter,
    stack: &str,
    logical_ids: &[&str],
) -> (u16, String) {
    let mut members = vec![member("StackName", stack)];
    for (index, logical_id) in logical_ids.iter().enumerate() {
        members.push(member(
            &format!("LogicalResourceIds.member.{}", index + 1),
            logical_id,
        ));
    }
    call(router, "DetectStackDrift", &members).await
}

/// The detection id a successful `DetectStackDrift` answered with.
async fn detection_id(router: &MultiServiceRouter, stack: &str, logical_ids: &[&str]) -> String {
    let (status, body) = detect_drift(router, stack, logical_ids).await;
    assert_eq!(
        status, 200,
        "DetectStackDrift({stack}) answered {status} with {body}"
    );
    xml_text(&body, "StackDriftDetectionId")
        .unwrap_or_else(|| panic!("case {stack:?}: expected a detection id, got {body}"))
}

async fn detection_status(router: &MultiServiceRouter, detection_id: &str) -> (u16, String) {
    call(
        router,
        "DescribeStackDriftDetectionStatus",
        &[member("StackDriftDetectionId", detection_id)],
    )
    .await
}

/// Each `<member>` block of one list element of a response, in wire order.
///
/// The first member of a list carries the serializer's `xmlns` attribute
/// (`<member xmlns="…">`), so the open tag is matched by prefix rather than literally.
fn member_blocks(xml: &str, container: &str) -> Vec<String> {
    let open = format!("<{container}>");
    let Some(start) = xml.find(&open) else {
        return Vec::new();
    };
    let after = &xml[start + open.len()..];
    let end = after
        .find(&format!("</{container}>"))
        .unwrap_or(after.len());
    let block = &after[..end];

    let mut blocks = Vec::new();
    let mut rest = block;
    while let Some(open) = rest.find("<member") {
        let after_tag = &rest[open + "<member".len()..];
        let Some(tag_end) = after_tag.find('>') else {
            break;
        };
        let after_open = &after_tag[tag_end + 1..];
        let Some(close) = after_open.find("</member>") else {
            break;
        };
        blocks.push(after_open[..close].to_string());
        rest = &after_open[close + "</member>".len()..];
    }
    blocks
}

/// One member's `<tag>` text, keyed by the logical id the member carries.
fn by_logical_id(xml: &str, container: &str, tag: &str) -> Vec<(String, Option<String>)> {
    member_blocks(xml, container)
        .into_iter()
        .map(|block| {
            (
                xml_text(&block, "LogicalResourceId").unwrap_or_default(),
                xml_text(&block, tag),
            )
        })
        .collect()
}

/// What `DescribeStackResources` reports under `tag` for each logical id of a stack.
async fn described_resources(
    router: &MultiServiceRouter,
    stack: &str,
    tag: &str,
) -> Vec<(String, Option<String>)> {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResources({stack}) answered {status} with {body}"
    );
    by_logical_id(&body, "StackResources", tag)
}

/// What `ListStackResources` reports under `tag` for each logical id of a stack.
async fn listed_resources(
    router: &MultiServiceRouter,
    stack: &str,
    tag: &str,
) -> Vec<(String, Option<String>)> {
    let (status, body) = call(
        router,
        "ListStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "ListStackResources({stack}) answered {status} with {body}"
    );
    by_logical_id(&body, "StackResourceSummaries", tag)
}

/// What `DescribeStackResource` reports under `tag` for one logical id.
async fn described_resource(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
    tag: &str,
) -> Option<String> {
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
    xml_text(&body, tag)
}

/// The stack output `key` resolved to.
async fn stack_output(router: &MultiServiceRouter, stack: &str, key: &str) -> Option<String> {
    let (status, body) = describe_stack(router, stack).await;
    assert_eq!(
        status, 200,
        "DescribeStacks({stack}) answered {status} with {body}"
    );
    member_blocks(&body, "Outputs")
        .into_iter()
        .find(|block| xml_text(block, "OutputKey").as_deref() == Some(key))
        .and_then(|block| xml_text(&block, "OutputValue"))
}

// ---------------------------------------------------------------------------
// 1. Drift detection
// ---------------------------------------------------------------------------

/// What a row does to the stack's resources, outside the stack, before the detection runs.
#[derive(Debug, Clone, Copy)]
enum Tamper {
    /// Nothing: the resources are as the deployment left them.
    Nothing,
    /// Delete the resource of this logical id in Amazon S3.
    DeleteBucket(&'static str),
    /// Suspend the versioning the template declared for this bucket.
    SuspendVersioning(&'static str),
}

/// One row of the drift table.
struct DriftCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    tamper: Tamper,
    /// The drift status each logical id must report, by logical id.
    resources: Vec<(&'static str, &'static str)>,
    stack_status: &'static str,
    drifted: &'static str,
}

fn drift_cases() -> Vec<DriftCase> {
    vec![
        DriftCase {
            case: "a provisioned bucket nothing touched",
            stack: "drift-in-sync",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "drift-in-sync-store"}
                    }
                }
            })
            .to_string(),
            tamper: Tamper::Nothing,
            resources: vec![("Store", resource_drift_status::IN_SYNC)],
            stack_status: DRIFT_STATUS_IN_SYNC,
            drifted: "0",
        },
        DriftCase {
            case: "the bucket was deleted outside the stack",
            stack: "drift-deleted",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "drift-deleted-store"}
                    }
                }
            })
            .to_string(),
            tamper: Tamper::DeleteBucket("Store"),
            resources: vec![("Store", resource_drift_status::DELETED)],
            stack_status: DRIFT_STATUS_DRIFTED,
            drifted: "1",
        },
        DriftCase {
            case: "the versioning the template declared was changed outside the stack",
            stack: "drift-modified",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "drift-modified-store",
                            "VersioningConfiguration": {"Status": "Enabled"}
                        }
                    }
                }
            })
            .to_string(),
            tamper: Tamper::SuspendVersioning("Store"),
            resources: vec![("Store", resource_drift_status::MODIFIED)],
            stack_status: DRIFT_STATUS_DRIFTED,
            drifted: "1",
        },
        DriftCase {
            case: "a resource type no service mounted here provisions",
            stack: "drift-unprovisioned",
            template: concat!(
                "Resources:\n",
                "  Signal:\n",
                "    Type: AWS::CloudWatch::CompositeAlarm\n",
                "    Properties:\n",
                "      AlarmName: drift-unprovisioned-signal\n",
                "      AlarmRule: ALARM(drift-unprovisioned-other)\n",
            )
            .to_string(),
            tamper: Tamper::Nothing,
            resources: vec![("Signal", resource_drift_status::NOT_CHECKED)],
            stack_status: DRIFT_STATUS_IN_SYNC,
            drifted: "0",
        },
        DriftCase {
            case: "several resources, one of them deleted outside the stack",
            stack: "drift-mixed",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "drift-mixed-store"}
                    },
                    "Kept": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "drift-mixed-kept"}
                    },
                    "Signal": {"Type": "AWS::CloudWatch::CompositeAlarm"}
                }
            })
            .to_string(),
            tamper: Tamper::DeleteBucket("Store"),
            resources: vec![
                ("Kept", resource_drift_status::IN_SYNC),
                ("Signal", resource_drift_status::NOT_CHECKED),
                ("Store", resource_drift_status::DELETED),
            ],
            stack_status: DRIFT_STATUS_DRIFTED,
            drifted: "1",
        },
        DriftCase {
            case: "a role another service owns, with the properties it reports back",
            stack: "drift-role",
            template: json!({
                "Resources": {
                    "Runner": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": "drift-role-runner",
                            "Description": "the stack's own runner",
                            "MaxSessionDuration": 7200,
                            "AssumeRolePolicyDocument": {
                                "Version": "2012-10-17",
                                "Statement": [{
                                    "Effect": "Allow",
                                    "Principal": {"Service": "ecs-tasks.amazonaws.com"},
                                    "Action": "sts:AssumeRole"
                                }]
                            }
                        }
                    }
                }
            })
            .to_string(),
            tamper: Tamper::Nothing,
            resources: vec![("Runner", resource_drift_status::IN_SYNC)],
            stack_status: DRIFT_STATUS_IN_SYNC,
            drifted: "0",
        },
    ]
}

/// Carry out a row's out-of-band change, through the owning service's own wire.
async fn tamper_with(router: &MultiServiceRouter, case: &DriftCase) {
    async fn bucket_of(
        router: &MultiServiceRouter,
        case: &DriftCase,
        logical_id: &str,
    ) -> String {
        described_resource(router, case.stack, logical_id, "PhysicalResourceId")
            .await
            .unwrap_or_else(|| panic!("case {:?}: expected a physical id", case.case))
    }
    match case.tamper {
        Tamper::Nothing => {}
        Tamper::DeleteBucket(logical_id) => {
            let bucket = bucket_of(router, case, logical_id).await;
            let (status, _, body) = s3(router, "DELETE", &format!("/{bucket}"), b"").await;
            assert!(
                (200..300).contains(&status),
                "case {:?}: deleting {bucket:?} outside the stack answered {status} with \
                 {body}",
                case.case
            );
        }
        Tamper::SuspendVersioning(logical_id) => {
            let bucket = bucket_of(router, case, logical_id).await;
            let body = b"<VersioningConfiguration><Status>Suspended</Status>\
                         </VersioningConfiguration>";
            let (status, _, text) =
                s3(router, "PUT", &format!("/{bucket}?versioning"), body).await;
            assert!(
                (200..300).contains(&status),
                "case {:?}: suspending versioning on {bucket:?} answered {status} with \
                 {text}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_completed_drift_detection_reports_what_the_owning_services_hold() {
    let router = router();

    for case in drift_cases() {
        deploy(&router, case.stack, &case.template).await;
        tamper_with(&router, &case).await;

        let detection_id = detection_id(&router, case.stack, &[]).await;
        assert_eq!(
            detection_id.len(),
            36,
            "case {:?}: expected a 36-character detection id, got {detection_id:?}",
            case.case
        );
        wait_for_the_detection().await;

        let (status, body) = detection_status(&router, &detection_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeStackDriftDetectionStatus answered {status} with {body}",
            case.case
        );
        for (tag, expected) in [
            (
                "DetectionStatus",
                drift_detection_status::DETECTION_COMPLETE.to_string(),
            ),
            ("StackDriftStatus", case.stack_status.to_string()),
            ("DriftedStackResourceCount", case.drifted.to_string()),
            ("StackDriftDetectionId", detection_id.clone()),
        ] {
            let actual = xml_text(&body, tag);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {:?}: expected <{tag}> {expected:?}, got {actual:?} from {body}",
                case.case
            );
        }
        let initiated = xml_text(&body, "Timestamp").unwrap_or_default();
        assert!(
            is_iso8601_instant(&initiated),
            "case {:?}: expected the instant the detection was initiated, got \
             {initiated:?}",
            case.case
        );

        // The stack reports the result, with the instant the detection was initiated.
        let (status, stack_body) = describe_stack(&router, case.stack).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeStacks answered {status} with {stack_body}",
            case.case
        );
        let stack_status = xml_text(&stack_body, "StackDriftStatus");
        assert_eq!(
            stack_status.as_deref(),
            Some(case.stack_status),
            "case {:?}: expected the stack to report {:?}, got {stack_status:?} from \
             {stack_body}",
            case.case,
            case.stack_status
        );
        let last_check = xml_text(&stack_body, "LastCheckTimestamp").unwrap_or_default();
        assert_eq!(
            last_check, initiated,
            "case {:?}: expected LastCheckTimestamp to be the instant the detection was \
             initiated ({initiated:?}), got {last_check:?}",
            case.case
        );

        // `ListStacks` reports the same result for the same stack, in the summary
        // spelling of the same two members.
        let (status, listed) = call(&router, "ListStacks", &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListStacks answered {status} with {listed}",
            case.case
        );
        let summary = member_blocks(&listed, "StackSummaries")
            .into_iter()
            .find(|block| xml_text(block, "StackName").as_deref() == Some(case.stack))
            .unwrap_or_else(|| {
                panic!("case {:?}: expected the stack in ListStacks, got {listed}", case.case)
            });
        for (tag, expected) in [
            ("StackDriftStatus", case.stack_status.to_string()),
            ("LastCheckTimestamp", initiated.clone()),
        ] {
            let actual = xml_text(&summary, tag);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {:?}: expected ListStacks to report <{tag}> {expected:?}, got \
                 {actual:?}",
                case.case
            );
        }

        // Every resource read path reports the same per-resource result.
        let expected: Vec<(String, Option<String>)> = case
            .resources
            .iter()
            .map(|(logical_id, status)| ((*logical_id).to_string(), Some((*status).to_string())))
            .collect();
        for (path, actual) in [
            (
                "DescribeStackResources",
                described_resources(&router, case.stack, "StackResourceDriftStatus").await,
            ),
            (
                "ListStackResources",
                listed_resources(&router, case.stack, "StackResourceDriftStatus").await,
            ),
        ] {
            assert_eq!(
                actual, expected,
                "case {:?}: expected {path} to report {expected:?}, got {actual:?}",
                case.case
            );
        }
        for (logical_id, status) in &case.resources {
            let actual =
                described_resource(&router, case.stack, logical_id, "StackResourceDriftStatus")
                    .await;
            assert_eq!(
                actual.as_deref(),
                Some(*status),
                "case {:?}: expected DescribeStackResource({logical_id}) to report \
                 {status:?}, got {actual:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_detections_own_status_is_in_progress_until_it_concludes() {
    let router = router();
    let stack = "drift-in-progress";
    deploy(
        &router,
        stack,
        &json!({
            "Resources": {
                "Store": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {"BucketName": "drift-in-progress-store"}
                }
            }
        })
        .to_string(),
    )
    .await;

    let started = Instant::now();
    let detection_id = detection_id(&router, stack, &[]).await;
    let (status, body) = detection_status(&router, &detection_id).await;
    assert_eq!(
        status, 200,
        "DescribeStackDriftDetectionStatus answered {status} with {body}"
    );
    // The conclusion is owed only once the delay has passed; a read that lost the race to
    // the clock is held to the concluded answer instead.
    if started.elapsed() < DRIFT_DETECTION_DELAY {
        let actual = xml_text(&body, "DetectionStatus");
        assert_eq!(
            actual.as_deref(),
            Some(drift_detection_status::DETECTION_IN_PROGRESS),
            "expected a detection just started to be in progress, got {actual:?} from {body}"
        );
        // "This value will be NULL for any stack drift detection operation that has not
        // yet concluded", as will the drifted count.
        for tag in ["StackDriftStatus", "DriftedStackResourceCount"] {
            let actual = xml_text(&body, tag);
            assert_eq!(
                actual, None,
                "expected no <{tag}> before the detection concluded, got {actual:?}"
            );
        }
    }

    // The stack reports what the check found, as of the instant the detection was
    // initiated — the check against the owning service is what the call carried out.
    let (_, stack_body) = describe_stack(&router, stack).await;
    let actual = xml_text(&stack_body, "StackDriftStatus");
    assert_eq!(
        actual.as_deref(),
        Some(DRIFT_STATUS_IN_SYNC),
        "expected the stack to report what the check found, got {actual:?} from {stack_body}"
    );
    let last_check = xml_text(&stack_body, "LastCheckTimestamp").unwrap_or_default();
    assert!(
        is_iso8601_instant(&last_check),
        "expected the instant the detection was initiated, got {last_check:?}"
    );

    wait_for_the_detection().await;
    let (_, body) = detection_status(&router, &detection_id).await;
    let actual = xml_text(&body, "DetectionStatus");
    assert_eq!(
        actual.as_deref(),
        Some(drift_detection_status::DETECTION_COMPLETE),
        "expected the detection to have concluded, got {actual:?} from {body}"
    );
}

/// A stack nothing has checked reports `NOT_CHECKED` with no check timestamp — the result
/// a detection replaces.
#[tokio::test]
async fn a_stack_no_detection_has_run_against_reports_not_checked() {
    let router = router();
    let stack = "drift-unchecked";
    deploy(
        &router,
        stack,
        &json!({
            "Resources": {
                "Store": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {"BucketName": "drift-unchecked-store"}
                }
            }
        })
        .to_string(),
    )
    .await;

    let (_, body) = describe_stack(&router, stack).await;
    let actual = xml_text(&body, "StackDriftStatus");
    assert_eq!(
        actual.as_deref(),
        Some(DRIFT_STATUS_NOT_CHECKED),
        "expected an unchecked stack to report NOT_CHECKED, got {actual:?} from {body}"
    );
    let last_check = xml_text(&body, "LastCheckTimestamp");
    assert_eq!(
        last_check, None,
        "expected no check timestamp on a stack nothing has checked, got {last_check:?}"
    );
    for (path, reported) in [
        (
            "DescribeStackResources",
            described_resources(&router, stack, "StackResourceDriftStatus").await,
        ),
        (
            "ListStackResources",
            listed_resources(&router, stack, "StackResourceDriftStatus").await,
        ),
    ] {
        let expected: Vec<(String, Option<String>)> = vec![(
            "Store".to_string(),
            Some(resource_drift_status::NOT_CHECKED.to_string()),
        )];
        assert_eq!(
            reported, expected,
            "expected {path} to report an unchecked resource as NOT_CHECKED, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn logical_resource_ids_filter_what_a_detection_covers() {
    let router = router();
    let stack = "drift-filtered";
    deploy(
        &router,
        stack,
        &json!({
            "Resources": {
                "Covered": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {"BucketName": "drift-filtered-covered"}
                },
                "Ignored": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {"BucketName": "drift-filtered-ignored"}
                }
            }
        })
        .to_string(),
    )
    .await;
    // The resource OUTSIDE the filter is the one that was deleted, so a detection that
    // honours the filter finds nothing drifted.
    let bucket = described_resource(&router, stack, "Ignored", "PhysicalResourceId")
        .await
        .expect("a physical id");
    let (status, _, body) = s3(&router, "DELETE", &format!("/{bucket}"), b"").await;
    assert!(
        (200..300).contains(&status),
        "deleting {bucket:?} answered {status} with {body}"
    );

    let detection_id = detection_id(&router, stack, &["Covered"]).await;
    wait_for_the_detection().await;
    let (_, body) = detection_status(&router, &detection_id).await;
    for (tag, expected) in [
        ("StackDriftStatus", DRIFT_STATUS_IN_SYNC),
        ("DriftedStackResourceCount", "0"),
    ] {
        let actual = xml_text(&body, tag);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "expected <{tag}> {expected:?} for a detection filtered to the untouched \
             resource, got {actual:?} from {body}"
        );
    }
    let reported = described_resources(&router, stack, "StackResourceDriftStatus").await;
    let expected: Vec<(String, Option<String>)> = vec![
        (
            "Covered".to_string(),
            Some(resource_drift_status::IN_SYNC.to_string()),
        ),
        (
            "Ignored".to_string(),
            Some(resource_drift_status::NOT_CHECKED.to_string()),
        ),
    ];
    assert_eq!(
        reported, expected,
        "expected only the covered resource to carry a result, got {reported:?}"
    );
}

#[tokio::test]
async fn a_detection_request_that_names_nothing_addressable_is_refused() {
    let router = router();
    let stack = "drift-refusals";
    deploy(
        &router,
        stack,
        &json!({
            "Resources": {
                "Store": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {"BucketName": "drift-refusals-store"}
                }
            }
        })
        .to_string(),
    )
    .await;

    // A stack no live stack carries is not addressable, so it is a `ValidationError`
    // rather than an empty result.
    let (status, body) = detect_drift(&router, "not-a-stack", &[]).await;
    assert_eq!(
        status, 400,
        "DetectStackDrift on an unknown stack answered {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "expected a ValidationError, got {body}"
    );

    // A logical id the stack has not deployed is a filter that selects nothing to check,
    // not a refusal: this operation models no error, and nothing documents one for it.
    let detection_id = detection_id(&router, stack, &["NotDeclared"]).await;
    wait_for_the_detection().await;
    let (status, body) = detection_status(&router, &detection_id).await;
    assert_eq!(
        status, 200,
        "DescribeStackDriftDetectionStatus answered {status} with {body}"
    );
    for (tag, expected) in [
        ("StackDriftStatus", DRIFT_STATUS_IN_SYNC),
        ("DriftedStackResourceCount", "0"),
    ] {
        let actual = xml_text(&body, tag);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "expected <{tag}> {expected:?} for a detection that covered no resource, got \
             {actual:?} from {body}"
        );
    }
    let reported = described_resources(&router, stack, "StackResourceDriftStatus").await;
    let expected: Vec<(String, Option<String>)> = vec![(
        "Store".to_string(),
        Some(resource_drift_status::NOT_CHECKED.to_string()),
    )];
    assert_eq!(
        reported, expected,
        "expected a filter that named no deployed resource to check nothing, got \
         {reported:?}"
    );

    let (status, body) = detection_status(&router, "00000000-0000-0000-0000-000000000000").await;
    assert_eq!(
        status, 400,
        "DescribeStackDriftDetectionStatus on an unknown id answered {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "expected a ValidationError, got {body}"
    );
}

// ---------------------------------------------------------------------------
// 2. One physical id per logical resource
// ---------------------------------------------------------------------------

/// One row of the physical-id agreement table: a template, and the logical ids whose
/// physical id every read path must agree on. Each row declares one output per resource,
/// `Ref`-ing it, so the stack's own hand-off is compared with the resource reads.
struct IdentityCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    logical_ids: Vec<&'static str>,
}

fn identity_cases() -> Vec<IdentityCase> {
    vec![
        IdentityCase {
            case: "a bucket the template names",
            stack: "ids-named",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "ids-named-store"}
                    }
                },
                "Outputs": {"Store": {"Value": {"Ref": "Store"}}}
            })
            .to_string(),
            logical_ids: vec!["Store"],
        },
        IdentityCase {
            case: "a bucket the template does not name, so CloudFormation generated its id",
            stack: "ids-generated",
            template: json!({
                "Resources": {"Store": {"Type": "AWS::S3::Bucket"}},
                "Outputs": {"Store": {"Value": {"Ref": "Store"}}}
            })
            .to_string(),
            logical_ids: vec!["Store"],
        },
        IdentityCase {
            case: "a type whose Ref is an ARN rather than a name",
            stack: "ids-arn",
            template: concat!(
                "Resources:\n",
                "  Signal:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: ids-arn-signal\n",
                "Outputs:\n",
                "  Signal:\n",
                "    Value: !Ref Signal\n",
            )
            .to_string(),
            logical_ids: vec!["Signal"],
        },
        IdentityCase {
            case: "several resources of several types in one stack",
            stack: "ids-several",
            template: json!({
                "Resources": {
                    "Store": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "ids-several-store"}
                    },
                    "Signal": {"Type": "AWS::SNS::Topic"},
                    "Log": {"Type": "AWS::Logs::LogGroup"}
                },
                "Outputs": {
                    "Store": {"Value": {"Ref": "Store"}},
                    "Signal": {"Value": {"Ref": "Signal"}},
                    "Log": {"Value": {"Ref": "Log"}}
                }
            })
            .to_string(),
            logical_ids: vec!["Log", "Signal", "Store"],
        },
    ]
}

#[tokio::test]
async fn every_resource_read_path_reports_one_physical_id_per_logical_resource() {
    let router = router();

    for case in identity_cases() {
        deploy(&router, case.stack, &case.template).await;

        let described = described_resources(&router, case.stack, "PhysicalResourceId").await;
        let listed = listed_resources(&router, case.stack, "PhysicalResourceId").await;
        assert_eq!(
            listed, described,
            "case {:?}: expected ListStackResources to report the same ids as \
             DescribeStackResources ({described:?}), got {listed:?}",
            case.case
        );
        let reported_ids: Vec<String> = described.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(
            reported_ids, case.logical_ids,
            "case {:?}: expected the stack's logical ids {:?}, got {reported_ids:?}",
            case.case, case.logical_ids
        );

        for (logical_id, physical_id) in &described {
            let physical_id = physical_id.clone().unwrap_or_default();
            assert!(
                !physical_id.is_empty(),
                "case {:?}: expected a physical id for {logical_id:?}, got none",
                case.case
            );
            let detail =
                described_resource(&router, case.stack, logical_id, "PhysicalResourceId").await;
            assert_eq!(
                detail.as_deref(),
                Some(physical_id.as_str()),
                "case {:?}: expected DescribeStackResource({logical_id}) to report \
                 {physical_id:?}, got {detail:?}",
                case.case
            );
            let output = stack_output(&router, case.stack, logical_id).await;
            assert_eq!(
                output.as_deref(),
                Some(physical_id.as_str()),
                "case {:?}: expected the stack output Ref-ing {logical_id:?} to resolve to \
                 {physical_id:?}, got {output:?}",
                case.case
            );
            // Addressing the stack by one of its resources' physical ids selects that same
            // stack, which is the other way this id is used.
            let (status, body) = call(
                &router,
                "DescribeStackResources",
                &[member("PhysicalResourceId", &physical_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: selecting the stack by {physical_id:?} answered {status} with \
                 {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackName").as_deref(),
                Some(case.stack),
                "case {:?}: expected {physical_id:?} to select {:?}, got {body}",
                case.case,
                case.stack
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. What a registry record reports about an extension version
// ---------------------------------------------------------------------------

async fn describe_type(router: &MultiServiceRouter, type_name: &str) -> (u16, String) {
    call(
        router,
        "DescribeType",
        &[member("Type", "RESOURCE"), member("TypeName", type_name)],
    )
    .await
}

/// The repository AWS publishes a resource type's provider in.
///
/// Source: AWS's CloudFormation resource providers are open source, one repository per
/// service named after the type's own service segment —
/// `aws-cloudformation-resource-providers-<service>` — and `SourceUrl` is that repository's
/// CLONE url, so it ends in the `.git` a caller hands a git client. Derived here the same
/// way the registry derives it, so the row for a type the issue never cited is covered too.
fn expected_source_url(type_name: &str) -> String {
    let service = type_name.split("::").nth(1).unwrap_or_default();
    format!(
        "https://github.com/aws-cloudformation/aws-cloudformation-resource-providers-{}.git",
        service.to_ascii_lowercase()
    )
}

#[tokio::test]
async fn a_published_extension_version_reports_its_instants_and_its_source() {
    let router = router();

    // Several of the published resource types, spanning the services they belong to —
    // including types the issue never cited.
    for type_name in [
        "AWS::S3::Bucket",
        "AWS::SNS::Topic",
        "AWS::SQS::Queue",
        "AWS::Logs::LogGroup",
        "AWS::DynamoDB::Table",
        "AWS::ElasticLoadBalancingV2::Listener",
    ] {
        let (status, body) = describe_type(&router, type_name).await;
        assert_eq!(
            status, 200,
            "case {type_name:?}: DescribeType answered {status} with {body}"
        );
        let source_url = xml_text(&body, "SourceUrl");
        assert_eq!(
            source_url.as_deref(),
            Some(expected_source_url(type_name).as_str()),
            "case {type_name:?}: expected the type's source repository, got {source_url:?}"
        );
        let created = xml_text(&body, "TimeCreated").unwrap_or_default();
        let updated = xml_text(&body, "LastUpdated").unwrap_or_default();
        for (tag, value) in [("TimeCreated", &created), ("LastUpdated", &updated)] {
            assert!(
                is_iso8601_instant(value),
                "case {type_name:?}: expected <{tag}> to be a shaped instant, got {value:?}"
            );
        }

        // A read reports the same instants to the next caller: nothing about the record
        // changed because it was read.
        let (_, again) = describe_type(&router, type_name).await;
        for (tag, first) in [("TimeCreated", &created), ("LastUpdated", &updated)] {
            let second = xml_text(&again, tag).unwrap_or_default();
            assert_eq!(
                &second, first,
                "case {type_name:?}: expected <{tag}> to be stable across reads, got \
                 {first:?} then {second:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_arn_this_emulator_mints_names_the_account_and_region_it_was_signed_for() {
    // A guard on the two constants the tables above build expected ids from, so a change
    // to either is a failure here rather than a silent re-interpretation of every row.
    let router = router();
    let stack = "ids-arn-shape";
    deploy(
        &router,
        stack,
        concat!(
            "Resources:\n",
            "  Signal:\n",
            "    Type: AWS::SNS::Topic\n",
            "    Properties:\n",
            "      TopicName: ids-arn-shape-signal\n",
        ),
    )
    .await;

    let physical_id = described_resource(&router, stack, "Signal", "PhysicalResourceId")
        .await
        .unwrap_or_default();
    let expected = format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:ids-arn-shape-signal");
    assert_eq!(
        physical_id, expected,
        "expected the topic's documented Ref value {expected:?}, got {physical_id:?}"
    );
}
