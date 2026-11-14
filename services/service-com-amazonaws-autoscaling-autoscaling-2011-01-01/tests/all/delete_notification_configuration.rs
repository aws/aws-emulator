// SPDX-License-Identifier: CC0-1.0

//! `DeleteNotificationConfiguration` over the awsQuery wire: taking one Amazon SNS
//! notification configuration off an Auto Scaling group.
//!
//! The RULE under test: a notification configuration is a child of ONE Auto Scaling group
//! keyed by the topic it names, so the delete takes the single entry the request's
//! (`AutoScalingGroupName`, `TopicARN`) pair keys — every event type that topic was
//! configured for goes with it — and leaves the group's other topics, and every other
//! group's entry for the same topic, exactly as they were. A pair the service holds no
//! configuration for names no notification to delete, so the call is refused
//! `ValidationError` (HTTP 400) rather than answered as a no-op, and the group's stored
//! configurations do not change.
//!
//! The rows span what the rule generalises over: how many event types the deleted topic
//! was configured for (one, several, the whole published catalog), how many topics the
//! group holds (one, three, more), which of them the request names (the first, a middle
//! one, the last), and, for the refusal, WHY no configuration answers the pair — the group
//! holds none at all, it holds other topics, the same topic was already deleted, or the
//! topic is configured on another group.
//!
//! Every row reads the group's configurations back through
//! `DescribeNotificationConfigurations`, so no row can pass on an accepted-but-unstored
//! change.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::lifecycle_hooks::create_group;
use crate::support::{borrowed, new_engine, p, query, router_with, tag_value, tag_values};

/// The event types this suite configures, spelled out here rather than imported, so a
/// wrong constant in the implementation cannot make a row pass. They are entries of the
/// catalog `DescribeAutoScalingNotificationTypes` publishes, documented on
/// `NotificationConfiguration$NotificationType`
/// (`smithy_query(query="doc", shape="NotificationConfiguration", member="NotificationType")`).
const TEST_NOTIFICATION: &str = "autoscaling:TEST_NOTIFICATION";
const INSTANCE_LAUNCH: &str = "autoscaling:EC2_INSTANCE_LAUNCH";
const INSTANCE_TERMINATE: &str = "autoscaling:EC2_INSTANCE_TERMINATE";
const INSTANCE_LAUNCH_ERROR: &str = "autoscaling:EC2_INSTANCE_LAUNCH_ERROR";

/// An Amazon SNS topic ARN of the Region these tests sign for, as a `TopicARN` member
/// carries one.
fn topic_arn(name: &str) -> String {
    format!(
        "arn:aws:sns:{}:{}:{name}",
        crate::support::REGION,
        engine::emulator_account_id()
    )
}

/// Configure `group` to notify `topic_arn` about `events`, and assert the write was
/// served.
async fn put_configuration(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    topic_arn: &str,
    events: &[&str],
) {
    let mut params = vec![p("AutoScalingGroupName", group), p("TopicARN", topic_arn)];
    for (index, event) in events.iter().enumerate() {
        params.push(p(
            format!("NotificationTypes.member.{}", index + 1).as_str(),
            *event,
        ));
    }
    let (status, body) = query(
        router,
        "PutNotificationConfiguration",
        &borrowed(&params),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutNotificationConfiguration for {topic_arn} expected 200, got \
         {status}: {body}"
    );
}

/// Post one `DeleteNotificationConfiguration` and answer its status, wire error code and
/// message.
async fn delete_configuration(
    router: &MultiServiceRouter,
    group: &str,
    topic_arn: &str,
) -> (u16, String, String) {
    let (status, body) = query(
        router,
        "DeleteNotificationConfiguration",
        &[("AutoScalingGroupName", group), ("TopicARN", topic_arn)],
    )
    .await;
    (
        status,
        tag_value(&body, "Code"),
        crate::support::xml_unescape(&tag_value(&body, "Message")),
    )
}

/// Every `(topic, event type)` pair `group` is configured with right now, as
/// `DescribeNotificationConfigurations` reports them.
async fn configured(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
) -> Vec<(String, String)> {
    let (status, body) = query(
        router,
        "DescribeNotificationConfigurations",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeNotificationConfigurations expected 200, got {status}: {body}"
    );
    let topics = tag_values(&body, "TopicARN");
    let types = tag_values(&body, "NotificationType");
    assert_eq!(
        topics.len(),
        types.len(),
        "case {case:?}: one topic per reported event type, got {topics:?} and {types:?}"
    );
    let mut pairs: Vec<(String, String)> = topics.into_iter().zip(types).collect();
    pairs.sort();
    pairs
}

