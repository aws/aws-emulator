// SPDX-License-Identifier: CC0-1.0

//! The TAGGING SUB-TYPES of S3's event notifications: a tag-set write and a tag-set
//! removal are two different events, and each reaches only the destinations and the rules
//! that name it.
//!
//! The rule under test: a destination configuration is notified of an object change when
//! its `Events` list names that change's own event value — literally or through the
//! `s3:<Type>:*` form, which stands for every event of that type — and, while the bucket
//! carries an `EventBridgeConfiguration`, the same change is published onto the account's
//! default event bus under the detail-type of its own event class, so a rule whose pattern
//! names that detail-type reaches its targets and a rule naming another one does not.
//!
//! The model makes the sub-type part of the wire contract: `Event`
//! (`smithy_query(query="shape",shape="Event")`) enumerates `s3:ObjectTagging:Put`,
//! `s3:ObjectTagging:Delete` and `s3:ObjectTagging:*` as three distinct values, so a
//! subscription naming one of the first two must not be delivered the other's event. The
//! two tagging calls are the pair that exercises it: `PutObjectTagging` "sets the supplied
//! tag-set to an object" and `DeleteObjectTagging` "removes the entire tag set from the
//! specified object", so one bucket carrying both sub-types' subscriptions at once has to
//! tell the two calls apart on both delivery paths.
//!
//! What a row asserts, for the call it makes:
//!
//! * the queue configuration scoped to that call's sub-type receives a record whose
//!   `eventName` is that sub-type's, for that key;
//! * the queue configuration scoped to the type WILDCARD receives the same event for the
//!   same call made under its own key prefix, because the wildcard spans every sub-type of
//!   the type;
//! * the queue the rule on that sub-type's detail-type targets receives an `aws.s3`
//!   envelope carrying that detail-type, the bucket and the key;
//! * and the OTHER sub-type's queue configuration and the OTHER detail-type's rule target
//!   stay empty — which is what makes the differential observable rather than vacuous.
//!
//! The three configurations sit on ONE bucket over DISJOINT key prefixes, because the type
//! wildcard covers each scoped configuration's own event: two configurations that would both
//! match one key for one event are ambiguous and S3 refuses the whole configuration, so the
//! `Filter` is what tells them apart and each call is made under the prefix of the
//! configuration it is about.
//!
//! The rows span what the rule generalises over: which sub-type the call produces, the
//! size of the tag set it writes (one tag, several, none at all), the form of the key
//! (plain, prefixed, carrying a space, so the record's URL-encoded key and the bus's raw
//! one are both exercised), and whether the write sub-type's routes are still live after
//! the removal sub-type's event was delivered.
//!
//! Every request is driven through a router carrying S3 beside EventBridge and SQS, so
//! what is pinned is what a consumer receives and not any handler's internals. Each row
//! gets a bucket and a set of queues of its own: a message this suite READS stays on its
//! queue until its visibility timeout runs out, so rows sharing a queue could see an
//! earlier row's record again in a silence check.

use std::collections::HashMap;

use serde_json::json;

use super::object_event_notifications::{queue_arn, request_key, Answer, Emulator, REGION, XMLNS};

/// The bytes every key below holds.
const OBJECT_BODY: &[u8] = b"tag-governance-payload";

/// The tag set every key is seeded with before the bucket is configured, so a row that
/// REMOVES a tag set removes one that was there.
const SEED_TAGS: &[(&str, &str)] = &[("classification", "internal")];

/// Which of the two tagging sub-types a call produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SubType {
    /// `PutObjectTagging` — a tag set was placed on the object.
    Added,
    /// `DeleteObjectTagging` — the object's tag set was removed.
    Deleted,
}

