// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's app block builder registry over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over the one
//! recorded request:
//!
//! * a create persists the builder and reports it back FROM the stored record: every
//!   member the request gave is reported unchanged, a member it omitted is absent, and
//!   the ARN, the state and the creation time are derived — so a later
//!   `DescribeAppBlockBuilders` answers identically;
//! * a created builder is `STOPPED`, because a builder can only be started once an app
//!   block is associated with it and a create associates none;
//! * a second create of a name the account holds is refused with
//!   `ResourceAlreadyExistsException` and AWS's duplicate-name sentence, and leaves the
//!   stored builder untouched;
//! * a read reports only stored builders: an unknown name is refused with
//!   `ResourceNotFoundException` and AWS's sentence for that class, a read naming none
//!   reports every stored builder, and `MaxResults` sizes the page so that following
//!   `NextToken` walks every record exactly once;
//! * the tags a create persisted live on the builder's own record, so they are the ones
//!   `ListTagsForResource` reports and `TagResource`/`UntagResource` change.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::refusal_messages::{already_exists_sentence, not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::Emulator;

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The only `AppBlockBuilderPlatformType` the model declares
/// (`APP_BLOCK_BUILDER_PLATFORM_TYPE_VALUES` in `src/ops/types/shared_types.rs`), and
/// the only value `CreateAppBlockBuilder.Platform` documents.
const PLATFORM: &str = "WINDOWS_SERVER_2019";

/// The `AppBlockBuilderState` a created builder carries.
const STOPPED: &str = "STOPPED";

/// The word AWS's refusal sentences use for an app block builder.
const KIND: &str = "appblockbuilder";

/// The instance types `CreateAppBlockBuilder.InstanceType` documents as available
/// ("stream.standard.small stream.standard.medium stream.standard.large
/// stream.standard.xlarge stream.standard.2xlarge" —
/// `smithy_query(query="doc", shape="CreateAppBlockBuilderRequest", member="InstanceType")`).
/// The rows below span the whole catalog, not only the smallest.
const INSTANCE_TYPES: &[&str] = &[
    "stream.standard.small",
    "stream.standard.medium",
    "stream.standard.large",
    "stream.standard.xlarge",
    "stream.standard.2xlarge",
];

/// The only `AccessEndpointType` the model declares (`ACCESS_ENDPOINT_TYPE_VALUES` in
/// `src/ops/types/shared_types.rs`).
const ENDPOINT_TYPE: &str = "STREAMING";

/// The members `CreateAppBlockBuilder` accepts that the modelled `AppBlockBuilder` also
/// carries: each is reported back exactly as the request gave it, and is absent from the
/// reported builder when the request omitted it. `Tags` is not among them — a resource's
/// tags are reported by `ListTagsForResource`, not by the resource itself.
const ECHOED_MEMBERS: &[&str] = &[
    "Name",
    "Platform",
    "InstanceType",
    "VpcConfig",
    "Description",
    "DisplayName",
    "EnableDefaultInternetAccess",
    "IamRoleArn",
    "AccessEndpoints",
    "DisableIMDSV1",
];

/// Names spanning the class: the rows differ along length, character classes and case.
/// The last is the longest name a create admits — `Name`'s pattern is
/// `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`
/// (`smithy_query(query="operation", operation="CreateAppBlockBuilder")`), so 101
/// characters.
fn names() -> Vec<String> {
    vec![
        "builder".to_string(),
        "Mixed.Case_builder-9".to_string(),
        "0123456789".to_string(),
        format!("b{}", "7".repeat(100)),
    ]
}

/// `AccessEndpoints` of `count` entries — 1 to 4 is the modelled length range of
/// `CreateAppBlockBuilder.AccessEndpoints`.
fn access_endpoints(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "EndpointType": ENDPOINT_TYPE, "VpceId": format!("vpce-0{index}") }))
            .collect(),
    )
}

/// One create row: a case name and the request body it sends.
struct Row {
    case: String,
    request: Value,
}

