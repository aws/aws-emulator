// SPDX-License-Identifier: CC0-1.0

//! `TagResource` end to end over HTTP through the same `MultiServiceRouter` the emulator
//! serves with.
//!
//! RULE under test: the call ADDS the tags it carries to the resource its `resourceArn`
//! names — whichever kind of Connect resource that ARN names — merging them into the tags
//! the resource already holds (a key it does not have is added, a key it does have takes
//! the request's value, a key the request does not name keeps the value it had), and the
//! stored map is what every later read of that resource reports. An ARN no stored resource
//! carries names nothing to tag, so it is refused with this service's single not-found
//! error and nothing is written.
//!
//! Every suite asserts that rule over a TABLE rather than one recorded call. The rows
//! differ along what the rule generalises over: the KIND of resource the ARN names (the
//! instance, a queue, a flow and an hours of operation inside it, a claimed phone number),
//! the relation between the stored map and the request's (disjoint, overlapping, a repeat
//! of the same call), the SIZE and FORM of the map (one tag, an empty value, a key using
//! the punctuation its modelled pattern allows, the modelled maximum of 50 and a request
//! that would push a resource past it), and the reason an ARN resolves to nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::harness::{
    connect_managed, expected, label, queue_with_tags, tag_map, tags_of, Answer, Connect,
    ABSENT_ID, FLOW_CONTENT, RESOURCE_NOT_FOUND, TAG_MAP_MAXIMUM,
};

/// A key made only of the punctuation `TagKey`'s pattern admits: letters, numbers, spaces
/// and `_.:/=+-@` (`smithy_query(query="shape",shape="TagKey")`).
const PUNCTUATED_KEY: &str = "cost.center/eu=west_1:a+b-c@d";

impl Connect {
    /// `TagResource`: `POST /tags/{resourceArn}`, whose body carries the modelled `tags`
    /// map.
    async fn tag_resource(&self, resource_arn: &str, tags: Value) -> Answer {
        self.send_answer(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
    }

    /// `TagResource`, asserting the call was served.
    async fn tag_resource_ok(&self, case: &str, resource_arn: &str, tags: Value) {
        let tagged = self.tag_resource(resource_arn, tags.clone()).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected HTTP 200 from TagResource {tags}, got {} {}",
            tagged.status, tagged.body
        );
    }

    /// The tags `ListTagsForResource` reports for an ARN, as a plain map.
    async fn stored_tags(
        &self,
        case: &str,
        resource_arn: &str,
    ) -> Option<BTreeMap<String, String>> {
        let listed = self.list_tags_for_resource(resource_arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} {}",
            listed.status, listed.body
        );
        tags_of(&listed)
    }
}

/// A `CreateQueue` body with no `Tags` member at all: a resource that starts untagged.
fn queue_without_tags(name: &str) -> Value {
    json!({ "Name": name, "HoursOfOperationId": ABSENT_ID })
}

/// The tag map of `count` entries [`tag_map`] builds, as the map a read should report.
fn tag_map_want(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect()
}

/// One row of the merge table: the case, the map the resource is created with (`None` for
/// one created without a `Tags` member), the map the call adds, and the map a later read
/// should then report.
type MergeRow<'a> = (&'a str, Option<Value>, Value, BTreeMap<String, String>);