/// The `(topic, event type)` pairs a configuration of `events` on `topic` amounts to.
fn pairs_of(topic: &str, events: &[&str]) -> Vec<(String, String)> {
    events
        .iter()
        .map(|event| (topic.to_string(), (*event).to_string()))
        .collect()
}

/// One group's configurations as a row states them: the topic suffix, and the event types
/// it is told about.
type Configured<'a> = (&'a str, &'a [&'a str]);

/// The delete takes the whole entry its (group, topic) pair keys, and only that entry.
#[tokio::test]
async fn delete_takes_the_named_topics_whole_configuration() {
    // (case, the configurations the group is given, the index of the one deleted) — the
    // rows differ in how many event types the deleted topic carried, how many topics the
    // group holds, and which of them the request names.
    let catalog: &[&str] = autoscaling::state::AUTO_SCALING_NOTIFICATION_TYPES;
    let rows: Vec<(&str, Vec<Configured>, usize)> = vec![
        (
            "the group's only topic, told about one event",
            vec![("solo", &[TEST_NOTIFICATION][..])],
            0,
        ),
        (
            "the group's only topic, told about the whole published catalog",
            vec![("every-event", catalog)],
            0,
        ),
        (
            "the first of three topics",
            vec![
                ("first", &[INSTANCE_LAUNCH][..]),
                ("second", &[INSTANCE_TERMINATE][..]),
                ("third", &[TEST_NOTIFICATION][..]),
            ],
            0,
        ),
        (
            "a middle topic of three, told about several events",
            vec![
                ("first", &[INSTANCE_LAUNCH][..]),
                (
                    "second",
                    &[INSTANCE_LAUNCH, INSTANCE_TERMINATE, TEST_NOTIFICATION][..],
                ),
                ("third", &[TEST_NOTIFICATION][..]),
            ],
            1,
        ),
        (
            "the last of three topics",
            vec![
                ("first", &[INSTANCE_LAUNCH][..]),
                ("second", &[INSTANCE_TERMINATE][..]),
                ("third", &[INSTANCE_LAUNCH_ERROR, TEST_NOTIFICATION][..]),
            ],
            2,
        ),
        (
            "one of five topics",
            vec![
                ("a", &[INSTANCE_LAUNCH][..]),
                ("b", &[INSTANCE_TERMINATE][..]),
                ("c", &[TEST_NOTIFICATION, INSTANCE_LAUNCH_ERROR][..]),
                ("d", &[INSTANCE_LAUNCH_ERROR][..]),
                ("e", &[TEST_NOTIFICATION][..]),
            ],
            3,
        ),
    ];

    for (row, (case, configurations, deleted)) in rows.iter().enumerate() {
        let router = router_with(new_engine());
        let group = format!("group-{row}");
        create_group(&router, &group).await;

        let topics: Vec<String> = configurations
            .iter()
            .map(|(suffix, _)| topic_arn(&format!("{group}-{suffix}")))
            .collect();
        for (topic, (_, events)) in topics.iter().zip(configurations.iter()) {
            put_configuration(&router, case, &group, topic, events).await;
        }

        let before = configured(&router, case, &group).await;
        let mut expected_before: Vec<(String, String)> = topics
            .iter()
            .zip(configurations.iter())
            .flat_map(|(topic, (_, events))| pairs_of(topic, events))
            .collect();
        expected_before.sort();
        assert_eq!(
            before, expected_before,
            "case {case:?}: the stored configurations must be what was written, got {before:?}"
        );

        let doomed = &topics[*deleted];
        let (status, code, message) = delete_configuration(&router, &group, doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting {doomed} expected 200, got {status} with \
             {code:?} {message:?}"
        );

        let after = configured(&router, case, &group).await;
        let mut expected_after: Vec<(String, String)> = topics
            .iter()
            .zip(configurations.iter())
            .filter(|(topic, _)| *topic != doomed)
            .flat_map(|(topic, (_, events))| pairs_of(topic, events))
            .collect();
        expected_after.sort();
        assert_eq!(
            after, expected_after,
            "case {case:?}: only {doomed} must be gone, got {after:?}"
        );
        assert!(
            after.iter().all(|(topic, _)| topic != doomed),
            "case {case:?}: {doomed} must be reported no more, got {after:?}"
        );
    }
}

