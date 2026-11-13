// SPDX-License-Identifier: CC0-1.0

//! `DescribeStackResource`, end to end over the awsQuery wire.
//!
//! The rule under test: `DescribeStackResource` reports the ONE resource of the named
//! stack's last completed deployment that carries the requested `LogicalResourceId`, as a
//! `StackResourceDetail` whose every member is read back from what the stack operation
//! that deployed it persisted — the stack's own name and id, the logical id and type the
//! template declared, the physical id CloudFormation's documented naming rules gave it,
//! the content of the `Metadata` resource attribute the declaration carried, and the
//! resource-level status that operation left the resource in with the instant it reached
//! that status. A `StackName` that resolves to no stack, and a `LogicalResourceId` the
//! resolved stack has not deployed, are each refused with HTTP 400 `ValidationError`.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client calling `DescribeStackResource` sees. The table spans the class the rule
//! generalises over: both template spellings, resource types the return-value catalogue
//! carries and one it does not, named and generated physical ids, declarations with and
//! without `Metadata` and `Metadata` of each JSON form (object, nested object, array,
//! number, string), the declaration order of a stack's resources and their count, and
//! selection by stack name as well as by stack id.

use std::time::Instant;

use crate::stack_lifecycle::{
    call, create_stack, member, router, wait_for_the_transition, xml_text,
};
use crate::stack_resources::{
    assert_physical_id, element, is_iso8601_instant, sns_arn, PhysicalId,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_status, STATUS_TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `StackResourceDetail` a response carries, member by member.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireDetail {
    stack_name: Option<String>,
    stack_id: Option<String>,
    logical_id: Option<String>,
    physical_id: Option<String>,
    resource_type: Option<String>,
    status: Option<String>,
    status_reason: Option<String>,
    last_updated_timestamp: Option<String>,
    description: Option<String>,
    metadata: Option<String>,
    drift_status: Option<String>,
    drift_last_check: Option<String>,
    /// Whether the response carries a `ModuleInfo` element at all.
    has_module_info: bool,
}

/// The one `StackResourceDetail` block of a response, or `None` when it carries none.
fn wire_detail(xml: &str) -> Option<WireDetail> {
    // The serializer puts its `xmlns` attribute on the structure's own tag
    // (`<StackResourceDetail xmlns="…">`), so the open tag is matched by prefix rather
    // than literally.
    let start = xml.find("<StackResourceDetail")?;
    let after_tag = &xml[start + "<StackResourceDetail".len()..];
    let tag_end = after_tag.find('>')?;
    let after = &after_tag[tag_end + 1..];
    let end = after.find("</StackResourceDetail>")?;
    let block = &after[..end];
    Some(WireDetail {
        stack_name: element(block, "StackName"),
        stack_id: element(block, "StackId"),
        logical_id: element(block, "LogicalResourceId"),
        physical_id: element(block, "PhysicalResourceId"),
        resource_type: element(block, "ResourceType"),
        status: element(block, "ResourceStatus"),
        status_reason: element(block, "ResourceStatusReason"),
        last_updated_timestamp: element(block, "LastUpdatedTimestamp"),
        description: element(block, "Description"),
        metadata: element(block, "Metadata"),
        drift_status: element(block, "StackResourceDriftStatus"),
        drift_last_check: element(block, "LastCheckTimestamp"),
        has_module_info: block.contains("<ModuleInfo"),
    })
}

/// The detail block a successful response must carry.
fn detail_of(case: &str, status: u16, body: &str) -> WireDetail {
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackResource to succeed, got {status} {body}"
    );
    wire_detail(body).unwrap_or_else(|| {
        panic!("case {case:?}: expected a StackResourceDetail, got {body}");
    })
}

async fn describe_stack_resource(
    router: &MultiServiceRouter,
    name_or_id: &str,
    logical_resource_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", name_or_id),
            member("LogicalResourceId", logical_resource_id),
        ],
    )
    .await
}