impl SubType {
    /// The `Event` value a configuration names to be notified of this sub-type — one of
    /// the model's own `Event` values (`smithy_query(query="shape",shape="Event")`).
    fn event_value(self) -> &'static str {
        match self {
            Self::Added => "s3:ObjectTagging:Put",
            Self::Deleted => "s3:ObjectTagging:Delete",
        }
    }

    /// The `eventName` a delivered record carries: the event value without its `s3:`
    /// prefix.
    fn record_event_name(self) -> &'static str {
        match self {
            Self::Added => "ObjectTagging:Put",
            Self::Deleted => "ObjectTagging:Delete",
        }
    }

    /// The `detail-type` the default bus carries this sub-type's event under.
    fn detail_type(self) -> &'static str {
        match self {
            Self::Added => "Object Tags Added",
            Self::Deleted => "Object Tags Deleted",
        }
    }

    /// The queue configuration scoped to this sub-type alone.
    fn subscription(self) -> &'static Subscription {
        match self {
            Self::Added => &ADDED_SCOPED,
            Self::Deleted => &DELETED_SCOPED,
        }
    }

    /// The other sub-type — the one whose routes have to stay silent.
    fn other(self) -> Self {
        match self {
            Self::Added => Self::Deleted,
            Self::Deleted => Self::Added,
        }
    }
}

/// Both sub-types, which is the span every row is judged over.
const SUB_TYPES: &[SubType] = &[SubType::Added, SubType::Deleted];

/// One `QueueConfiguration` of the bucket's notification configuration: the `Id` it is
/// stored under (which a delivered record reports as its `configurationId`), the queue it
/// names, the `Events` it is notified of and the key-name prefix its `Filter` admits.
struct Subscription {
    id: &'static str,
    queue: &'static str,
    events: &'static [&'static str],
    prefix: &'static str,
}

/// The prefix the two SCOPED configurations filter by, and the one the WILDCARD
/// configuration filters by.
///
/// They are DISJOINT because the type wildcard covers each scoped configuration's own
/// event: two configurations of one bucket cannot share an event type, and S3 refuses the
/// whole configuration when they do (`InvalidArgument`, "Configurations overlap"). The
/// filters are what tell them apart, so one bucket carries all three and each call below is
/// made under the prefix of the configuration it is about.
const SCOPED_PREFIX: &str = "scoped/";
const WILDCARD_PREFIX: &str = "every/";

/// The queue configuration scoped to a tag-set WRITE alone.
const ADDED_SCOPED: Subscription = Subscription {
    id: "tags-added-only",
    queue: "tag-events-added",
    events: &["s3:ObjectTagging:Put"],
    prefix: SCOPED_PREFIX,
};

/// The queue configuration scoped to a tag-set REMOVAL alone.
const DELETED_SCOPED: Subscription = Subscription {
    id: "tags-deleted-only",
    queue: "tag-events-deleted",
    events: &["s3:ObjectTagging:Delete"],
    prefix: SCOPED_PREFIX,
};

/// The queue configuration naming the type wildcard, which stands for every event of the
/// tagging type and so is notified of both sub-types.
const EVERY_TAGGING: Subscription = Subscription {
    id: "every-tagging-event",
    queue: "tag-events-any",
    events: &["s3:ObjectTagging:*"],
    prefix: WILDCARD_PREFIX,
};

/// The three queue configurations one bucket carries at once.
const SUBSCRIPTIONS: &[&Subscription] = &[&ADDED_SCOPED, &DELETED_SCOPED, &EVERY_TAGGING];