/// A pair the service holds no configuration for is refused, and nothing is removed.
#[tokio::test]
async fn deleting_a_configuration_the_group_does_not_hold_is_refused() {
    let held = "held";
    // (case, the topic suffix the group is configured with or `None` for a group with no
    // configuration at all, the suffix the request names, whether the named one is deleted
    // first) — the rows differ in WHY no configuration answers the pair.
    let rows: &[(&str, Option<&str>, &str, bool)] = &[
        ("a group configured with nothing at all", None, "absent", false),
        (
            "a topic the group was never configured with",
            Some(held),
            "never-configured",
            false,
        ),
        (
            "a topic whose configuration this caller already deleted",
            Some(held),
            held,
            true,
        ),
    ];

    for (row, (case, configured_suffix, named, delete_first)) in rows.iter().enumerate() {
        let router = router_with(new_engine());
        let group = format!("refusal-group-{row}");
        create_group(&router, &group).await;

        let stored = configured_suffix.map(|suffix| topic_arn(&format!("{group}-{suffix}")));
        if let Some(topic) = &stored {
            put_configuration(&router, case, &group, topic, &[TEST_NOTIFICATION]).await;
        }
        let requested = topic_arn(&format!("{group}-{named}"));
        if *delete_first {
            let (status, code, message) = delete_configuration(&router, &group, &requested).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first delete of {requested} expected 200, got {status} \
                 with {code:?} {message:?}"
            );
        }

        let before = configured(&router, case, &group).await;
        let (status, code, message) = delete_configuration(&router, &group, &requested).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting {requested} expected 400, got {status} with \
             {code:?} {message:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected ValidationError, got {code:?} with {message:?}"
        );
        assert_eq!(
            message,
            autoscaling::state::absent_notification_configuration_message(&requested),
            "case {case:?}: unexpected refusal wording, got {message:?}"
        );
        assert!(
            message.contains(&requested),
            "case {case:?}: the refusal names the topic it was asked for, got {message:?}"
        );
        let after = configured(&router, case, &group).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete changes nothing, got {after:?}"
        );
    }
}

/// The configuration is keyed by the PAIR: the same topic on two groups is two
/// configurations, and deleting one leaves the other.
#[tokio::test]
async fn the_configuration_deleted_is_the_pairs_own() {
    let router = router_with(new_engine());
    let case = "one topic configured on two groups";
    let (kept, deleted_from) = ("keeps-its-topic", "loses-its-topic");
    create_group(&router, kept).await;
    create_group(&router, deleted_from).await;

    let shared = topic_arn("shared-topic");
    let events = [INSTANCE_LAUNCH, TEST_NOTIFICATION];
    put_configuration(&router, case, kept, &shared, &events).await;
    put_configuration(&router, case, deleted_from, &shared, &events).await;

    let (status, code, message) = delete_configuration(&router, deleted_from, &shared).await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting {shared} from {deleted_from} expected 200, got {status} \
         with {code:?} {message:?}"
    );

    let gone = configured(&router, case, deleted_from).await;
    assert!(
        gone.is_empty(),
        "case {case:?}: {deleted_from} must hold no configuration, got {gone:?}"
    );
    let mut still_there = pairs_of(&shared, &events);
    still_there.sort();
    assert_eq!(
        configured(&router, case, kept).await,
        still_there,
        "case {case:?}: {kept} keeps its own configuration for {shared}"
    );

    // The group that lost it is now in the same position as a group that never had it: a
    // second delete of the same pair is refused.
    let (status, code, message) = delete_configuration(&router, deleted_from, &shared).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationError"),
        "case {case:?}: a repeated delete is refused, got {status} with {code:?} {message:?}"
    );
}

/// A name no Auto Scaling group answers to is refused the way this service's group-scoped
/// writes refuse one, and the groups it does hold keep their configurations.
#[tokio::test]
async fn deleting_from_a_group_the_service_does_not_hold_is_refused() {
    let router = router_with(new_engine());
    let case = "a group that was never created";
    let group = "the-only-group";
    create_group(&router, group).await;
    let topic = topic_arn("the-only-topic");
    put_configuration(&router, case, group, &topic, &[TEST_NOTIFICATION]).await;

    for absent in ["no-such-group", "another-name-no-group-answers-to"] {
        let (status, code, message) = delete_configuration(&router, absent, &topic).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting from {absent:?} expected 400, got {status} with \
             {code:?} {message:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected ValidationError for {absent:?}, got {code:?} with \
             {message:?}"
        );
    }

    let mut expected = pairs_of(&topic, &[TEST_NOTIFICATION]);
    expected.sort();
    assert_eq!(
        configured(&router, case, group).await,
        expected,
        "case {case:?}: the group this service does hold keeps its configuration"
    );
}
