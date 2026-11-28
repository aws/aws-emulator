// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the parts of a resource share's STATE that live beside
//! its identity: its configuration, its tag set, its terminal state after a delete,
//! and the message a read reports for an ARN no share has.
//!
//! The rules under test, each asserted over a table of inputs rather than one case:
//!
//! * a `ResourceShare` reports its `resourceShareConfiguration` and its `tags` on
//!   every operation that returns one, with this service's defaults for a member the
//!   create did not name and the empty list for a share that carries no tags;
//! * `TagResource` and `UntagResource` change the stored share's tag set, so the next
//!   read reports exactly what they left behind;
//! * a share `DeleteResourceShare` removed keeps its record in the terminal `DELETED`
//!   state, so deleting it again is the same success and a read still reports it;
//! * every operation that takes a `resourceShareArn` reports an ARN no share has with
//!   one message template, built from the ARN the request named;
//! * an association that has not settled yet reports a `statusMessage` saying what it
//!   is waiting for, and reports none once it has settled.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    ASSOCIATION_READY_AFTER_MILLIS, MAX_TAGS_PER_SHARE,
};

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// The configuration a share created without an explicit one reports.
///
/// `ResourceShareConfiguration` declares one boolean member,
/// `retainSharingOnAccountLeaveOrganization`
/// (`smithy_query(query="shape",shape="ResourceShareConfiguration")`), and a share
/// does not retain sharing unless that was asked for.
fn default_configuration() -> Value {
    json!({
        "retainSharingOnAccountLeaveOrganization": false,
    })
}

/// A row of the configuration/tag-list projection table.
struct ProjectionCase {
    case: &'static str,
    /// The create request body.
    request: Value,
    /// The `resourceShareConfiguration` every read of the share must report.
    configuration: Value,
    /// The tag pairs every read must report, in key order.
    tags: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn every_reported_share_carries_its_configuration_and_its_tag_list() {
    let router = emulator();

    // The rule generalises over which configuration members the request names and how
    // many tags it carries, so the rows differ along exactly that: no configuration at
    // all, an empty configuration structure, each published value of the one member,
    // and zero, one or several tags.
    let cases = vec![
        ProjectionCase {
            case: "no configuration and no tags",
            request: json!({ "name": "projection-bare" }),
            configuration: default_configuration(),
            tags: vec![],
        },
        ProjectionCase {
            case: "an empty configuration structure",
            request: json!({
                "name": "projection-empty-configuration",
                "resourceShareConfiguration": {},
            }),
            configuration: default_configuration(),
            tags: vec![],
        },
        ProjectionCase {
            case: "retention asked for",
            request: json!({
                "name": "projection-retain",
                "resourceShareConfiguration": {
                    "retainSharingOnAccountLeaveOrganization": true,
                },
                "tags": [{ "key": "team", "value": "platform" }],
            }),
            configuration: json!({
                "retainSharingOnAccountLeaveOrganization": true,
            }),
            tags: vec![("team", "platform")],
        },
        ProjectionCase {
            case: "retention asked for beside several tags",
            request: json!({
                "name": "projection-retain-tagged",
                "resourceShareConfiguration": {
                    "retainSharingOnAccountLeaveOrganization": true,
                },
                "tags": [
                    { "key": "cost-center", "value": "7" },
                    { "key": "team", "value": "network" },
                ],
            }),
            configuration: json!({
                "retainSharingOnAccountLeaveOrganization": true,
            }),
            tags: vec![("cost-center", "7"), ("team", "network")],
        },
        ProjectionCase {
            case: "retention spelled out as the default",
            request: json!({
                "name": "projection-no-retain",
                "resourceShareConfiguration": {
                    "retainSharingOnAccountLeaveOrganization": false,
                },
                "tags": [
                    { "key": "cost-center", "value": "42" },
                    { "key": "team", "value": "platform" },
                ],
            }),
            configuration: json!({
                "retainSharingOnAccountLeaveOrganization": false,
            }),
            tags: vec![("cost-center", "42"), ("team", "platform")],
        },
        ProjectionCase {
            case: "an explicitly empty tag list",
            request: json!({
                "name": "projection-empty-tags",
                "tags": [],
                "resourceArns": [LOCAL_RESOURCE],
            }),
            configuration: default_configuration(),
            tags: vec![],
        },
    ];

    for ProjectionCase {
        case,
        request,
        configuration,
        tags,
    } in cases
    {
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        let created = create_ok(&router, case, &request).await;
        let arn = arn_of(case, &created);
        assert_eq!(
            created
                .get("resourceShareConfiguration")
                .cloned()
                .unwrap_or(Value::Null),
            configuration,
            "case {case:?}: create reported the wrong resourceShareConfiguration, got {created:?}"
        );
        assert_eq!(
            tags_of(case, &created),
            expected_tags,
            "case {case:?}: create reported the wrong tags, got {created:?}"
        );

        // Every read path reports the same projection, however the share is selected.
        let name = created
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        for (selector, body) in [
            ("by arn", json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] })),
            ("by name", json!({ "resourceOwner": "SELF", "name": name })),
            (
                "by status",
                json!({
                    "resourceOwner": "SELF",
                    "name": name,
                    "resourceShareStatus": "ACTIVE",
                }),
            ),
        ] {
            let (status, listed) = get_shares(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{selector}: expected 200, got {status} with {listed:?}"
            );
            let share = only_share(case, &listed);
            assert_eq!(
                share
                    .get("resourceShareConfiguration")
                    .cloned()
                    .unwrap_or(Value::Null),
                configuration,
                "case {case:?}/{selector}: wrong resourceShareConfiguration, got {share:?}"
            );
            assert_eq!(
                tags_of(case, &share),
                expected_tags,
                "case {case:?}/{selector}: wrong tags, got {share:?}"
            );
        }

        // A rename changes the name and nothing else: the configuration and the tag
        // set the create stored are still what the update reports.
        let renamed = format!("{name}-renamed");
        let (status, updated) = update(
            &router,
            &json!({ "resourceShareArn": arn, "name": renamed }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} with {updated:?}"
        );
        let share = updated
            .get("resourceShare")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: update reported no resourceShare"));
        assert_eq!(
            share
                .get("resourceShareConfiguration")
                .cloned()
                .unwrap_or(Value::Null),
            configuration,
            "case {case:?}: an update must not change the configuration, got {share:?}"
        );
        assert_eq!(
            tags_of(case, &share),
            expected_tags,
            "case {case:?}: an update must not change the tags, got {share:?}"
        );
    }
}