/// One `Metadata` value as JSON, so two spellings of the same content compare equal
/// whatever order the members are rendered in.
fn as_json(case: &str, raw: &str) -> ::serde_json::Value {
    ::serde_json::from_str(raw).unwrap_or_else(|error| {
        panic!("case {case:?}: expected Metadata to be JSON, got {raw:?} ({error})")
    })
}

/// Assert a reported `Metadata` against what the declaration carried.
///
/// The member is reported for EVERY resource: a declaration stating no `Metadata` has an
/// EMPTY document rather than no document, so `None` here expects an empty JSON document
/// on the wire rather than an absent element.
fn assert_metadata(case: &str, expected: Option<&str>, actual: Option<&str>) {
    let expected = expected.unwrap_or("{}");
    let Some(actual) = actual else {
        panic!("case {case:?}: expected Metadata {expected:?}, got no Metadata element");
    };
    assert_eq!(
        as_json(case, actual),
        as_json(case, expected),
        "case {case:?}: expected the declared Metadata {expected:?}, got {actual:?}"
    );
}

/// What one declared resource must be described as.
struct ExpectedDetail {
    logical_id: &'static str,
    resource_type: &'static str,
    physical_id: PhysicalId,
    /// The content of the resource's declared `Metadata`, or `None` for a declaration
    /// that carries none.
    metadata: Option<&'static str>,
}

/// One row of the described-resource table: a stack, what describing each of its
/// resources must report, and the logical ids it must refuse.
struct DetailCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    expected: Vec<ExpectedDetail>,
    /// Logical ids this stack does NOT deploy, which must be refused rather than
    /// answered with an empty description.
    absent: Vec<&'static str>,
}

fn expect(
    logical_id: &'static str,
    resource_type: &'static str,
    physical_id: PhysicalId,
    metadata: Option<&'static str>,
) -> ExpectedDetail {
    ExpectedDetail {
        logical_id,
        resource_type,
        physical_id,
        metadata,
    }
}

