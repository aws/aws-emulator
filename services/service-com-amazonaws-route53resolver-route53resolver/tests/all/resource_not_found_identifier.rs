// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH identifier the service names in the one sentence
//! it answers for a resource the caller named but cannot address.
//!
//! The rule under test: that sentence names the resource by its bare ID — the
//! resource-id component of whatever the request submitted — so an operation
//! addressed by an ARN and its sibling addressed by the bare id answer the very
//! same message, and no ARN prefix (partition, service, region, account or
//! resource type) ever appears inside the quotes.
//!
//! The table spans what the rule generalises over: the form and the length of
//! the id (both bounds the model's `ResourceId` fixes, length 1..64, are rows),
//! the partition, region and account the ARN is rooted in, the resource type its
//! ARN names, and an identifier that is not an ARN at all. Each row is driven
//! through every operation that resolves such an identifier — the two
//! ARN-addressed rule-group policy operations and the id-addressed rule-group
//! operations — so the wording cannot depend on how the resource was addressed.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::RSLVR_CODE_RESOURCE_NOT_FOUND;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member, traced_message, ACCOUNT, REGION};

/// An account that is not the one this emulator's resources belong to, so a
/// policy document naming it really does ask for a share.
const OTHER_ACCOUNT: &str = "123456789012";

/// The whole message the service answers for a resource the caller cannot
/// address, written out here rather than built from the emulator's own message
/// builder so the wording itself is what this suite pins: the service error code
/// in square brackets, the sentence with the resource's bare ID double-quoted
/// inside it, and the closing period.
fn expected_message(resource_id: &str) -> String {
    format!("[{RSLVR_CODE_RESOURCE_NOT_FOUND}] Can’t find the resource with ID \"{resource_id}\".")
}

/// A grammatically valid document that shares `arn` with another account, so a
/// policy write reaches the identifier resolution instead of being refused for
/// the document it carries.
fn sharing_document(arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": OTHER_ACCOUNT },
            "Action": "route53resolver:GetFirewallRuleGroup",
            "Resource": arn,
        }],
    })
    .to_string()
}

/// An ARN of this service rooted in `partition`, `region` and `account`, naming
/// the resource `resource_type`/`resource_id`.
fn resolver_arn(
    partition: &str,
    region: &str,
    account: &str,
    resource_type: &str,
    resource_id: &str,
) -> String {
    format!("arn:{partition}:route53resolver:{region}:{account}:{resource_type}/{resource_id}")
}

/// One row: how the request addressed a rule group nothing was created under,
/// and the bare id the refusal has to name for it.
struct Row {
    case: &'static str,
    /// The `FirewallRuleGroupId` the id-addressed operations are given, within
    /// the model's `ResourceId` length 1..64.
    id: String,
    /// The `Arn` the policy operations are given, naming the same resource.
    arn: String,
}

/// The identifiers no rule group is stored under, spanning the forms the rule
/// generalises over.
fn rows() -> Vec<Row> {
    let typical = "rslvr-frg-9f8e7d6c5b4a3210".to_string();
    // `ResourceId` is length 1..64 (smithy_query(query="shape",shape="ResourceId")),
    // so a one-character id and a 64-character id are its two bounds.
    let shortest = "z".to_string();
    let longest = format!("rslvr-frg-{}", "f".repeat(54));
    assert_eq!(
        longest.len(),
        64,
        "the longest row must sit on ResourceId's upper bound, got {}",
        longest.len()
    );

    vec![
        Row {
            case: "the id form the service mints, in this region and account",
            arn: resolver_arn(
                "aws",
                REGION,
                ACCOUNT,
                "firewall-rule-group",
                &typical,
            ),
            id: typical.clone(),
        },
        Row {
            case: "the shortest id the model admits",
            arn: resolver_arn(
                "aws",
                REGION,
                ACCOUNT,
                "firewall-rule-group",
                &shortest,
            ),
            id: shortest,
        },
        Row {
            case: "the longest id the model admits",
            arn: resolver_arn("aws", REGION, ACCOUNT, "firewall-rule-group", &longest),
            id: longest,
        },
        Row {
            case: "an ARN in another partition",
            arn: resolver_arn(
                "aws-us-gov",
                "us-gov-west-1",
                ACCOUNT,
                "firewall-rule-group",
                &typical,
            ),
            id: typical.clone(),
        },
        Row {
            case: "an ARN in another region and another account",
            arn: resolver_arn(
                "aws-cn",
                "cn-north-1",
                OTHER_ACCOUNT,
                "firewall-rule-group",
                &typical,
            ),
            id: typical.clone(),
        },
        Row {
            case: "an ARN naming another resource type",
            arn: resolver_arn(
                "aws",
                REGION,
                ACCOUNT,
                "firewall-domain-list",
                &typical,
            ),
            id: typical.clone(),
        },
        Row {
            case: "an id carrying no service prefix at all",
            arn: resolver_arn("aws", REGION, ACCOUNT, "firewall-rule-group", "group-17"),
            id: "group-17".to_string(),
        },
        Row {
            case: "the bare id submitted where an ARN is expected",
            arn: typical.clone(),
            id: typical,
        },
    ]
}