/// RULE: the tags the call adds are the ones every later read of that resource reports —
/// for every kind of resource an ARN can name, and merged into whatever that resource
/// already carried.
///
/// The rows cross the resource kinds with the tag-map forms: a resource that starts
/// tagged and one that starts untagged, one entry and the modelled maximum of 50, an
/// empty value (the modelled `TagValue` minimum length is 0) and a key using the
/// punctuation its modelled pattern allows. Every resource is tagged before anything is
/// read back, so a row also shows that tagging one resource leaves the others alone.
#[tokio::test]
async fn the_tags_added_are_the_ones_every_later_read_reports() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect
        .create_instance(json!({
            "IdentityManagementType": "CONNECT_MANAGED",
            "InstanceAlias": "tag-writes",
            "InboundCallsEnabled": true,
            "OutboundCallsEnabled": true,
            "Tags": { "env": "prod" },
        }))
        .await;

    let queue_arn = connect
        .queue_arn(&instance_id, queue_without_tags("support"))
        .await;
    let flow_arn = connect
        .contact_flow_arn(
            &instance_id,
            json!({
                "Name": "greeting",
                "Type": "CONTACT_FLOW",
                "Content": FLOW_CONTENT,
                "Tags": { "owner": "" },
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
            }),
        )
        .await;
    let number_arn = connect
        .claimed_phone_number_arn(&instance_id, json!({ "line": "main" }))
        .await;

    // What each row's call adds, and what the resource should carry once it was added.
    let mut flow_want = expected(&[("owner", "")]);
    flow_want.insert(PUNCTUATED_KEY.to_string(), "1".to_string());
    let rows: Vec<(&str, &String, Value, BTreeMap<String, String>)> = vec![
        (
            "the instance itself, one tag added beside the one it was created with",
            &instance_arn,
            json!({ "team": "voice" }),
            expected(&[("env", "prod"), ("team", "voice")]),
        ),
        (
            "a queue created without tags, one tag added",
            &queue_arn,
            json!({ "tier": "gold" }),
            expected(&[("tier", "gold")]),
        ),
        (
            "a flow, a key of the punctuation its pattern allows added beside an empty value",
            &flow_arn,
            json!({ PUNCTUATED_KEY: "1" }),
            flow_want,
        ),
        (
            "an hours of operation created without tags, the modelled maximum added",
            &hours_arn,
            tag_map(TAG_MAP_MAXIMUM),
            tag_map_want(TAG_MAP_MAXIMUM),
        ),
        (
            "a claimed phone number, one tag added beside the one it was claimed with",
            &number_arn,
            json!({ "env": "prod" }),
            expected(&[("line", "main"), ("env", "prod")]),
        ),
    ];

    for (case, arn, add, _) in &rows {
        connect.tag_resource_ok(case, arn, add.clone()).await;
    }

    for (case, arn, _, want) in &rows {
        let actual = connect.stored_tags(case, arn).await;
        assert_eq!(
            actual.as_ref(),
            Some(want),
            "case {case:?}: expected {want:?}, got {actual:?}"
        );
    }

    // The resources' OWN readers report the same tags: the tagging plane writes the one
    // map each record carries, not a second copy beside it.
    let instance = connect
        .send_answer("GET", &format!("/instance/{instance_id}"), None)
        .await;
    assert_eq!(
        instance.body["Instance"]["Tags"],
        json!({ "env": "prod", "team": "voice" }),
        "DescribeInstance reports the tags TagResource added, got {}",
        instance.body
    );
    let queue_id = queue_arn
        .rsplit('/')
        .next()
        .expect("a queue ARN ends in its id")
        .to_string();
    let queue = connect
        .send_answer("GET", &format!("/queues/{instance_id}/{queue_id}"), None)
        .await;
    assert_eq!(
        queue.body["Queue"]["Tags"],
        json!({ "tier": "gold" }),
        "DescribeQueue reports the tags TagResource added, got {}",
        queue.body
    );
}