/// One tag mutation applied to a share: either `TagResource` with pairs, or
/// `UntagResource` with keys.
enum TagStep {
    Add(Vec<(&'static str, &'static str)>),
    Remove(Vec<&'static str>),
}

/// A row of the tag-mutation table.
struct TagCase {
    case: &'static str,
    /// The tags the share is created with.
    initial: Vec<(&'static str, &'static str)>,
    /// The mutations applied, in order.
    steps: Vec<TagStep>,
    /// The tag pairs the share must report afterwards, in key order.
    expected: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn a_tag_mutation_is_what_the_next_read_reports() {
    let router = emulator();

    // The rule is "the stored tag set is what the mutations left behind", so the rows
    // differ along what a mutation can do to a set: add to an empty one, add beside an
    // existing key, replace a key's value, add several at once, add an empty value,
    // remove one of several, remove a key that was never there, remove everything, and
    // add then remove the same key.
    let cases = vec![
        TagCase {
            case: "add one tag to an untagged share",
            initial: vec![],
            steps: vec![TagStep::Add(vec![("cost-center", "42")])],
            expected: vec![("cost-center", "42")],
        },
        TagCase {
            case: "add a tag beside an existing one",
            initial: vec![("team", "platform")],
            steps: vec![TagStep::Add(vec![("cost-center", "42")])],
            expected: vec![("cost-center", "42"), ("team", "platform")],
        },
        TagCase {
            case: "add a key the share already carries",
            initial: vec![("team", "platform")],
            steps: vec![TagStep::Add(vec![("team", "network")])],
            expected: vec![("team", "network")],
        },
        TagCase {
            case: "add several tags in one call",
            initial: vec![("team", "platform")],
            steps: vec![TagStep::Add(vec![
                ("cost-center", "42"),
                ("env", "test"),
                ("owner", ""),
            ])],
            expected: vec![
                ("cost-center", "42"),
                ("env", "test"),
                ("owner", ""),
                ("team", "platform"),
            ],
        },
        TagCase {
            case: "add the same tag twice",
            initial: vec![],
            steps: vec![
                TagStep::Add(vec![("env", "test")]),
                TagStep::Add(vec![("env", "test")]),
            ],
            expected: vec![("env", "test")],
        },
        TagCase {
            case: "remove one of two keys",
            initial: vec![("cost-center", "42"), ("team", "platform")],
            steps: vec![TagStep::Remove(vec!["team"])],
            expected: vec![("cost-center", "42")],
        },
        TagCase {
            case: "remove a key the share never carried",
            initial: vec![("team", "platform")],
            steps: vec![TagStep::Remove(vec!["absent"])],
            expected: vec![("team", "platform")],
        },
        TagCase {
            case: "remove every key",
            initial: vec![("cost-center", "42"), ("team", "platform")],
            steps: vec![TagStep::Remove(vec!["cost-center", "team"])],
            expected: vec![],
        },
        TagCase {
            case: "remove the same key twice",
            initial: vec![("team", "platform")],
            steps: vec![
                TagStep::Remove(vec!["team"]),
                TagStep::Remove(vec!["team"]),
            ],
            expected: vec![],
        },
        TagCase {
            case: "add then remove the added key",
            initial: vec![("team", "platform")],
            steps: vec![
                TagStep::Add(vec![("cost-center", "42")]),
                TagStep::Remove(vec!["cost-center"]),
            ],
            expected: vec![("team", "platform")],
        },
    ];

    for TagCase {
        case,
        initial,
        steps,
        expected,
    } in cases
    {
        let initial_tags: Vec<Value> = initial
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect();
        let created = create_ok(
            &router,
            case,
            &json!({ "name": format!("tagged-{}", case.replace(' ', "-")), "tags": initial_tags }),
        )
        .await;
        let arn = arn_of(case, &created);

        for (index, step) in steps.iter().enumerate() {
            let (status, body) = match step {
                TagStep::Add(pairs) => {
                    let tags: Vec<Value> = pairs
                        .iter()
                        .map(|(key, value)| json!({ "key": key, "value": value }))
                        .collect();
                    tag_resource(
                        &router,
                        &json!({ "resourceShareArn": arn, "tags": tags }),
                    )
                    .await
                }
                TagStep::Remove(keys) => {
                    untag_resource(
                        &router,
                        &json!({ "resourceShareArn": arn, "tagKeys": keys }),
                    )
                    .await
                }
            };
            assert_eq!(
                status, 200,
                "case {case:?}/step {index}: the mutation expected 200, got {status} with {body:?}"
            );
            // `TagResourceResponse` and `UntagResourceResponse` declare no members, so a
            // successful mutation answers with an empty body; what it changed is read
            // back below through `GetResourceShares`.
            assert_eq!(
                body,
                json!({}),
                "case {case:?}/step {index}: expected an empty mutation body, got {body:?}"
            );
        }

        let expected_tags: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let (status, listed) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read-back expected 200, got {status} with {listed:?}"
        );
        let share = only_share(case, &listed);
        assert_eq!(
            tags_of(case, &share),
            expected_tags,
            "case {case:?}: the share reports the wrong tag set, got {share:?}"
        );

        // A tag the share carries is also what a `tagFilters` read selects on, so the
        // mutation reached the one stored record rather than a copy.
        for (key, value) in &expected_tags {
            let (_, filtered) = get_shares(
                &router,
                &json!({
                    "resourceOwner": "SELF",
                    "tagFilters": [{ "tagKey": key, "tagValues": [value] }],
                }),
            )
            .await;
            assert!(
                shares_of(case, &filtered)
                    .iter()
                    .any(|found| arn_of(case, found) == arn),
                "case {case:?}: tagFilters on {key}={value} did not select the share, got \
                 {filtered:?}"
            );
        }
    }
}

/// A row of the refusal table: a request that must be refused, and the modelled error
/// shape RAM words that refusal with.
struct RefusalCase {
    case: String,
    uri: &'static str,
    request: Value,
    expected: &'static str,
}

#[tokio::test]
async fn a_tag_mutation_the_service_cannot_apply_is_refused_as_ram_words_it() {
    let router = emulator();
    let created = create_ok(
        &router,
        "refusals",
        &json!({ "name": "share-refusals", "tags": [{ "key": "team", "value": "platform" }] }),
    )
    .await;
    let arn = arn_of("refusals", &created);

    // A share holding RAM's whole per-share tag allowance, so one more key is one too
    // many.
    let full_tags: Vec<Value> = (0..MAX_TAGS_PER_SHARE)
        .map(|index| json!({ "key": format!("key-{index}"), "value": "v" }))
        .collect();
    let full = create_ok(
        &router,
        "refusals",
        &json!({ "name": "share-full-of-tags", "tags": full_tags }),
    )
    .await;
    let full_arn = arn_of("refusals", &full);

    // Well-formed, and no share has it.
    let absent_arn =
        format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/00000000-0000-4000-8000-000000000000");
    // A managed-permission ARN: what `resourceArn` names on both operations.
    let permission_arn = "arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSubnet";

    let cases = vec![
        RefusalCase {
            case: "tag naming both ARN members".to_string(),
            uri: "/tagresource",
            request: json!({
                "resourceShareArn": arn,
                "resourceArn": permission_arn,
                "tags": [{ "key": "env", "value": "test" }],
            }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "tag naming neither ARN member".to_string(),
            uri: "/tagresource",
            request: json!({ "tags": [{ "key": "env", "value": "test" }] }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "tag a managed permission".to_string(),
            uri: "/tagresource",
            request: json!({
                "resourceArn": permission_arn,
                "tags": [{ "key": "env", "value": "test" }],
            }),
            expected: "ResourceArnNotFoundException",
        },
        RefusalCase {
            case: "tag a resourceArn that is not an ARN".to_string(),
            uri: "/tagresource",
            request: json!({
                "resourceArn": "not-an-arn",
                "tags": [{ "key": "env", "value": "test" }],
            }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "tag a share ARN that is not one".to_string(),
            uri: "/tagresource",
            request: json!({
                "resourceShareArn": "arn:aws:ram:us-west-2:000000000000:not-a-share/x",
                "tags": [{ "key": "env", "value": "test" }],
            }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "tag a share no account has".to_string(),
            uri: "/tagresource",
            request: json!({
                "resourceShareArn": absent_arn,
                "tags": [{ "key": "env", "value": "test" }],
            }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "tag with a reserved key prefix".to_string(),
            uri: "/tagresource",
            request: json!({
                "resourceShareArn": arn,
                "tags": [{ "key": "aws:created-by", "value": "x" }],
            }),
            expected: "TagPolicyViolationException",
        },
        RefusalCase {
            case: "tag with a key that is the empty string".to_string(),
            uri: "/tagresource",
            request: json!({
                "resourceShareArn": arn,
                "tags": [{ "key": "", "value": "x" }],
            }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: format!("tag a share past the {MAX_TAGS_PER_SHARE}-tag limit"),
            uri: "/tagresource",
            request: json!({
                "resourceShareArn": full_arn,
                "tags": [{ "key": "one-too-many", "value": "x" }],
            }),
            expected: "TagLimitExceededException",
        },
        RefusalCase {
            case: "untag naming both ARN members".to_string(),
            uri: "/untagresource",
            request: json!({
                "resourceShareArn": arn,
                "resourceArn": permission_arn,
                "tagKeys": ["team"],
            }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "untag naming neither ARN member".to_string(),
            uri: "/untagresource",
            request: json!({ "tagKeys": ["team"] }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "untag a managed permission".to_string(),
            uri: "/untagresource",
            request: json!({ "resourceArn": permission_arn, "tagKeys": ["team"] }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "untag a resourceArn that is not an ARN".to_string(),
            uri: "/untagresource",
            request: json!({ "resourceArn": "not-an-arn", "tagKeys": ["team"] }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "untag a share no account has".to_string(),
            uri: "/untagresource",
            request: json!({ "resourceShareArn": absent_arn, "tagKeys": ["team"] }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "untag with a key that is the empty string".to_string(),
            uri: "/untagresource",
            request: json!({ "resourceShareArn": arn, "tagKeys": [""] }),
            expected: "InvalidParameterException",
        },
    ];

    for RefusalCase {
        case,
        uri,
        request,
        expected,
    } in cases
    {
        let (status, body) = call(&router, "POST", uri, Some(&request)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected),
            "case {case:?}: wrong refusal for {request}, got {body:?}"
        );
    }

    // A refusal changes nothing: the share still carries exactly the tag it was
    // created with.
    let (_, listed) = get_shares(
        &router,
        &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
    )
    .await;
    assert_eq!(
        tags_of("refusals", &only_share("refusals", &listed)),
        vec![("team".to_string(), "platform".to_string())],
        "case \"refusals\": a refused mutation must leave the tag set alone, got {listed:?}"
    );
}

#[tokio::test]
async fn a_deleted_share_keeps_its_record_and_a_repeat_delete_is_the_same_success() {
    let router = emulator();

    // The rule generalises over how the delete is issued and how often, so the rows
    // differ along the number of deletes and whether they carry a client token.
    for (case, repeats, token) in [
        ("one delete", 1usize, None),
        ("two deletes", 2usize, None),
        ("three deletes", 3usize, None),
        ("two deletes, one token", 2usize, Some("token-shared")),
        ("two deletes, fresh tokens", 2usize, Some("token-fresh")),
    ] {
        let created = create_ok(
            &router,
            case,
            &json!({
                "name": format!("deleted-{}", case.replace([' ', ','], "-")),
                "resourceArns": [LOCAL_RESOURCE],
                "principals": ["444455556666"],
                "tags": [{ "key": "team", "value": "platform" }],
                "resourceShareConfiguration": {
                    "retainSharingOnAccountLeaveOrganization": true,
                },
            }),
        )
        .await;
        let arn = arn_of(case, &created);

        for attempt in 0..repeats {
            let client_token = token.map(|base| match base {
                // One token reused across both calls, versus a different one per call:
                // either way the answer is the success the first call reported.
                "token-shared" => format!("{base}-{case}"),
                _ => format!("{base}-{case}-{attempt}"),
            });
            let (status, body) = delete(&router, &arn, client_token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?}/attempt {attempt}: delete expected 200, got {status} with {body:?}"
            );
            assert_eq!(
                body.get("returnValue").and_then(Value::as_bool),
                Some(true),
                "case {case:?}/attempt {attempt}: expected returnValue true, got {body:?}"
            );
        }

        // The record survives, in the terminal state, with everything it carried.
        for (selector, request) in [
            ("no status", json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] })),
            (
                "status DELETED",
                json!({
                    "resourceOwner": "SELF",
                    "resourceShareArns": [arn],
                    "resourceShareStatus": "DELETED",
                }),
            ),
        ] {
            let (status, listed) = get_shares(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{selector}: expected 200, got {status} with {listed:?}"
            );
            let share = only_share(case, &listed);
            assert_eq!(
                share.get("status").and_then(Value::as_str),
                Some("DELETED"),
                "case {case:?}/{selector}: expected the DELETED record, got {share:?}"
            );
            assert_eq!(
                tags_of(case, &share),
                vec![("team".to_string(), "platform".to_string())],
                "case {case:?}/{selector}: the record must keep its tags, got {share:?}"
            );
            assert_eq!(
                share
                    .get("resourceShareConfiguration")
                    .cloned()
                    .unwrap_or(Value::Null),
                json!({
                    "retainSharingOnAccountLeaveOrganization": true,
                }),
                "case {case:?}/{selector}: the record must keep its configuration, got {share:?}"
            );
            assert_eq!(
                share.get("owningAccountId").and_then(Value::as_str),
                Some(ACCOUNT),
                "case {case:?}/{selector}: the record must keep its owner, got {share:?}"
            );
            assert_eq!(
                share.get("featureSet").and_then(Value::as_str),
                Some("STANDARD"),
                "case {case:?}/{selector}: the record must keep its featureSet, got {share:?}"
            );
        }

        // And it is no longer selected by the status it left.
        let (_, active) = get_shares(
            &router,
            &json!({
                "resourceOwner": "SELF",
                "resourceShareArns": [arn],
                "resourceShareStatus": "ACTIVE",
            }),
        )
        .await;
        assert!(
            shares_of(case, &active).is_empty(),
            "case {case:?}: a deleted share is not ACTIVE any more, got {active:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_resource_share_arn_is_reported_with_one_message_template() {
    let router = emulator();

    // Every row is a well-formed resource-share ARN no share has; they differ along
    // the fields an ARN is made of, so the message is built from the ARN the request
    // named rather than from one remembered string.
    let unknown_arns = [
        format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/00000000-0000-0000-0000-000000000000"),
        format!("arn:aws:ram:eu-central-1:{ACCOUNT}:resource-share/11111111-2222-4333-8444-555555555555"),
        "arn:aws:ram:us-east-1:111122223333:resource-share/deadbeef-dead-4eef-8eef-deadbeefdead"
            .to_string(),
        format!("arn:aws-us-gov:ram:us-gov-west-1:{ACCOUNT}:resource-share/abc"),
    ];

    for arn in unknown_arns {
        // The template, as the four operations the recording covers all render it.
        let expected = format!("ResourceShare {arn} could not be found.");

        let calls: Vec<(&str, (u16, Value))> = vec![
            (
                "GetResourceShares",
                get_shares(
                    &router,
                    &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
                )
                .await,
            ),
            (
                "UpdateResourceShare",
                update(
                    &router,
                    &json!({ "resourceShareArn": arn, "name": "renamed" }),
                )
                .await,
            ),
            ("DeleteResourceShare", delete(&router, &arn, None).await),
            (
                "AssociateResourceShare",
                associate(
                    &router,
                    &json!({ "resourceShareArn": arn, "principals": ["444455556666"] }),
                )
                .await,
            ),
            (
                "GetResourceShareAssociations",
                get_associations(
                    &router,
                    &json!({ "associationType": "PRINCIPAL", "resourceShareArns": [arn] }),
                )
                .await,
            ),
            (
                "TagResource",
                tag_resource(
                    &router,
                    &json!({
                        "resourceShareArn": arn,
                        "tags": [{ "key": "env", "value": "test" }],
                    }),
                )
                .await,
            ),
            (
                "UntagResource",
                untag_resource(
                    &router,
                    &json!({ "resourceShareArn": arn, "tagKeys": ["env"] }),
                )
                .await,
            ),
        ];

        for (operation, (status, body)) in calls {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "UnknownResourceException"),
                "case {operation:?}: wrong refusal for {arn}, got {body:?}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {operation:?}: wrong message for {arn}, got {body:?}"
            );
        }
    }
}

/// A row of the association status-message table: one entity to associate, and
/// whether its association reports what it is waiting for while it is pending.
struct WaitCase {
    association_type: &'static str,
    entity: &'static str,
    /// True when RAM invites the principal, so a pending association says it is
    /// waiting for that invitation to be answered.
    waits_for_invitation: bool,
}

#[tokio::test]
async fn a_pending_principal_association_reports_what_it_is_waiting_for() {
    let router = emulator();
    let case = "association wait";

    // The rule generalises over WHAT an association is waiting for, so the rows
    // differ along the association type and the form of the entity: a resource
    // inside and outside the owning account, an account id that is and is not the
    // owner's, an ARN of a role in another account, a service principal name, an
    // organizational unit and a source. Every PRINCIPAL association is invited and
    // so reports the wait; a resource or a source association names no principal and
    // reports nothing.
    let cases = vec![
        WaitCase {
            association_type: "RESOURCE",
            entity: LOCAL_RESOURCE,
            waits_for_invitation: false,
        },
        WaitCase {
            association_type: "RESOURCE",
            entity: FOREIGN_RESOURCE,
            waits_for_invitation: false,
        },
        WaitCase {
            association_type: "PRINCIPAL",
            entity: "444455556666",
            waits_for_invitation: true,
        },
        WaitCase {
            association_type: "PRINCIPAL",
            entity: ACCOUNT,
            waits_for_invitation: true,
        },
        WaitCase {
            association_type: "PRINCIPAL",
            entity: "arn:aws:iam::777788889999:role/consumer",
            waits_for_invitation: true,
        },
        WaitCase {
            association_type: "PRINCIPAL",
            entity: "lambda.amazonaws.com",
            waits_for_invitation: true,
        },
        WaitCase {
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::000000000000:ou/o-abc/ou-def",
            waits_for_invitation: true,
        },
        WaitCase {
            association_type: "SOURCE",
            entity: "111122223333",
            waits_for_invitation: false,
        },
    ];

    let entities = |wanted: &str| -> Vec<&'static str> {
        cases
            .iter()
            .filter(|row| row.association_type == wanted)
            .map(|row| row.entity)
            .collect()
    };
    let created = create_ok(
        &router,
        case,
        &json!({
            "name": "share-association-wait",
            "resourceArns": entities("RESOURCE"),
            "principals": entities("PRINCIPAL"),
            "sources": entities("SOURCE"),
        }),
    )
    .await;
    let arn = arn_of(case, &created);

    /// Assert every row's association, as the reads report it.
    async fn check(
        router: &smithy_server_runtime::MultiServiceRouter,
        case: &str,
        arn: &str,
        cases: &[WaitCase],
        settled: bool,
    ) {
        for row in cases {
            let label = format!("{} {}", row.association_type, row.entity);
            // Each association type takes its own entity filter: `principal` for a
            // PRINCIPAL association, `resourceArn` for a RESOURCE one, and neither
            // for a SOURCE one.
            let mut request = json!({
                "associationType": row.association_type,
                "resourceShareArns": [arn],
            });
            match row.association_type {
                "PRINCIPAL" => {
                    request["principal"] = Value::String(row.entity.to_string());
                }
                "RESOURCE" => {
                    request["resourceArn"] = Value::String(row.entity.to_string());
                }
                _ => {}
            }
            let (status, found) = get_associations(router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{label}: expected 200, got {status} with {found:?}"
            );
            let reported: Vec<Value> = associations_of(case, &found)
                .into_iter()
                .filter(|association| {
                    association.get("associatedEntity").and_then(Value::as_str)
                        == Some(row.entity)
                })
                .collect();
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}/{label}: expected exactly this one association, got {found:?}"
            );
            let association = &reported[0];
            // Every association is owed the move to ASSOCIATED, so once the clock
            // has made it due nothing is pending any more.
            let expected_status = if settled { "ASSOCIATED" } else { "ASSOCIATING" };
            assert_eq!(
                association.get("status").and_then(Value::as_str),
                Some(expected_status),
                "case {case:?}/{label}: expected {expected_status}, got {association:?}"
            );
            let message = association.get("statusMessage").and_then(Value::as_str);
            if row.waits_for_invitation && !settled {
                assert!(
                    message.is_some_and(|message| message.contains("invitation")),
                    "case {case:?}/{label}: a pending principal association must say it is \
                     waiting for the invitation to be answered, got {association:?}"
                );
            } else {
                assert_eq!(
                    message, None,
                    "case {case:?}/{label}: nothing is being waited for, so no statusMessage \
                     belongs here, got {association:?}"
                );
            }
        }
    }

    // Straight after the create, nothing has settled yet: the owed span is never zero.
    check(&router, case, &arn, &cases, false).await;

    // Nothing sleeps server-side; the move is materialised by the first call at or
    // after the instant it fell due.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(ASSOCIATION_READY_AFTER_MILLIS).unwrap_or_default() + 100,
    ));
    check(&router, case, &arn, &cases, true).await;
}