/// Every operation that resolves a rule group the store does not hold answers
/// the one sentence, naming the bare id — whether it was addressed by ARN or by
/// id.
#[tokio::test]
async fn the_not_found_sentence_names_the_bare_id_however_the_resource_was_addressed() {
    let router = emulator();

    for Row { case, id, arn } in rows() {
        let expected = expected_message(&id);

        // Addressed by ARN: the two rule-group policy operations.
        for (operation, body) in [
            (
                "GetFirewallRuleGroupPolicy",
                json!({ "Arn": arn.clone() }),
            ),
            (
                "PutFirewallRuleGroupPolicy",
                json!({
                    "Arn": arn.clone(),
                    "FirewallRuleGroupPolicy": sharing_document(&arn),
                }),
            ),
        ] {
            let message = not_found_message(&router, case, operation, body).await;
            assert_eq!(
                message, expected,
                "case {case:?}: {operation} addressed by {arn:?} must name the bare id, got \
                 {message:?}"
            );
            assert_quotes_hold_only_the_id(case, operation, &id, &message);
        }

        // Addressed by the bare id: the rule-group operations that take a
        // `FirewallRuleGroupId`. AWS words this refusal identically for them,
        // so the message must be the very same string.
        for (operation, body) in [
            (
                "GetFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id.clone() }),
            ),
            (
                "DeleteFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id.clone() }),
            ),
            (
                "ListFirewallRules",
                json!({ "FirewallRuleGroupId": id.clone() }),
            ),
        ] {
            let message = not_found_message(&router, case, operation, body).await;
            assert_eq!(
                message, expected,
                "case {case:?}: {operation} addressed by {id:?} must answer the same sentence, \
                 got {message:?}"
            );
            assert_quotes_hold_only_the_id(case, operation, &id, &message);
        }
    }
}

