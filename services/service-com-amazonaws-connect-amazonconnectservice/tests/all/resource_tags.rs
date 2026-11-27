// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` end to end over HTTP through the same `MultiServiceRouter` the
//! emulator serves with.
//!
//! RULE under test: the call reports the tag map stored on the resource its `resourceArn`
//! names, whichever kind of Connect resource that ARN names, and refuses an ARN no stored
//! resource carries with this service's single not-found error. It is a reporting call, so
//! it writes nothing: the tags it answers are still what every later read reports.
//!
//! Every suite asserts that rule over a TABLE rather than one recorded call. The rows
//! differ along what the rule generalises over: the KIND of resource the ARN names
//! (the instance, a queue, a flow and an hours of operation inside it, a claimed phone
//! number), the SIZE and FORM of the tag map (one tag, several, an empty value, a key
//! using the punctuation its modelled pattern allows, and the modelled maximum of 50),
//! and the reason an ARN resolves to nothing (never minted, a different resource kind,
//! another service's ARN, an identifier that is not an ARN, a resource a delete retired).

use std::collections::BTreeMap;

use serde_json::json;

use super::harness::{
    connect_managed, expected, queue_with_tags, tag_map, tags_of, Connect, ABSENT_ID, FLOW_CONTENT,
    RESOURCE_NOT_FOUND, TAG_MAP_MAXIMUM,
};

impl Connect {
    /// The ARN of one resource the instance was born with, read off the `List*` that
    /// reports that kind: an untagged resource nobody created by hand.
    async fn first_listed_arn(&self, uri: &str, list_member: &str) -> String {
        let listed = self.send_answer("GET", uri, None).await;
        assert_eq!(
            listed.status, 200,
            "GET {uri}: got {} {}",
            listed.status, listed.body
        );
        listed.body[list_member][0]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("{uri} reports an entry with an Arn, got {}", listed.body))
            .to_string()
    }
}