/// Create requests spanning the class: the rows differ along which optional members are
/// given, the instance type, how many subnets and security groups the VPC configuration
/// names, how many access endpoints are given, and the value of each boolean.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "only the required members".to_string(),
            request: json!({
                "Name": "minimal",
                "Platform": PLATFORM,
                "InstanceType": INSTANCE_TYPES[0],
                "VpcConfig": { "SubnetIds": ["subnet-a1", "subnet-b2"] },
            }),
        },
        Row {
            case: "every optional member given".to_string(),
            request: json!({
                "Name": "fully.specified-1",
                "Platform": PLATFORM,
                "InstanceType": INSTANCE_TYPES[4],
                "VpcConfig": {
                    "SubnetIds": ["subnet-a1", "subnet-b2", "subnet-c3"],
                    "SecurityGroupIds": ["sg-1", "sg-2"],
                },
                "Description": "a builder with everything set",
                "DisplayName": "Fully specified",
                "EnableDefaultInternetAccess": true,
                "DisableIMDSV1": true,
                "IamRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/app-block-builder"),
                "AccessEndpoints": access_endpoints(4),
            }),
        },
        Row {
            case: "both booleans false".to_string(),
            request: json!({
                "Name": "booleans-false",
                "Platform": PLATFORM,
                "InstanceType": INSTANCE_TYPES[2],
                "VpcConfig": {
                    "SubnetIds": ["subnet-a1"],
                    "SecurityGroupIds": [],
                },
                "EnableDefaultInternetAccess": false,
                "DisableIMDSV1": false,
            }),
        },
        Row {
            case: "an empty description and display name".to_string(),
            request: json!({
                "Name": "empty-text",
                "Platform": PLATFORM,
                "InstanceType": INSTANCE_TYPES[3],
                "VpcConfig": { "SubnetIds": ["subnet-a1", "subnet-b2"] },
                // `Description` and `DisplayName` are length 0..256 / 0..100, so the
                // empty string is a value a create accepts and must report back.
                "Description": "",
                "DisplayName": "",
            }),
        },
    ];

    // One row per admissible `AccessEndpoints` length, so the count the rule generalises
    // over is covered at both published bounds.
    for count in 1..=4_usize {
        rows.push(Row {
            case: format!("{count} access endpoint(s)"),
            request: json!({
                "Name": format!("endpoints-{count}"),
                "Platform": PLATFORM,
                "InstanceType": INSTANCE_TYPES[count % INSTANCE_TYPES.len()],
                "VpcConfig": { "SubnetIds": ["subnet-a1", "subnet-b2"] },
                "AccessEndpoints": access_endpoints(count),
            }),
        });
    }
    rows
}

/// The ARN AppStream stamps on an app block builder.
fn builder_arn(region: &str, name: &str) -> String {
    format!("arn:aws:appstream:{region}:{ACCOUNT}:app-block-builder/{name}")
}

/// The `AppBlockBuilder` a create or a read reported.
fn builder(case: &str, payload: &Value) -> Value {
    payload
        .get("AppBlockBuilder")
        .unwrap_or_else(|| panic!("case {case:?}: expected an AppBlockBuilder, got {payload}"))
        .clone()
}

/// The builders a `DescribeAppBlockBuilders` reported.
fn builders(case: &str, payload: &Value) -> Vec<Value> {
    payload
        .get("AppBlockBuilders")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected AppBlockBuilders, got {payload}"))
        .clone()
}

/// Assert the reported builder renders the request: every member the request gave is
/// reported unchanged, every member it omitted is absent, and the derived members carry
/// the ARN, the created state and a creation time.
fn assert_renders_request(case: &str, region: &str, request: &Value, reported: &Value) {
    let name = request["Name"].as_str().expect("the row names the builder");

    for member in ECHOED_MEMBERS {
        let expected = request.get(member);
        let actual = reported.get(member);
        assert_eq!(
            actual, expected,
            "case {case:?}: {member} must be reported as the request gave it \
             ({expected:?}), got {actual:?}"
        );
    }

    let expected_arn = builder_arn(region, name);
    assert_eq!(
        reported.get("Arn").and_then(Value::as_str),
        Some(expected_arn.as_str()),
        "case {case:?}: expected Arn {expected_arn:?}, got {reported}"
    );
    assert_eq!(
        reported.get("State").and_then(Value::as_str),
        Some(STOPPED),
        "case {case:?}: a created builder must be {STOPPED}, got {reported}"
    );
    assert!(
        reported.get("CreatedTime").is_some_and(Value::is_number),
        "case {case:?}: expected a numeric CreatedTime, got {reported}"
    );
    // Nothing has failed and nothing has changed the builder's state, so neither member
    // is claimed.
    for member in ["AppBlockBuilderErrors", "StateChangeReason"] {
        assert!(
            reported.get(member).is_none(),
            "case {case:?}: a freshly created builder must report no {member}, got {reported}"
        );
    }
    assert!(
        reported.get("Tags").is_none(),
        "case {case:?}: the modelled AppBlockBuilder carries no Tags member, got {reported}"
    );
}