/// The same rule over the other resource types this service refuses by the one
/// sentence: a domain list, a rule-group association, and the tag operations,
/// which address ANY resource of the service by its ARN.
#[tokio::test]
async fn the_sentence_names_the_bare_id_for_every_resource_type() {
    let router = emulator();

    // (the resource type an ARN names, the id form that type carries)
    let resources = [
        ("firewall-rule-group", "rslvr-frg-5a4b3c2d1e0f9876"),
        ("firewall-domain-list", "rslvr-fdl-5a4b3c2d1e0f9876"),
        ("resolver-rule", "rslvr-rr-5a4b3c2d1e0f9876"),
        ("resolver-endpoint", "rslvr-out-5a4b3c2d1e0f9876"),
        ("resolver-query-log-config", "rslvr-qlc-5a4b3c2d1e0f9876"),
    ];

    for (resource_type, id) in resources {
        let case = resource_type;
        let arn = resolver_arn("aws", REGION, ACCOUNT, resource_type, id);
        let expected = expected_message(id);

        for (operation, body) in [
            (
                "ListTagsForResource",
                json!({ "ResourceArn": arn.clone() }),
            ),
            (
                "TagResource",
                json!({
                    "ResourceArn": arn.clone(),
                    "Tags": [{ "Key": "team", "Value": "net" }],
                }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": arn.clone(), "TagKeys": ["team"] }),
            ),
        ] {
            let message = not_found_message(&router, case, operation, body).await;
            assert_eq!(
                message, expected,
                "case {case:?}: {operation} addressed by {arn:?} must name the bare id, got \
                 {message:?}"
            );
            assert_quotes_hold_only_the_id(case, operation, id, &message);
        }
    }

    // The id-addressed readers of the other DNS Firewall resources word it the
    // same way. The rule-group association's own refusal closes with the trace
    // clause of the request (see [`super::firewall_association_error_bodies`]),
    // so what is compared for it is the sentence before that clause: the
    // identifier it names is the same bare id either way.
    let message = not_found_message(
        &router,
        "FirewallDomainListId",
        "GetFirewallDomainList",
        json!({ "FirewallDomainListId": "rslvr-fdl-0f1e2d3c4b5a6978" }),
    )
    .await;
    assert_eq!(
        message,
        expected_message("rslvr-fdl-0f1e2d3c4b5a6978"),
        "case \"FirewallDomainListId\": GetFirewallDomainList must name the bare id, got \
         {message:?}"
    );
    assert_quotes_hold_only_the_id(
        "FirewallDomainListId",
        "GetFirewallDomainList",
        "rslvr-fdl-0f1e2d3c4b5a6978",
        &message,
    );

    let association = "rslvr-frgassoc-0f1e2d3c4b5a6978";
    let message = not_found_message(
        &router,
        "FirewallRuleGroupAssociationId",
        "GetFirewallRuleGroupAssociation",
        json!({ "FirewallRuleGroupAssociationId": association }),
    )
    .await;
    let (stated, _) = traced_message(&message).unwrap_or_else(|| {
        panic!(
            "case \"FirewallRuleGroupAssociationId\": the refusal must close with its trace \
             clause, got {message:?}"
        )
    });
    assert_eq!(
        stated,
        expected_message(association),
        "case \"FirewallRuleGroupAssociationId\": GetFirewallRuleGroupAssociation must name the \
         bare id, got {message:?}"
    );
    assert_quotes_hold_only_the_id(
        "FirewallRuleGroupAssociationId",
        "GetFirewallRuleGroupAssociation",
        association,
        stated,
    );
}

/// The reproduction a client hits: a rule group that DOES exist, addressed by an
/// ARN whose last segment was rewritten to an id nothing was created under. The
/// refusal names that id, the live group is still served, and the two messages
/// differ only in the id they name.
#[tokio::test]
async fn an_arn_rewritten_to_an_unknown_id_names_that_id_and_leaves_the_live_group_served() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": "not-found-identifier", "Name": "not-found-identifier" }),
    )
    .await;
    let group = created
        .get("FirewallRuleGroup")
        .expect("the create answers a FirewallRuleGroup");
    let live_arn = string_member(group, "Arn");
    let live_id = string_member(group, "Id");

    for unknown_id in ["rslvr-frg-ffffffffffffffff", "rslvr-frg-1"] {
        let case = unknown_id;
        let prefix = live_arn
            .rsplit_once('/')
            .map(|(prefix, _)| prefix)
            .unwrap_or(&live_arn);
        let rewritten = format!("{prefix}/{unknown_id}");
        let message = not_found_message(
            &router,
            case,
            "GetFirewallRuleGroupPolicy",
            json!({ "Arn": rewritten.clone() }),
        )
        .await;
        assert_eq!(
            message,
            expected_message(unknown_id),
            "case {case:?}: the refusal for {rewritten:?} must name the id, got {message:?}"
        );
        assert!(
            !message.contains(prefix) && !message.contains(&live_id),
            "case {case:?}: the refusal must carry no ARN prefix and no other id, got {message:?}"
        );
    }

    // The group that does exist is still served by the same operation, so the
    // refusal above is about the identifier and nothing else.
    let read = ok(
        &router,
        "GetFirewallRuleGroupPolicy",
        json!({ "Arn": live_arn }),
    )
    .await;
    assert!(
        read.get("FirewallRuleGroupPolicy").is_some(),
        "the live group must still report a policy document, got {read}"
    );
}

/// What the sentence's quotes hold is the id and only the id: no ARN prefix, no
/// separator, nothing else.
fn assert_quotes_hold_only_the_id(case: &str, operation: &str, id: &str, message: &str) {
    let opened = message
        .find('"')
        .unwrap_or_else(|| panic!("case {case:?}: {operation}'s message must quote the resource it could not find, got {message:?}"));
    let rest = &message[opened + 1..];
    let closed = rest.rfind('"').unwrap_or_else(|| {
        panic!("case {case:?}: {operation}'s quoted id must be closed, got {message:?}")
    });
    let quoted = &rest[..closed];
    assert_eq!(
        quoted, id,
        "case {case:?}: {operation} must quote only the id, got {quoted:?} in {message:?}"
    );
    assert!(
        !message.contains("arn:"),
        "case {case:?}: {operation}'s message must carry no ARN, got {message:?}"
    );
}

/// Call `operation` with `body`, require the status and the error shape the
/// service refuses an unaddressable resource with, and answer the message.
async fn not_found_message(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> String {
    let (status, refusal) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer HTTP 400, got {status}: {refusal}"
    );
    let answered = error_type(&refusal);
    assert!(
        answered.contains("ResourceNotFoundException"),
        "case {case:?}: {operation} must answer ResourceNotFoundException, got {answered:?}: \
         {refusal}"
    );
    string_member(&refusal, "Message")
}