/// RULE: the answer is the tag map stored on the resource the ARN names — for every kind
/// of resource that carries tags, and whatever the map's size or the form of its keys and
/// values.
///
/// The rows cross the resource kinds with the tag-map forms: one entry, several, a key
/// using the punctuation the modelled `TagKey` pattern allows, an empty value (the
/// modelled `TagValue` minimum length is 0) and the modelled `TagMap` maximum of 50.
#[tokio::test]
async fn the_tags_reported_are_the_ones_the_named_resource_carries() {
    let connect = Connect::new();

    // A punctuation-only-but-legal key: `TagKey`'s pattern admits letters, numbers,
    // spaces and `_.:/=+-@` (`smithy_query(query="shape",shape="TagKey")`).
    let punctuated = "cost.center/eu=west_1:a+b-c@d";
    let instance_tags = json!({ "env": "prod", "team": "voice" });
    let (instance_id, instance_arn) = connect
        .create_instance(json!({
            "IdentityManagementType": "CONNECT_MANAGED",
            "InstanceAlias": "tagged",
            "InboundCallsEnabled": true,
            "OutboundCallsEnabled": true,
            "Tags": instance_tags,
        }))
        .await;

    let queue_arn = connect
        .queue_arn(
            &instance_id,
            queue_with_tags("support", json!({ punctuated: "1" })),
        )
        .await;
    let flow_arn = connect
        .contact_flow_arn(
            &instance_id,
            json!({
                "Name": "greeting",
                "Type": "CONTACT_FLOW",
                "Content": FLOW_CONTENT,
                "Tags": tag_map(TAG_MAP_MAXIMUM),
            }),
        )
        .await;
    let hours_arn = connect
        .hours_of_operation_arn(
            &instance_id,
            json!({
                "Name": "always",
                "TimeZone": "UTC",
                "Config": [{
                    "Day": "MONDAY",
                    "StartTime": { "Hours": 0, "Minutes": 0 },
                    "EndTime": { "Hours": 23, "Minutes": 59 },
                }],
                // A `TagValue` may be empty (length 0..256), which is a stored tag, not
                // an absent one.
                "Tags": { "owner": "" },
            }),
        )
        .await;
    let number_arn = connect
        .claimed_phone_number_arn(&instance_id, json!({ "line": "main", "env": "prod" }))
        .await;

    let maximum: Vec<(String, String)> = (0..TAG_MAP_MAXIMUM)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    let maximum: BTreeMap<String, String> = maximum.into_iter().collect();

    let rows: Vec<(&str, &String, BTreeMap<String, String>)> = vec![
        (
            "the instance itself, two tags",
            &instance_arn,
            expected(&[("env", "prod"), ("team", "voice")]),
        ),
        (
            "a queue, one tag whose key uses the punctuation its pattern allows",
            &queue_arn,
            expected(&[(punctuated, "1")]),
        ),
        ("a flow, the modelled maximum of tags", &flow_arn, maximum),
        (
            "an hours of operation, one tag with an empty value",
            &hours_arn,
            expected(&[("owner", "")]),
        ),
        (
            "a claimed phone number, two tags",
            &number_arn,
            expected(&[("line", "main"), ("env", "prod")]),
        ),
    ];

    for (case, arn, want) in rows {
        let answer = connect.list_tags_for_resource(arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        let actual = tags_of(&answer);
        assert_eq!(
            actual.as_ref(),
            Some(&want),
            "case {case:?}: expected {want:?}, got {actual:?}"
        );
    }
}

/// RULE (the other end of the same rule): a resource that EXISTS and carries no tags is
/// not a missing resource — it answers 200, reporting no tags.
///
/// The rows span the kinds again, through the resources `CreateInstance` provisions the
/// instance with (nobody tagged those) plus one created without a `Tags` member.
#[tokio::test]
async fn an_untagged_resource_answers_with_no_tags() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect.create_instance(connect_managed("untagged")).await;

    let default_queue = connect
        .first_listed_arn(&format!("/queues-summary/{instance_id}"), "QueueSummaryList")
        .await;
    let default_flow = connect
        .first_listed_arn(
            &format!("/contact-flows-summary/{instance_id}"),
            "ContactFlowSummaryList",
        )
        .await;
    let default_hours = connect
        .first_listed_arn(
            &format!("/hours-of-operations-summary/{instance_id}"),
            "HoursOfOperationSummaryList",
        )
        .await;
    let created_queue = connect
        .queue_arn(
            &instance_id,
            json!({ "Name": "no-tags", "HoursOfOperationId": ABSENT_ID }),
        )
        .await;

    let rows: Vec<(&str, &String)> = vec![
        ("an instance created without Tags", &instance_arn),
        ("a queue the instance was born with", &default_queue),
        ("a flow the instance was born with", &default_flow),
        (
            "the hours of operation the instance was born with",
            &default_hours,
        ),
        ("a queue created without Tags", &created_queue),
    ];

    for (case, arn) in rows {
        let answer = connect.list_tags_for_resource(arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        let actual = tags_of(&answer);
        // The service has ONE tag projection: an untagged resource reports no `tags`
        // member at all, which is also the only form the modelled `TagMap` (length
        // 1..50) can take — so an empty map is not an acceptable answer either.
        assert!(
            actual.is_none(),
            "case {case:?}: expected no tags member, got {actual:?}"
        );
    }
}

/// RULE: an ARN no stored resource carries names nothing whose tags could be reported, so
/// it is refused with this service's one not-found error (HTTP 404,
/// `ResourceNotFoundException`, `Resource not found`) — whatever makes it resolve to
/// nothing, and quoting nothing the request carried.
#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_refused_as_not_found() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect.create_instance(connect_managed("refusals")).await;

    // A queue that exists and is then deleted: its ARN named a resource until the delete
    // retired it, which is the same answer as one that never existed.
    let retired_queue = connect
        .queue_arn(
            &instance_id,
            queue_with_tags("retired", json!({ "env": "prod" })),
        )
        .await;
    let retired_id = retired_queue
        .rsplit('/')
        .next()
        .expect("a queue ARN ends in its id")
        .to_string();
    let deleted = connect
        .send_answer(
            "DELETE",
            &format!("/queues/{instance_id}/{retired_id}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteQueue {retired_id}: got {} {}",
        deleted.status, deleted.body
    );

    let rows: Vec<(&str, String)> = vec![
        (
            "an instance ARN of an id never minted",
            format!("arn:aws:connect:us-east-1:000000000000:instance/{ABSENT_ID}"),
        ),
        (
            "a queue ARN under a live instance, with an unknown queue id",
            format!("{instance_arn}/queue/{ABSENT_ID}"),
        ),
        (
            "a flow ARN under a live instance, with an unknown flow id",
            format!("{instance_arn}/contact-flow/{ABSENT_ID}"),
        ),
        (
            "an hours-of-operation ARN under a live instance, unknown id",
            format!("{instance_arn}/operating-hours/{ABSENT_ID}"),
        ),
        (
            "a phone-number ARN this account never claimed",
            format!("arn:aws:connect:us-east-1:000000000000:phone-number/{ABSENT_ID}"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:lex:us-east-1:000000000000:bot/{ABSENT_ID}"),
        ),
        (
            "the live instance's identifier rather than its ARN",
            instance_id.clone(),
        ),
        (
            "the live instance's ARN with another segment after it",
            format!("{instance_arn}/not-a-resource-kind/{ABSENT_ID}"),
        ),
        ("a queue a delete retired", retired_queue.clone()),
    ];

    for (case, arn) in rows {
        assert_ne!(
            arn, instance_arn,
            "case {case:?}: the row has to name a resource the store does not hold"
        );
        let answer = connect.list_tags_for_resource(&arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected error code ResourceNotFoundException, got {:?} {}",
            answer.error_code, answer.body
        );
        let message = answer.body["Message"].as_str().unwrap_or_default();
        assert_eq!(
            message, RESOURCE_NOT_FOUND,
            "case {case:?}: expected message {RESOURCE_NOT_FOUND:?}, got {message:?}"
        );
    }

    // The instance that IS there still reports its own tags: the refusals above belong to
    // the ARNs that resolve to nothing.
    let served = connect.list_tags_for_resource(&instance_arn).await;
    assert_eq!(
        served.status, 200,
        "the live instance's own ARN is still served, got {} {}",
        served.status, served.body
    );
}

/// RULE (a report, not a mutation): listing a resource's tags writes nothing, so a second
/// listing and every other read of the same resource answer exactly what the first did.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect
        .create_instance(json!({
            "IdentityManagementType": "SAML",
            "InstanceAlias": "stable",
            "InboundCallsEnabled": true,
            "OutboundCallsEnabled": true,
            "Tags": { "env": "prod" },
        }))
        .await;
    let queue_arn = connect
        .queue_arn(&instance_id, queue_with_tags("stable", json!({ "tier": "gold" })))
        .await;
    let queue_id = queue_arn
        .rsplit('/')
        .next()
        .expect("a queue ARN ends in its id")
        .to_string();

    for (case, arn) in [("the instance", &instance_arn), ("its queue", &queue_arn)] {
        let first = connect.list_tags_for_resource(arn).await;
        let second = connect.list_tags_for_resource(arn).await;
        assert_eq!(
            (first.status, tags_of(&first)),
            (second.status, tags_of(&second)),
            "case {case:?}: a second listing answers the same, got {} then {}",
            first.body,
            second.body
        );
    }

    // The tags the creates persisted are still the ones the resources' own reads report,
    // under the member name each of those shapes gives them.
    let instance = connect
        .send_answer("GET", &format!("/instance/{instance_id}"), None)
        .await;
    assert_eq!(
        instance.body["Instance"]["Tags"],
        json!({ "env": "prod" }),
        "DescribeInstance still reports the instance's tags, got {}",
        instance.body
    );
    let queue = connect
        .send_answer("GET", &format!("/queues/{instance_id}/{queue_id}"), None)
        .await;
    assert_eq!(
        queue.body["Queue"]["Tags"],
        json!({ "tier": "gold" }),
        "DescribeQueue still reports the queue's tags, got {}",
        queue.body
    );
}