/// RULE (the merge itself): the request's tags are ADDED to the stored map. A key the
/// resource does not carry is added, a key it does carry takes the request's value, a key
/// the request does not name is left as it was — so repeating the same call leaves exactly
/// the same tags stored.
///
/// Each row starts a queue of its own at the stored map it names, so the rows differ only
/// along the relation between that map and the request's: disjoint, overlapping, equal,
/// and growing to the modelled maximum.
#[tokio::test]
async fn tagging_merges_the_request_into_the_tags_the_resource_carries() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("merge")).await;

    let mut overlap_want = expected(&[("env", "dev"), ("team", "voice")]);
    overlap_want.insert("owner".to_string(), String::new());
    // One key past the map `tag_map(TAG_MAP_MAXIMUM - 1)` stored, so the merge lands
    // exactly on the modelled maximum.
    let last_key = format!("key{}", TAG_MAP_MAXIMUM - 1);
    let last_value = format!("value{}", TAG_MAP_MAXIMUM - 1);

    let rows: Vec<MergeRow<'_>> = vec![
        (
            "an untagged resource takes the request's map",
            None,
            json!({ "env": "prod" }),
            expected(&[("env", "prod")]),
        ),
        (
            "a disjoint key is added beside the stored ones",
            Some(json!({ "env": "prod" })),
            json!({ "team": "voice" }),
            expected(&[("env", "prod"), ("team", "voice")]),
        ),
        (
            "a key the resource carries takes the request's value, the others stay",
            Some(json!({ "env": "prod", "team": "voice", "owner": "ops" })),
            json!({ "env": "dev", "owner": "" }),
            overlap_want,
        ),
        (
            "a request equal to the stored map leaves it as it is",
            Some(json!({ "env": "prod" })),
            json!({ "env": "prod" }),
            expected(&[("env", "prod")]),
        ),
        (
            "a punctuated key is stored as given",
            None,
            json!({ PUNCTUATED_KEY: "1" }),
            expected(&[(PUNCTUATED_KEY, "1")]),
        ),
        (
            "the stored map grows to the modelled maximum of 50",
            Some(tag_map(TAG_MAP_MAXIMUM - 1)),
            json!({ last_key: last_value }),
            tag_map_want(TAG_MAP_MAXIMUM),
        ),
    ];

    for (index, (case, stored, add, want)) in rows.into_iter().enumerate() {
        let name = format!("merge-{index}");
        let body = match stored {
            Some(tags) => queue_with_tags(&name, tags),
            None => queue_without_tags(&name),
        };
        let arn = connect.queue_arn(&instance_id, body).await;

        connect.tag_resource_ok(case, &arn, add.clone()).await;
        let actual = connect.stored_tags(case, &arn).await;
        assert_eq!(
            actual.as_ref(),
            Some(&want),
            "case {case:?}: expected {want:?}, got {actual:?}"
        );

        // Adding the same tags again is the same state: the call sets the keys it names
        // rather than appending anything.
        connect.tag_resource_ok(case, &arn, add).await;
        let repeated = connect.stored_tags(case, &arn).await;
        assert_eq!(
            repeated.as_ref(),
            Some(&want),
            "case {case:?}: repeating the call still expects {want:?}, got {repeated:?}"
        );
    }
}