/// Which tagging call a row makes.
#[derive(Debug, Clone, Copy)]
enum Call {
    /// `PutObjectTagging` carrying exactly these tags.
    Tag(&'static [(&'static str, &'static str)]),
    /// `DeleteObjectTagging`.
    Untag,
}

impl Call {
    fn sub_type(self) -> SubType {
        match self {
            Self::Tag(_) => SubType::Added,
            Self::Untag => SubType::Deleted,
        }
    }
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The key the call is made on.
    key: &'static str,
    /// The key as a delivered RECORD spells it — "URL encoded" (Amazon S3 User Guide,
    /// *Event message structure*), where a space appears as a plus.
    published_key: &'static str,
    call: Call,
}

/// The rule, over the class it generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "a one-tag set placed on a plain key",
        key: "report.json",
        published_key: "report.json",
        call: Call::Tag(&[("reviewed", "yes")]),
    },
    Row {
        case: "the tag set removed from a plain key",
        key: "report.json",
        published_key: "report.json",
        call: Call::Untag,
    },
    Row {
        case: "a three-tag set placed on a prefixed key",
        key: "reports/2026/q1.json",
        published_key: "reports/2026/q1.json",
        call: Call::Tag(&[
            ("owner", "data-platform"),
            ("tier", "gold"),
            ("retention", "7y"),
        ]),
    },
    Row {
        case: "the tag set removed from a key carrying a space",
        key: "quarterly report.txt",
        published_key: "quarterly+report.txt",
        call: Call::Untag,
    },
    Row {
        case: "an empty tag set placed on a key whose tag set was removed first",
        key: "report.json",
        published_key: "report.json",
        call: Call::Tag(&[]),
    },
];

/// The queue, bucket or rule `name` belongs to the row at `index`, so no two rows share a
/// queue and a message one row read cannot reappear in another row's silence check.
fn per_row(name: &str, index: usize) -> String {
    format!("{name}-{index}")
}

/// A default-bus rule matching the `aws.s3` events of `bucket` whose `detail-type` is
/// `detail_type`, delivering to the queue named `queue`.
///
/// `pub(crate)`: the suite for what a REFUSED change publishes
/// ([`super::refused_change_publishes_no_event`]) scopes its own bus inbox to one
/// detail-type through this rather than writing a second rule builder.
pub(crate) async fn rule_on_detail_type(
    emulator: &Emulator,
    name: &str,
    bucket: &str,
    detail_type: &str,
    queue: &str,
) {
    emulator
        .events(
            "PutRule",
            &json!({
                "Name": name,
                "EventPattern": json!({
                    "source": ["aws.s3"],
                    "detail-type": [detail_type],
                    "detail": { "bucket": { "name": [bucket] } },
                })
                .to_string(),
            }),
        )
        .await;
    let targeted = emulator
        .events(
            "PutTargets",
            &json!({
                "Rule": name,
                "Targets": [{ "Id": "the-queue", "Arn": queue_arn(queue) }],
            }),
        )
        .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "case {name:?}: the queue target must be accepted, got {targeted}"
    );
}