/// The class the rule covers. The issue's own case — a stack created through
/// `CreateStack` whose one resource a client then describes by its logical id — is one
/// row of it.
fn detail_cases() -> Vec<DetailCase> {
    vec![
        DetailCase {
            // A template may declare no resources at all, so every logical id is absent
            // and every request against this stack is refused.
            case: "a json template with no Resources section",
            stack_name: "detail-empty",
            template: "{}".to_string(),
            expected: vec![],
            absent: vec!["Anything"],
        },
        DetailCase {
            case: "a json template naming one catalogued resource, with Metadata",
            stack_name: "detail-json",
            template: concat!(
                r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","#,
                r#""Properties":{"TopicName":"detail-topic"},"#,
                r#""Metadata":{"Owner":"platform"}}}}"#
            )
            .to_string(),
            expected: vec![expect(
                "Topic",
                "AWS::SNS::Topic",
                PhysicalId::Exact(sns_arn("detail-topic")),
                Some(r#"{"Owner":"platform"}"#),
            )],
            // A logical id is matched exactly, so neither another casing of the declared
            // one nor an undeclared one names a resource.
            absent: vec!["topic", "Missing"],
        },
        DetailCase {
            case: "a yaml template whose one resource is unnamed and declares no Metadata",
            stack_name: "detail-yaml",
            template: "Resources:\n  Unnamed:\n    Type: AWS::SNS::Topic\n".to_string(),
            expected: vec![expect(
                "Unnamed",
                "AWS::SNS::Topic",
                PhysicalId::Generated(sns_arn("detail-yaml-Unnamed-")),
                None,
            )],
            absent: vec!["Named"],
        },
        DetailCase {
            // Three types, declared out of logical-id order, one of them a type the
            // return-value catalogue does not carry; two carry Metadata and one does not.
            case: "a yaml template declaring three resources of three types",
            stack_name: "detail-mixed",
            template: concat!(
                "Resources:\n",
                "  Table:\n",
                "    Type: AWS::DynamoDB::Table\n",
                "    Properties:\n",
                "      TableName: detail-table\n",
                "      BillingMode: PAY_PER_REQUEST\n",
                "      KeySchema:\n",
                "        - AttributeName: pk\n",
                "          KeyType: HASH\n",
                "      AttributeDefinitions:\n",
                "        - AttributeName: pk\n",
                "          AttributeType: S\n",
                "    Metadata:\n",
                "      Purpose: ingest\n",
                "  Bucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: detail-bucket\n",
                "  Alpha:\n",
                "    Type: AWS::CloudFormation::WaitConditionHandle\n",
                "    Metadata:\n",
                "      Nested:\n",
                "        Depth: two\n",
            )
            .to_string(),
            expected: vec![
                expect(
                    "Table",
                    "AWS::DynamoDB::Table",
                    PhysicalId::Exact("detail-table".to_string()),
                    Some(r#"{"Purpose":"ingest"}"#),
                ),
                expect(
                    "Bucket",
                    "AWS::S3::Bucket",
                    PhysicalId::Exact("detail-bucket".to_string()),
                    None,
                ),
                expect(
                    "Alpha",
                    "AWS::CloudFormation::WaitConditionHandle",
                    PhysicalId::Generated("detail-mixed-Alpha-".to_string()),
                    Some(r#"{"Nested":{"Depth":"two"}}"#),
                ),
            ],
            absent: vec!["Beta"],
        },
        DetailCase {
            // `Metadata` is reported as its content, whatever JSON form the declaration
            // spells it in — the wire member is a string, not an object.
            case: "a json template whose Metadata is an array, a number and a string",
            stack_name: "detail-shapes",
            template: concat!(
                r#"{"Resources":{"#,
                r#""List":{"Type":"AWS::SNS::Topic","Metadata":["a","b"]},"#,
                r#""Count":{"Type":"AWS::SNS::Topic","Metadata":{"Retries":3}},"#,
                r#""Text":{"Type":"AWS::SNS::Topic","Metadata":"plain"}}}"#
            )
            .to_string(),
            expected: vec![
                expect(
                    "List",
                    "AWS::SNS::Topic",
                    PhysicalId::Generated(sns_arn("detail-shapes-List-")),
                    Some(r#"["a","b"]"#),
                ),
                expect(
                    "Count",
                    "AWS::SNS::Topic",
                    PhysicalId::Generated(sns_arn("detail-shapes-Count-")),
                    Some(r#"{"Retries":3}"#),
                ),
                expect(
                    "Text",
                    "AWS::SNS::Topic",
                    PhysicalId::Generated(sns_arn("detail-shapes-Text-")),
                    Some(r#""plain""#),
                ),
            ],
            absent: vec!["Absent"],
        },
    ]
}

/// Assert one described resource against what the row expects of it.
fn assert_detail(
    case: &str,
    expected: &ExpectedDetail,
    actual: &WireDetail,
    stack_name: &str,
    stack_id: &str,
    status: &str,
) {
    assert_eq!(
        actual.logical_id.as_deref(),
        Some(expected.logical_id),
        "case {case:?}: expected the requested logical id back, got {actual:?}"
    );
    assert_eq!(
        actual.resource_type.as_deref(),
        Some(expected.resource_type),
        "case {case:?}: expected the declared type {:?}, got {actual:?}",
        expected.resource_type
    );
    assert_physical_id(case, &expected.physical_id, actual.physical_id.as_deref());
    assert_eq!(
        actual.stack_name.as_deref(),
        Some(stack_name),
        "case {case:?}: the detail must name the stack the resource belongs to, got {actual:?}"
    );
    assert_eq!(
        actual.stack_id.as_deref(),
        Some(stack_id),
        "case {case:?}: the detail must carry its stack's id, got {actual:?}"
    );
    assert_eq!(
        actual.status.as_deref(),
        Some(status),
        "case {case:?}: expected the resource status {status:?}, got {actual:?}"
    );
    assert_eq!(
        actual.status_reason, None,
        "case {case:?}: nothing failed, so no resource carries a status reason, got {actual:?}"
    );
    assert_eq!(
        actual.description, None,
        "case {case:?}: a resource declaration carries no description, got {actual:?}"
    );
    assert!(
        !actual.has_module_info,
        "case {case:?}: no resource here came from a module, got {actual:?}"
    );
    assert_metadata(case, expected.metadata, actual.metadata.as_deref());
    let timestamp = actual.last_updated_timestamp.clone().unwrap_or_default();
    assert!(
        is_iso8601_instant(&timestamp),
        "case {case:?}: expected the instant the status was updated, got {timestamp:?}"
    );
    assert_eq!(
        actual.drift_status.as_deref(),
        Some("NOT_CHECKED"),
        "case {case:?}: a resource never checked for drift is NOT_CHECKED, got {actual:?}"
    );
    assert_eq!(
        actual.drift_last_check, None,
        "case {case:?}: a resource never checked for drift has no check instant, got {actual:?}"
    );
}

/// Assert a refusal: HTTP 400 with CloudFormation's own `ValidationError` code and a
/// message built from what the request named.
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

#[tokio::test]
async fn a_deployed_resource_is_described_from_what_its_deployment_persisted() {
    let router = router();

    for case in detail_cases() {
        let (status, body) = create_stack(&router, case.stack_name, &case.template, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
        let stack_id = xml_text(&body, "StackId")
            .unwrap_or_else(|| panic!("case {:?}: CreateStack answered no StackId", case.case));
        wait_for_the_transition().await;

        // Addressed by name and by stack id: both name the same stack, so both describe
        // the same resource.
        for identifier in [case.stack_name.to_string(), stack_id.clone()] {
            for expected in &case.expected {
                let (status, body) =
                    describe_stack_resource(&router, &identifier, expected.logical_id).await;
                let detail = detail_of(case.case, status, &body);
                assert_detail(
                    case.case,
                    expected,
                    &detail,
                    case.stack_name,
                    &stack_id,
                    resource_status::CREATE_COMPLETE,
                );
            }
            for absent in &case.absent {
                let (status, body) = describe_stack_resource(&router, &identifier, absent).await;
                assert_refused(case.case, status, &body, &[absent, &identifier]);
            }
        }
    }
}

/// A `StackName` that resolves to no stack is refused, whatever logical id the request
/// names — including a name whose stack was never created and an id of the right shape
/// that names nothing.
#[tokio::test]
async fn a_stack_that_does_not_exist_is_refused() {
    let router = router();
    create_stack(
        &router,
        "detail-present",
        r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
        &[],
    )
    .await;
    wait_for_the_transition().await;

    let rows: Vec<(&str, &str)> = vec![
        ("a stack name nothing created", "detail-absent"),
        (
            "a stack id of the right shape naming nothing",
            "arn:aws:cloudformation:us-east-1:000000000000:stack/detail-absent/00000000-0000-0000-0000-000000000000",
        ),
        ("another casing of a stack that does exist", "Detail-Present"),
    ];
    for (case, stack_name) in rows {
        let (status, body) = describe_stack_resource(&router, stack_name, "Topic").await;
        assert_refused(case, status, &body, &[stack_name]);
    }
}

/// A resource is described only once the deployment that materialises it has COMPLETED: a
/// read taken while the create is still in progress has no deployed resource to describe.
#[tokio::test]
async fn a_resource_is_described_only_once_its_deployment_completed() {
    let router = router();
    let started = Instant::now();
    let (status, body) = create_stack(
        &router,
        "detail-pending",
        r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"pending-topic"}}}}"#,
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );

    let (status, body) = describe_stack_resource(&router, "detail-pending", "Topic").await;
    if started.elapsed() < STATUS_TRANSITION_DELAY {
        assert_refused(
            "a read taken while the create is still in progress",
            status,
            &body,
            &["Topic", "detail-pending"],
        );
    } else {
        // The machine lost the race to the clock, so the create had already settled by the
        // time the read landed: the emulator then owes the completed description, and the
        // case holds it to that rather than asserting nothing.
        let detail = detail_of(
            "a read the clock beat to the create's completion",
            status,
            &body,
        );
        assert_eq!(
            detail.status.as_deref(),
            Some(resource_status::CREATE_COMPLETE),
            "the create had settled, so its resource must be CREATE_COMPLETE, got {detail:?}"
        );
    }

    wait_for_the_transition().await;
    let (status, body) = describe_stack_resource(&router, "detail-pending", "Topic").await;
    let detail = detail_of("a read taken after the create completed", status, &body);
    assert_eq!(
        detail.status.as_deref(),
        Some(resource_status::CREATE_COMPLETE),
        "the completed create must leave its resource CREATE_COMPLETE, got {detail:?}"
    );
    assert_eq!(
        detail.physical_id.as_deref(),
        Some(sns_arn("pending-topic").as_str()),
        "expected the id the documented naming rule gives a named topic, got {detail:?}"
    );
}

/// What an UPDATE leaves each resource in is what `DescribeStackResource` reports: the
/// resource it changed carries the update's completion at the instant it completed, the
/// one it left alone keeps the status and instant it already had, and a `Metadata` the
/// update rewrote is reported as the new content.
#[tokio::test]
async fn an_update_is_reported_per_resource() {
    let router = router();
    let before = concat!(
        r#"{"Resources":{"#,
        r#""Keep":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"keep-topic"}},"#,
        r#""Change":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"change-topic"},"#,
        r#""Metadata":{"Revision":"one"}}}}"#
    );
    let after = concat!(
        r#"{"Resources":{"#,
        r#""Keep":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"keep-topic"}},"#,
        r#""Change":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"change-topic"},"#,
        r#""Metadata":{"Revision":"two"}},"#,
        r#""Added":{"Type":"AWS::Logs::LogGroup","Properties":{"LogGroupName":"added-logs"}}}}"#
    );

    let (status, body) = create_stack(&router, "detail-updated", before, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let mut created_at = Vec::new();
    for logical_id in ["Keep", "Change"] {
        let (status, body) = describe_stack_resource(&router, "detail-updated", logical_id).await;
        let detail = detail_of("the stack as created", status, &body);
        created_at.push((logical_id, detail));
    }

    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "detail-updated"),
            member("TemplateBody", after),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UpdateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    // (case, logical id, the status the resource must carry, the Metadata it must report,
    //  whether the update moved the instant the status was updated)
    let rows: Vec<(&str, &str, &str, Option<&str>, bool)> = vec![
        (
            "a resource the update left exactly as it was",
            "Keep",
            resource_status::CREATE_COMPLETE,
            None,
            false,
        ),
        (
            "a resource whose Metadata the update rewrote",
            "Change",
            resource_status::UPDATE_COMPLETE,
            Some(r#"{"Revision":"two"}"#),
            true,
        ),
        (
            "a resource the update added",
            "Added",
            resource_status::CREATE_COMPLETE,
            None,
            true,
        ),
    ];
    for (case, logical_id, status_value, metadata, moved) in rows {
        let (status, body) = describe_stack_resource(&router, "detail-updated", logical_id).await;
        let detail = detail_of(case, status, &body);
        assert_eq!(
            detail.status.as_deref(),
            Some(status_value),
            "case {case:?}: expected {status_value}, got {detail:?}"
        );
        assert_metadata(case, metadata, detail.metadata.as_deref());
        if let Some((_, created)) = created_at.iter().find(|(id, _)| *id == logical_id) {
            let was = created.last_updated_timestamp.clone().unwrap_or_default();
            let now = detail.last_updated_timestamp.clone().unwrap_or_default();
            if moved {
                assert!(
                    now > was,
                    "case {case:?}: the update changed the resource, so its instant must \
                     move past {was:?}, got {now:?}"
                );
            } else {
                assert_eq!(
                    now, was,
                    "case {case:?}: nothing about this resource's status was updated, so \
                     its instant must not move, got {now:?}"
                );
            }
        }
    }
}

/// "For deleted stacks, DescribeStackResource returns resource information for up to 90
/// days after the stack has been deleted": a completed deletion leaves the resource
/// described as `DELETE_COMPLETE`, addressable by the stack's unique id — and the stack's
/// NAME no longer names it ("Deleted stacks: You must specify the unique stack ID").
#[tokio::test]
async fn a_deleted_stack_still_describes_its_resources_by_stack_id() {
    let router = router();
    let (status, body) = create_stack(
        &router,
        "detail-doomed",
        r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Metadata":{"Owner":"platform"}}}}"#,
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );
    let stack_id = xml_text(&body, "StackId").expect("CreateStack answered no StackId");
    wait_for_the_transition().await;

    call(
        &router,
        "DeleteStack",
        &[member("StackName", "detail-doomed")],
    )
    .await;
    wait_for_the_transition().await;

    let (status, body) = describe_stack_resource(&router, &stack_id, "Topic").await;
    let detail = detail_of("a deleted stack addressed by its id", status, &body);
    assert_eq!(
        detail.status.as_deref(),
        Some(resource_status::DELETE_COMPLETE),
        "the completed deletion deleted the resource, got {detail:?}"
    );
    assert_metadata(
        "a deleted stack addressed by its id",
        Some(r#"{"Owner":"platform"}"#),
        detail.metadata.as_deref(),
    );
    assert_eq!(
        detail.stack_id.as_deref(),
        Some(stack_id.as_str()),
        "the detail must carry the deleted stack's id, got {detail:?}"
    );

    let (status, body) = describe_stack_resource(&router, "detail-doomed", "Topic").await;
    assert_refused(
        "a deleted stack addressed by its name",
        status,
        &body,
        &["detail-doomed"],
    );
}

/// `DescribeStackResource` and `DescribeStackResources` read the SAME persisted resource,
/// so for one logical id of one stack they report the same values.
#[tokio::test]
async fn one_resource_reads_the_same_through_both_describes() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Bucket:\n",
        "    Type: AWS::S3::Bucket\n",
        "    Properties:\n",
        "      BucketName: agree-bucket\n",
        "  Topic:\n",
        "    Type: AWS::SNS::Topic\n",
    );
    let (status, body) = create_stack(&router, "detail-agree", template, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    for logical_id in ["Bucket", "Topic"] {
        let (status, body) = call(
            &router,
            "DescribeStackResources",
            &[
                member("StackName", "detail-agree"),
                member("LogicalResourceId", logical_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {logical_id:?}: expected DescribeStackResources to succeed, got {status} {body}"
        );
        let plural = body;
        let (status, body) = describe_stack_resource(&router, "detail-agree", logical_id).await;
        let detail = detail_of(logical_id, status, &body);

        for (tag, singular) in [
            ("StackName", detail.stack_name.as_deref()),
            ("StackId", detail.stack_id.as_deref()),
            ("LogicalResourceId", detail.logical_id.as_deref()),
            ("PhysicalResourceId", detail.physical_id.as_deref()),
            ("ResourceType", detail.resource_type.as_deref()),
            ("ResourceStatus", detail.status.as_deref()),
            ("StackResourceDriftStatus", detail.drift_status.as_deref()),
        ] {
            assert_eq!(
                singular,
                xml_text(&plural, tag).as_deref(),
                "case {logical_id:?}: both describes read one resource, so {tag} must agree; \
                 got {singular:?} against {plural}"
            );
        }
        // The plural read spells the same instant under `Timestamp`, the singular under
        // `LastUpdatedTimestamp` ("Time the status was updated" in both).
        assert_eq!(
            detail.last_updated_timestamp.as_deref(),
            xml_text(&plural, "Timestamp").as_deref(),
            "case {logical_id:?}: both describes report the instant the status was updated, \
             got {detail:?} against {plural}"
        );
    }
}
