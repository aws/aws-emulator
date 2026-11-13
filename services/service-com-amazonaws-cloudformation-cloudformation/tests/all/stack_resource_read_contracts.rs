// SPDX-License-Identifier: CC0-1.0

//! The two read contracts of CloudFormation's per-resource describes, end to end over the
//! awsQuery wire.
//!
//! Two rules are under test, each asserted over the whole class it covers rather than over
//! one recorded request:
//!
//! 1. `DescribeStackResource` reports `StackResourceDetail$Metadata` for EVERY resource it
//!    describes. The member carries "the content of the `Metadata` attribute declared for
//!    the resource" as a JSON document in a string, and a declaration that states no
//!    `Metadata` has an EMPTY document rather than no document — so the element is always
//!    present and always parses as JSON, whatever the declaration spelled.
//! 2. `DescribeStackResources` selects its stack from EXACTLY ONE of `StackName` and
//!    `PhysicalResourceId` ("You must specify either StackName or PhysicalResourceId, but
//!    not both"), and every request outside that rule — one naming neither and one naming
//!    both — is refused with HTTP 400 `ValidationError` and the ONE fixed message
//!    "Either StackName or PhysicalResourceId must be specified.", while each spelling
//!    inside it answers with the whole owning stack's resources.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client calling these operations sees.

use crate::stack_lifecycle::{
    call, create_stack, member, router, wait_for_the_transition, xml_text,
};
use crate::stack_resources::{element, sns_arn, wire_resources};
use smithy_server_runtime::MultiServiceRouter;

/// The ONE refusal both spellings outside the either-or rule answer with, as AWS words it
/// — fixed text, trailing period included.
const NO_SINGLE_SELECTOR: &str = "Either StackName or PhysicalResourceId must be specified.";

/// The `Metadata` element of the one `StackResourceDetail` a response carries.
///
/// `None` only when the response carries no `Metadata` element at all, which is the defect
/// these cases pin: the detail block itself is located first, so a `Metadata` element of
/// some other structure cannot be mistaken for it.
fn reported_metadata(case: &str, body: &str) -> Option<String> {
    let start = body
        .find("<StackResourceDetail")
        .unwrap_or_else(|| panic!("case {case:?}: expected a StackResourceDetail, got {body}"));
    let after_tag = &body[start + "<StackResourceDetail".len()..];
    let tag_end = after_tag
        .find('>')
        .unwrap_or_else(|| panic!("case {case:?}: unterminated StackResourceDetail in {body}"));
    let after = &after_tag[tag_end + 1..];
    let end = after
        .find("</StackResourceDetail>")
        .unwrap_or_else(|| panic!("case {case:?}: unclosed StackResourceDetail in {body}"));
    element(&after[..end], "Metadata")
}

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

/// One declared resource and the `Metadata` document describing it must report.
struct MetadataCase {
    case: &'static str,
    logical_id: &'static str,
    /// The resource's declaration, as the template body spells it.
    declaration: &'static str,
    /// The document the detail must carry, as JSON text.
    expected: &'static str,
}

/// RULE: the `Metadata` member is reported for every described resource — the declared
/// document where the declaration carries one, the EMPTY document where it carries none.
///
/// The table spans the class the rule generalises over: a declaration with no `Metadata`
/// key at all, one whose `Metadata` is explicitly null, one whose `Metadata` is the empty
/// object, and each JSON form a `Metadata` body may take (object, nested object, array,
/// number, string, boolean). Only the first three are the "declares none" half of the
/// rule; the rest pin that reporting an empty document for them did not flatten what a
/// real declaration carries.
#[tokio::test]
async fn every_described_resource_reports_a_metadata_document() {
    let cases = vec![
        MetadataCase {
            case: "a declaration with no Metadata key at all",
            logical_id: "Bare",
            declaration: r#""Bare":{"Type":"AWS::SNS::Topic"}"#,
            expected: "{}",
        },
        MetadataCase {
            case: "a declaration whose Metadata is null",
            logical_id: "Null",
            declaration: r#""Null":{"Type":"AWS::SNS::Topic","Metadata":null}"#,
            expected: "{}",
        },
        MetadataCase {
            case: "a declaration whose Metadata is the empty object",
            logical_id: "Empty",
            declaration: r#""Empty":{"Type":"AWS::SNS::Topic","Metadata":{}}"#,
            expected: "{}",
        },
        MetadataCase {
            case: "a declaration whose Metadata is an object",
            logical_id: "Flat",
            declaration: r#""Flat":{"Type":"AWS::SNS::Topic","Metadata":{"Owner":"platform"}}"#,
            expected: r#"{"Owner":"platform"}"#,
        },
        MetadataCase {
            case: "a declaration whose Metadata nests objects",
            logical_id: "Nested",
            declaration:
                r#""Nested":{"Type":"AWS::SNS::Topic","Metadata":{"Build":{"Depth":"two"}}}"#,
            expected: r#"{"Build":{"Depth":"two"}}"#,
        },
        MetadataCase {
            case: "a declaration whose Metadata is an array",
            logical_id: "List",
            declaration: r#""List":{"Type":"AWS::SNS::Topic","Metadata":["a","b"]}"#,
            expected: r#"["a","b"]"#,
        },
        MetadataCase {
            case: "a declaration whose Metadata is a number",
            logical_id: "Count",
            declaration: r#""Count":{"Type":"AWS::SNS::Topic","Metadata":3}"#,
            expected: "3",
        },
        MetadataCase {
            case: "a declaration whose Metadata is a string",
            logical_id: "Text",
            declaration: r#""Text":{"Type":"AWS::SNS::Topic","Metadata":"plain"}"#,
            expected: r#""plain""#,
        },
        MetadataCase {
            case: "a declaration whose Metadata is a boolean",
            logical_id: "Flag",
            declaration: r#""Flag":{"Type":"AWS::SNS::Topic","Metadata":true}"#,
            expected: "true",
        },
    ];

    let declarations = cases
        .iter()
        .map(|row| row.declaration)
        .collect::<Vec<_>>()
        .join(",");
    let template = format!(r#"{{"Resources":{{{declarations}}}}}"#);

    let router = router();
    let (status, body) = create_stack(&router, "metadata-documents", &template, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    for MetadataCase {
        case,
        logical_id,
        declaration: _,
        expected,
    } in &cases
    {
        let (status, body) =
            describe_stack_resource(&router, "metadata-documents", logical_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStackResource to succeed, got {status} {body}"
        );
        let reported = reported_metadata(case, &body);
        let Some(reported) = reported else {
            panic!(
                "case {case:?}: every described resource reports a Metadata document; \
                 expected {expected:?}, got no Metadata element in {body}"
            );
        };
        let parsed: ::serde_json::Value = ::serde_json::from_str(&reported).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: Metadata carries a JSON document; expected {expected:?}, \
                 got {reported:?} ({error})"
            )
        });
        let wanted: ::serde_json::Value =
            ::serde_json::from_str(expected).expect("the case states a JSON document");
        assert_eq!(
            parsed, wanted,
            "case {case:?}: expected the Metadata document {expected:?}, got {reported:?}"
        );
    }
}