/// RULE: the map a resource carries is held to the maximum the modelled `TagMap` every
/// read reports it through declares (50): a request whose NEW keys would push the stored
/// map past it is refused with the modelled `InvalidParameterException` (HTTP 400) naming
/// the member, and writes nothing — while a request that only re-values keys the resource
/// already carries grows nothing and is still served.
///
/// The rows differ along how far past the bound the request reaches and how much of it is
/// growth: exactly at the maximum, one key past it, many keys past it, and a request that
/// names only stored keys.
#[tokio::test]
async fn the_stored_tag_map_is_held_to_the_modelled_maximum() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("bounded")).await;

    // A queue carrying the modelled maximum already.
    let full = connect
        .queue_arn(
            &instance_id,
            queue_with_tags("full", tag_map(TAG_MAP_MAXIMUM)),
        )
        .await;
    let want = tag_map_want(TAG_MAP_MAXIMUM);

    // A request of keys the resource already carries is growth of zero, whatever its
    // size: it is served, and the values it names are the ones stored.
    connect
        .tag_resource_ok(
            "re-valuing stored keys at the maximum",
            &full,
            json!({ "key0": "changed" }),
        )
        .await;
    let mut revalued = want.clone();
    revalued.insert("key0".to_string(), "changed".to_string());
    let actual = connect.stored_tags("re-valuing stored keys", &full).await;
    assert_eq!(
        actual.as_ref(),
        Some(&revalued),
        "a request that adds no key is served: expected {revalued:?}, got {actual:?}"
    );

    let rows: Vec<(&str, Value)> = vec![
        ("one key past the maximum", json!({ "spare": "1" })),
        (
            "one new key beside stored ones",
            json!({ "key0": "a", "spare": "1" }),
        ),
        (
            "a second map of the modelled maximum, all of it new keys",
            Value::Object(
                (0..TAG_MAP_MAXIMUM)
                    .map(|index| (format!("extra{index}"), json!("1")))
                    .collect(),
            ),
        ),
    ];

    for (case, add) in rows {
        let refused = connect.tag_resource(&full, add.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, "InvalidParameterException",
            "case {case:?}: expected error code InvalidParameterException, got {:?} {}",
            refused.error_code, refused.body
        );
        let message = refused.body["Message"].as_str().unwrap_or_default();
        assert_eq!(
            message, "Invalid parameter: Tags",
            "case {case:?}: expected the member named, got {message:?}"
        );
        // The refusal wrote nothing: the map is still the one the re-valued call left.
        let after = connect.stored_tags(case, &full).await;
        assert_eq!(
            after.as_ref(),
            Some(&revalued),
            "case {case:?}: the refusal stored nothing, expected {revalued:?}, got {after:?}"
        );
    }

    // A resource BELOW the maximum still takes the keys that fit it exactly.
    let nearly = connect
        .queue_arn(
            &instance_id,
            queue_with_tags("nearly", tag_map(TAG_MAP_MAXIMUM - 1)),
        )
        .await;
    let last_key = format!("key{}", TAG_MAP_MAXIMUM - 1);
    let last_value = format!("value{}", TAG_MAP_MAXIMUM - 1);
    connect
        .tag_resource_ok(
            "the key that fits exactly",
            &nearly,
            json!({ last_key: last_value }),
        )
        .await;
    let filled = connect
        .stored_tags("the key that fits exactly", &nearly)
        .await;
    assert_eq!(
        filled.as_ref(),
        Some(&want),
        "the last key that fits is stored: expected {want:?}, got {filled:?}"
    );
}

/// RULE: an ARN no stored resource carries names nothing to tag, so the call is refused
/// with this service's one not-found error (HTTP 404, `ResourceNotFoundException`,
/// `Resource not found`) — whatever makes it resolve to nothing — and the refusal writes
/// nothing, for that ARN or for the resources that do exist.
#[tokio::test]
async fn tagging_an_arn_that_names_no_stored_resource_is_refused_as_not_found() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect
        .create_instance(json!({
            "IdentityManagementType": "SAML",
            "InstanceAlias": "tag-refusals",
            "InboundCallsEnabled": true,
            "OutboundCallsEnabled": true,
            "Tags": { "env": "prod" },
        }))
        .await;

    // A queue that exists and is then deleted: its ARN named a resource until the delete
    // retired it, which is the same answer as one that never existed.
    let retired_queue = connect
        .queue_arn(&instance_id, queue_with_tags("retired", json!({ "env": "prod" })))
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

    for (case, arn) in &rows {
        assert_ne!(
            arn, &instance_arn,
            "case {case:?}: the row has to name a resource the store does not hold"
        );
        let answer = connect.tag_resource(arn, json!({ "env": "prod" })).await;
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

    // A refused call stored nothing: the ARN it named still names nothing, and the
    // instance that IS there carries exactly the tags it was created with.
    for (case, arn) in &rows {
        let listed = connect.list_tags_for_resource(arn).await;
        assert_eq!(
            listed.status, 404,
            "case {case:?}: the refused ARN still names no resource, got {} {}",
            listed.status, listed.body
        );
    }
    let want = expected(&[("env", "prod")]);
    let actual = connect.stored_tags("the live instance", &instance_arn).await;
    assert_eq!(
        actual.as_ref(),
        Some(&want),
        "the refusals left the live instance's tags alone: expected {want:?}, got {actual:?}"
    );
}