#[tokio::test]
async fn a_created_builder_is_reported_back_from_the_stored_record() {
    // Rows: the Region the request's credential scope names — the Region whose ARNs the
    // created builder carries.
    for region in ["us-east-1", "eu-west-2"] {
        let emulator = Emulator::in_region(region);

        for row in rows() {
            let case = format!("{region} {}", row.case);
            let created = builder(
                &case,
                &emulator
                    .ok(&case, "CreateAppBlockBuilder", row.request.clone())
                    .await,
            );
            assert_renders_request(&case, region, &row.request, &created);

            let name = row.request["Name"].as_str().expect("the row names it");
            let read = emulator
                .ok(
                    &case,
                    "DescribeAppBlockBuilders",
                    json!({ "Names": [name] }),
                )
                .await;
            let reported = builders(&case, &read);
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: a read naming one builder reports exactly it, got {read}"
            );
            assert_eq!(
                reported[0], created,
                "case {case:?}: the read must report the record the create persisted \
                 ({created}), got {}",
                reported[0]
            );

            // A read changes nothing a later caller sees: the same read answers the same
            // builder.
            let again = emulator
                .ok(
                    &case,
                    "DescribeAppBlockBuilders",
                    json!({ "Names": [name] }),
                )
                .await;
            assert_eq!(
                again, read,
                "case {case:?}: a second read must answer identically, got {again}"
            );
        }

        // Every row's builder is in the account-wide read, each exactly once.
        let case = format!("{region} every stored builder");
        let read = emulator
            .ok(&case, "DescribeAppBlockBuilders", json!({}))
            .await;
        let reported: Vec<String> = builders(&case, &read)
            .iter()
            .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: BTreeSet<String> = rows()
            .iter()
            .map(|row| row.request["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case:?}: expected {} builders, got {reported:?}",
            expected.len()
        );
        assert_eq!(
            reported.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_name_the_account_holds_is_refused_and_leaves_the_builder_untouched() {
    for name in names() {
        let emulator = Emulator::new();
        let case = format!("duplicate {name:?}");
        let request = json!({
            "Name": name,
            "Platform": PLATFORM,
            "InstanceType": INSTANCE_TYPES[1],
            "VpcConfig": { "SubnetIds": ["subnet-a1", "subnet-b2"] },
        });
        let created = builder(
            &case,
            &emulator
                .ok(&case, "CreateAppBlockBuilder", request.clone())
                .await,
        );

        // A second create with DIFFERENT members is still refused: the name is what
        // collides.
        let message = refusal(
            &emulator,
            &case,
            "CreateAppBlockBuilder",
            json!({
                "Name": name,
                "Platform": PLATFORM,
                "InstanceType": INSTANCE_TYPES[4],
                "VpcConfig": { "SubnetIds": ["subnet-z9"] },
                "Description": "a second builder nobody creates",
            }),
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        let expected = already_exists_sentence(KIND, &name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        let read = emulator
            .ok(
                &case,
                "DescribeAppBlockBuilders",
                json!({ "Names": [name] }),
            )
            .await;
        let reported = builders(&case, &read);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the refused create must have added nothing, got {read}"
        );
        assert_eq!(
            reported[0], created,
            "case {case:?}: the stored builder must be unchanged ({created}), got {}",
            reported[0]
        );
    }
}

#[tokio::test]
async fn a_read_of_a_name_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "setup",
            "CreateAppBlockBuilder",
            json!({
                "Name": "stored-builder",
                "Platform": PLATFORM,
                "InstanceType": INSTANCE_TYPES[0],
                "VpcConfig": { "SubnetIds": ["subnet-a1", "subnet-b2"] },
            }),
        )
        .await;

    for name in names() {
        // Rows: the missing name alone, and the missing name beside one the account
        // holds — a read reports every name it was given or none of them.
        for request in [
            json!({ "Names": [name] }),
            json!({ "Names": ["stored-builder", name] }),
        ] {
            let case = format!("missing {name:?} in {request}");
            let message = refusal(
                &emulator,
                &case,
                "DescribeAppBlockBuilders",
                request,
                400,
                "ResourceNotFoundException",
            )
            .await;
            let expected = not_found_sentence(KIND, &name);
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn max_results_sizes_the_page_and_the_token_walks_every_builder_once() {
    /// The page `DescribeAppBlockBuilders` publishes: its `MaxResults` is documented as
    /// "the maximum size of each page of results. The maximum value is 25".
    const PUBLISHED_PAGE: usize = 25;

    let emulator = Emulator::new();
    let stored: BTreeSet<String> = (0..30).map(|index| format!("builder-{index:02}")).collect();
    for name in &stored {
        emulator
            .ok(
                "setup",
                "CreateAppBlockBuilder",
                json!({
                    "Name": name,
                    "Platform": PLATFORM,
                    "InstanceType": INSTANCE_TYPES[name.len() % INSTANCE_TYPES.len()],
                    "VpcConfig": { "SubnetIds": ["subnet-a1", "subnet-b2"] },
                }),
            )
            .await;
    }

    // Rows: each published bound of `MaxResults`, one past the upper bound, a size in
    // between, and a read that asks for no size at all.
    for max_results in [Some(1), Some(7), Some(PUBLISHED_PAGE), Some(100), None] {
        let case = format!("MaxResults {max_results:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({});
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = emulator
                .ok(&case, "DescribeAppBlockBuilders", request)
                .await;
            let reported = builders(&case, &page);
            let limit = max_results.map_or(PUBLISHED_PAGE, |size| size.min(PUBLISHED_PAGE));
            let is_last_page = page.get("NextToken").is_none();
            // Every page but the last carries the FULL page size, so the size is pinned
            // rather than merely bounded; the last carries what is left, at least one.
            if is_last_page {
                assert!(
                    !reported.is_empty() && reported.len() <= limit,
                    "case {case:?}: the last page must carry 1..={limit} builders, got {}",
                    reported.len()
                );
            } else {
                assert_eq!(
                    reported.len(),
                    limit,
                    "case {case:?}: a page before the last must carry exactly {limit} \
                     builders, got {}",
                    reported.len()
                );
            }
            walked.extend(
                reported
                    .iter()
                    .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string()),
            );
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            walked.len(),
            stored.len(),
            "case {case:?}: following the token must report every builder exactly once, \
             got {walked:?}"
        );
        assert_eq!(
            walked.iter().cloned().collect::<BTreeSet<String>>(),
            stored,
            "case {case:?}: expected {stored:?}, got {walked:?}"
        );
    }

    // A page of fewer than one record is not a size this read can answer with, and a
    // token this service never issued resumes nothing.
    for (case, request) in [
        ("MaxResults 0", json!({ "MaxResults": 0 })),
        ("MaxResults -1", json!({ "MaxResults": -1 })),
        ("a token nobody issued", json!({ "NextToken": "not-a-token" })),
    ] {
        refusal(
            &emulator,
            case,
            "DescribeAppBlockBuilders",
            request,
            400,
            "ValidationException",
        )
        .await;
    }
}

#[tokio::test]
async fn a_builders_tags_live_on_its_own_record() {
    // Rows: the tag maps a create can persist — `Tags` is length 1..50, and a value is
    // optional ("If you do not specify a value, the value is set to an empty string").
    let tag_rows = vec![
        json!({ "Environment": "Test" }),
        json!({ "Environment": "", "Owner": "platform", "cost.center/42": "a-b_c" }),
    ];

    for (index, tags) in tag_rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("tagged-builder-{index}");
        let case = format!("tags {tags}");
        let created = builder(
            &case,
            &emulator
                .ok(
                    &case,
                    "CreateAppBlockBuilder",
                    json!({
                        "Name": name,
                        "Platform": PLATFORM,
                        "InstanceType": INSTANCE_TYPES[0],
                        "VpcConfig": { "SubnetIds": ["subnet-a1", "subnet-b2"] },
                        "Tags": tags,
                    }),
                )
                .await,
        );
        let arn = created["Arn"].as_str().expect("a created builder has an ARN");
        assert_eq!(
            arn,
            builder_arn("us-east-1", &name),
            "case {case:?}: expected the builder's own ARN, got {arn}"
        );

        let listed = emulator
            .ok(
                &case,
                "ListTagsForResource",
                json!({ "ResourceArn": arn }),
            )
            .await;
        assert_eq!(
            listed.get("Tags"),
            Some(&tags),
            "case {case:?}: the create's tags must be the ones reported, got {listed}"
        );

        // The map the create persisted is the one a later write changes, so there is no
        // second copy to drift.
        emulator
            .ok(
                &case,
                "TagResource",
                json!({ "ResourceArn": arn, "Tags": { "Added": "yes" } }),
            )
            .await;
        let listed = emulator
            .ok(
                &case,
                "ListTagsForResource",
                json!({ "ResourceArn": arn }),
            )
            .await;
        let mut expected = tags.clone();
        expected["Added"] = json!("yes");
        assert_eq!(
            listed.get("Tags"),
            Some(&expected),
            "case {case:?}: expected {expected}, got {listed}"
        );

        emulator
            .ok(
                &case,
                "UntagResource",
                json!({ "ResourceArn": arn, "TagKeys": ["Added"] }),
            )
            .await;
        let listed = emulator
            .ok(
                &case,
                "ListTagsForResource",
                json!({ "ResourceArn": arn }),
            )
            .await;
        assert_eq!(
            listed.get("Tags"),
            Some(&tags),
            "case {case:?}: expected {tags}, got {listed}"
        );
    }

    // Tags belong TO a builder, so an ARN addressing none is refused with AWS's
    // sentence for that resource kind.
    let emulator = Emulator::new();
    for name in names() {
        let case = format!("tags of the absent {name:?}");
        let message = refusal(
            &emulator,
            &case,
            "ListTagsForResource",
            json!({ "ResourceArn": builder_arn("us-east-1", &name) }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        let expected = not_found_sentence(KIND, &name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }
}