/// One `DescribeStackResources` request outside the exactly-one-selector rule.
struct SelectorRefusal {
    case: &'static str,
    members: Vec<(String, String)>,
}

/// RULE: a `DescribeStackResources` request that does not name exactly one of `StackName`
/// and `PhysicalResourceId` is refused with HTTP 400 `ValidationError` and the one fixed
/// message "Either StackName or PhysicalResourceId must be specified."
///
/// The table spans both halves of the rule and both spellings of a stack identifier: no
/// selector at all, a `LogicalResourceId` alone (which selects no stack), and both
/// selectors together with the stack named once by its name and once by its id — and, for
/// the empty-string spelling of each member, which names no stack either.
#[tokio::test]
async fn naming_neither_or_both_stack_selectors_is_refused_with_one_message() {
    let router = router();
    let (status, created) = create_stack(
        &router,
        "selector-rule",
        r#"{"Resources":{
             "Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"selector-rule-topic"}},
             "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {created}"
    );
    let stack_id = xml_text(&created, "StackId").expect("CreateStack answered no StackId");
    wait_for_the_transition().await;
    let physical_id = sns_arn("selector-rule-topic");

    let cases = vec![
        SelectorRefusal {
            case: "neither selector",
            members: vec![],
        },
        SelectorRefusal {
            case: "a LogicalResourceId alone, which selects no stack",
            members: vec![member("LogicalResourceId", "Topic")],
        },
        SelectorRefusal {
            case: "both selectors, the stack named by its name",
            members: vec![
                member("StackName", "selector-rule"),
                member("PhysicalResourceId", &physical_id),
            ],
        },
        SelectorRefusal {
            case: "both selectors, the stack named by its id",
            members: vec![
                member("StackName", &stack_id),
                member("PhysicalResourceId", &physical_id),
            ],
        },
        SelectorRefusal {
            case: "both selectors, the physical id of a resource of another type",
            members: vec![
                member("StackName", "selector-rule"),
                member("PhysicalResourceId", "no-such-resource"),
            ],
        },
    ];

    for SelectorRefusal { case, members } in cases {
        let (status, body) = call(&router, "DescribeStackResources", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(NO_SINGLE_SELECTOR),
            "case {case:?}: expected the either-or refusal {NO_SINGLE_SELECTOR:?}, got {body}"
        );
    }

    // The two spellings INSIDE the rule each answer with the whole owning stack, so the
    // refusal above is about the combination and not about either member.
    for (case, members) in [
        (
            "the stack named by its name",
            vec![member("StackName", "selector-rule")],
        ),
        (
            "the stack named by its id",
            vec![member("StackName", &stack_id)],
        ),
        (
            "the stack reached through one resource's physical id",
            vec![member("PhysicalResourceId", &physical_id)],
        ),
    ] {
        let (status, body) = call(&router, "DescribeStackResources", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStackResources to succeed, got {status} {body}"
        );
        let mut reported = wire_resources(&body)
            .into_iter()
            .map(|resource| resource.logical_id.unwrap_or_default())
            .collect::<Vec<_>>();
        reported.sort();
        assert_eq!(
            reported,
            vec!["Handle".to_string(), "Topic".to_string()],
            "case {case:?}: expected the whole owning stack's resources, got {body}"
        );
    }
}