/// `PUT /<bucket>?notification` — `PutBucketNotificationConfiguration` carrying every
/// queue configuration of [`SUBSCRIPTIONS`], each naming the row's own queue, plus the
/// `EventBridgeConfiguration` that "enables delivery of events to Amazon EventBridge".
///
/// The harness's own `put_notification` carries ONE destination configuration, and what
/// this suite is about is several of them on one bucket, which is why the body is built
/// here.
async fn install_notification(emulator: &Emulator, bucket: &str, index: usize) {
    let mut body = format!("<NotificationConfiguration xmlns=\"{XMLNS}\">");
    for subscription in SUBSCRIPTIONS {
        body.push_str("<QueueConfiguration>");
        body.push_str(&format!("<Id>{}</Id>", subscription.id));
        body.push_str(&format!(
            "<Queue>{}</Queue>",
            queue_arn(&per_row(subscription.queue, index))
        ));
        for event in subscription.events {
            body.push_str(&format!("<Event>{event}</Event>"));
        }
        body.push_str(&format!(
            "<Filter><S3Key><FilterRule><Name>prefix</Name><Value>{}</Value>\
             </FilterRule></S3Key></Filter>",
            subscription.prefix
        ));
        body.push_str("</QueueConfiguration>");
    }
    body.push_str("<EventBridgeConfiguration></EventBridgeConfiguration>");
    body.push_str("</NotificationConfiguration>");
    let answer = emulator
        .s3_xml("PUT", &format!("/{bucket}?notification"), &body)
        .await;
    assert_eq!(
        answer.status,
        200,
        "configuring notifications on {bucket} must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
}

/// Take the `s3:TestEvent` the installed configuration published to the queue it
/// validated, so a row below reads only what its own call delivered.
async fn take_test_event(emulator: &Emulator, id: &str, url: &str) {
    let test_event = emulator.delivered(url).await.unwrap_or_else(|| {
        panic!(
            "case {id:?}: configuring notifications must publish an s3:TestEvent to the \
             queue it validated"
        )
    });
    assert_eq!(
        test_event["Event"].as_str(),
        Some("s3:TestEvent"),
        "case {id:?}: expected the validation test notification, got {test_event}"
    );
}

/// `DELETE /<bucket>/<key>?tagging` — `DeleteObjectTagging`, which answers 204.
async fn delete_object_tagging(emulator: &Emulator, bucket: &str, key: &str) -> Answer {
    let answer = emulator
        .s3_xml(
            "DELETE",
            &format!("/{bucket}/{}?tagging", request_key(key)),
            "",
        )
        .await;
    assert_eq!(
        answer.status,
        204,
        "untagging {key:?} must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
    answer
}

/// The url of the queue `name` names in `urls`, or a failure saying which lookup failed.
fn url_of<'a, Key: ::std::fmt::Debug + ::std::hash::Hash + Eq>(
    urls: &'a HashMap<Key, String>,
    name: &Key,
) -> &'a str {
    urls.get(name)
        .unwrap_or_else(|| panic!("the queue of {name:?} must have been created"))
        .as_str()
}

#[tokio::test]
async fn each_tagging_sub_type_reaches_only_the_subscriptions_and_rules_that_name_it() {
    // The two scoped configurations each name exactly their own sub-type's `Event` value,
    // which is what makes everything below a statement about the sub-type rather than
    // about the type.
    for sub_type in SUB_TYPES {
        assert_eq!(
            sub_type.subscription().events,
            &[sub_type.event_value()][..],
            "case {sub_type:?}: the scoped configuration must name {:?} and nothing else, \
             got {:?}",
            sub_type.event_value(),
            sub_type.subscription().events
        );
    }

    let emulator = Emulator::new();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let bucket = per_row("tag-event-sub-types", index);
        emulator.create_bucket(&bucket).await;

        // This row's own queues: the three the bucket's queue configurations name, and one
        // per detail-type rule.
        let mut classic: HashMap<&str, String> = HashMap::new();
        for subscription in SUBSCRIPTIONS {
            let url = emulator
                .create_queue(&per_row(subscription.queue, index))
                .await;
            classic.insert(subscription.id, url);
        }
        let mut bus: HashMap<SubType, String> = HashMap::new();
        for sub_type in SUB_TYPES {
            let queue = per_row(&format!("bus-{}", sub_type.subscription().queue), index);
            let url = emulator.create_queue(&queue).await;
            bus.insert(*sub_type, url);
            // One rule per detail-type, each on the default bus and each with its own
            // target queue, so the two sub-types are routed independently there too.
            rule_on_detail_type(
                &emulator,
                &per_row(sub_type.subscription().id, index),
                &bucket,
                sub_type.detail_type(),
                &queue,
            )
            .await;
        }

        // The row's call is made TWICE, once under each configuration's own prefix: the
        // scoped configurations and the wildcard one cannot both match a single key for one
        // event, so each is exercised on the key its own filter admits.
        let scoped_key = format!("{SCOPED_PREFIX}{}", row.key);
        let wildcard_key = format!("{WILDCARD_PREFIX}{}", row.key);

        // Seeded BEFORE the configuration is installed, so the write and the tag set that
        // put each key into the state this row needs notify nothing, and the row's own
        // calls are the only changes the configuration sees.
        for key in [&scoped_key, &wildcard_key] {
            emulator.put_object(&bucket, key, OBJECT_BODY).await;
            emulator
                .put_object_tagging_of(&bucket, key, SEED_TAGS)
                .await;
        }

        install_notification(&emulator, &bucket, index).await;
        for subscription in SUBSCRIPTIONS {
            take_test_event(&emulator, subscription.id, url_of(&classic, &subscription.id)).await;
        }

        let sub_type = row.call.sub_type();
        // The call the row is about on the SCOPED prefix, and the pair ITS OWN response
        // named the request by.
        let answer = match row.call {
            Call::Tag(tags) => {
                emulator
                    .put_object_tagging_of(&bucket, &scoped_key, tags)
                    .await
            }
            Call::Untag => delete_object_tagging(&emulator, &bucket, &scoped_key).await,
        };
        let (request_id, extended_request_id) = answer.correlation(case);

        // The configuration scoped to THIS sub-type is notified of it, and the record names
        // the call it came of.
        {
            let subscription = sub_type.subscription();
            let published_key = format!("{SCOPED_PREFIX}{}", row.published_key);
            let url = url_of(&classic, &subscription.id);
            let message = emulator.delivered(url).await.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no notification reached the {:?} subscription's queue",
                    subscription.id
                )
            });
            let record = &message["Records"][0];
            assert_eq!(
                record["eventName"].as_str(),
                Some(sub_type.record_event_name()),
                "case {case:?}: the {:?} subscription expected eventName {:?}, got {message}",
                subscription.id,
                sub_type.record_event_name()
            );
            assert_eq!(
                record["eventSource"].as_str(),
                Some("aws:s3"),
                "case {case:?}: the {:?} subscription expected eventSource aws:s3, got \
                 {message}",
                subscription.id
            );
            assert_eq!(
                record["awsRegion"].as_str(),
                Some(REGION),
                "case {case:?}: the {:?} subscription expected awsRegion {REGION:?}, got \
                 {message}",
                subscription.id
            );
            assert_eq!(
                record["s3"]["configurationId"].as_str(),
                Some(subscription.id),
                "case {case:?}: expected the record to name the configuration it was \
                 delivered for ({:?}), got {message}",
                subscription.id
            );
            assert_eq!(
                record["s3"]["bucket"]["name"].as_str(),
                Some(bucket.as_str()),
                "case {case:?}: the {:?} subscription expected the bucket {bucket:?}, got \
                 {message}",
                subscription.id
            );
            assert_eq!(
                record["s3"]["object"]["key"].as_str(),
                Some(published_key.as_str()),
                "case {case:?}: the {:?} subscription expected the key {published_key:?}, \
                 got {message}",
                subscription.id
            );
            assert_eq!(
                record["responseElements"]["x-amz-request-id"].as_str(),
                Some(request_id.as_str()),
                "case {case:?}: the {:?} subscription expected the call's own \
                 x-amz-request-id {request_id:?}, got {message}",
                subscription.id
            );
            assert_eq!(
                record["responseElements"]["x-amz-id-2"].as_str(),
                Some(extended_request_id.as_str()),
                "case {case:?}: the {:?} subscription expected the call's own x-amz-id-2 \
                 {extended_request_id:?}, got {message}",
                subscription.id
            );
        }

        // The EventBridge path: the rule whose pattern names THIS sub-type's detail-type
        // reaches its target.
        let envelope = emulator
            .delivered(url_of(&bus, &sub_type))
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no aws.s3 event reached the rule on {:?}",
                    sub_type.detail_type()
                )
            });
        assert_eq!(
            envelope["source"].as_str(),
            Some("aws.s3"),
            "case {case:?}: expected the envelope's source to be aws.s3, got {envelope}"
        );
        assert_eq!(
            envelope["detail-type"].as_str(),
            Some(sub_type.detail_type()),
            "case {case:?}: expected detail-type {:?}, got {envelope}",
            sub_type.detail_type()
        );
        assert_eq!(
            envelope["detail"]["bucket"]["name"].as_str(),
            Some(bucket.as_str()),
            "case {case:?}: expected detail.bucket.name {bucket:?}, got {envelope}"
        );
        assert_eq!(
            envelope["detail"]["object"]["key"].as_str(),
            Some(scoped_key.as_str()),
            "case {case:?}: expected detail.object.key {scoped_key:?}, got {envelope}"
        );
        assert_eq!(
            envelope["detail"]["request-id"].as_str(),
            Some(request_id.as_str()),
            "case {case:?}: expected detail.request-id {request_id:?}, got {envelope}"
        );

        // ...and the SAME call under the wildcard configuration's own prefix reaches it,
        // because the wildcard "stands for every event of that type": the sub-type the row
        // produces is one of them.
        let wildcard_answer = match row.call {
            Call::Tag(tags) => {
                emulator
                    .put_object_tagging_of(&bucket, &wildcard_key, tags)
                    .await
            }
            Call::Untag => delete_object_tagging(&emulator, &bucket, &wildcard_key).await,
        };
        let (wildcard_request_id, wildcard_extended_id) = wildcard_answer.correlation(case);
        let wildcard_published_key = format!("{WILDCARD_PREFIX}{}", row.published_key);
        let message = emulator
            .delivered(url_of(&classic, &EVERY_TAGGING.id))
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no notification reached the {:?} subscription's queue",
                    EVERY_TAGGING.id
                )
            });
        let record = &message["Records"][0];
        assert_eq!(
            record["eventName"].as_str(),
            Some(sub_type.record_event_name()),
            "case {case:?}: the {:?} subscription expected eventName {:?}, got {message}",
            EVERY_TAGGING.id,
            sub_type.record_event_name()
        );
        assert_eq!(
            record["s3"]["configurationId"].as_str(),
            Some(EVERY_TAGGING.id),
            "case {case:?}: expected the record to name the configuration it was delivered \
             for ({:?}), got {message}",
            EVERY_TAGGING.id
        );
        assert_eq!(
            record["s3"]["bucket"]["name"].as_str(),
            Some(bucket.as_str()),
            "case {case:?}: the {:?} subscription expected the bucket {bucket:?}, got \
             {message}",
            EVERY_TAGGING.id
        );
        assert_eq!(
            record["s3"]["object"]["key"].as_str(),
            Some(wildcard_published_key.as_str()),
            "case {case:?}: the {:?} subscription expected the key \
             {wildcard_published_key:?}, got {message}",
            EVERY_TAGGING.id
        );
        assert_eq!(
            record["responseElements"]["x-amz-request-id"].as_str(),
            Some(wildcard_request_id.as_str()),
            "case {case:?}: the {:?} subscription expected the call's own x-amz-request-id \
             {wildcard_request_id:?}, got {message}",
            EVERY_TAGGING.id
        );
        assert_eq!(
            record["responseElements"]["x-amz-id-2"].as_str(),
            Some(wildcard_extended_id.as_str()),
            "case {case:?}: the {:?} subscription expected the call's own x-amz-id-2 \
             {wildcard_extended_id:?}, got {message}",
            EVERY_TAGGING.id
        );

        // THE DIFFERENTIAL: the other sub-type's subscription and the other detail-type's
        // rule are not notified of this call at all.
        let other = sub_type.other();
        let quiet = emulator
            .nothing_delivered(url_of(&classic, &other.subscription().id))
            .await;
        assert!(
            quiet.is_empty(),
            "case {case:?}: the {:?} subscription names {:?} only, so it must receive \
             nothing for this call, got {quiet:?}",
            other.subscription().id,
            other.subscription().events
        );
        let quiet_bus = emulator.nothing_delivered(url_of(&bus, &other)).await;
        assert!(
            quiet_bus.is_empty(),
            "case {case:?}: the rule on {:?} must receive nothing for this call, got \
             {quiet_bus:?}",
            other.detail_type()
        );
    }
}
