// SPDX-License-Identifier: CC0-1.0

//! A subscription's `FilterPolicy` is evaluated ONCE, at publish time: a message the policy
//! in force at that moment rejected is dropped then and there, and relaxing the policy
//! afterwards redelivers nothing — while the next publish is delivered under the policy the
//! subscription holds by then.
//!
//! The RULE asserted here: "FilterPolicy – The simple JSON object that lets your subscriber
//! receive only a subset of messages, rather than receiving every message published to the
//! topic" (`SubscribeInput$Attributes` doc), applied to the message being published — SNS
//! "delivers the message to each endpoint that is subscribed to the topic" at the publish
//! ("When a messageId is returned, the message is saved and Amazon SNS immediately delivers
//! it to subscribers", `Publish` doc). So the policy decides ONE message's delivery at the
//! instant it is published: there is no per-subscription backlog a later write of the
//! attribute could release, and no message a withheld delivery leaves behind.
//!
//! The rows differ along what the rule generalises over: HOW the policy is relaxed (cleared
//! by an empty value, cleared by no value at all, cleared by an empty policy, widened, or
//! replaced by a condition of another operator — `exists`, `anything-but`, `prefix`,
//! `suffix`, `equals-ignore-case`, `wildcard`, `numeric`, `cidr`, `$or`), the scope the
//! policy is matched under (the message attributes and the message body), and whether the
//! control message is the one the first publish carried or another.
//!
//! Each row drives the whole path end to end over the wire, with SQS mounted on the same
//! engine, so what was delivered is read where a client reads it: on the queue.

use serde_json::json;

use super::sqs_subscription_delivery::{element, emulator, given_queue, sns, sns_call, sqs};

/// The policy in force when the first message is published, which that message does not
/// satisfy.
const RESTRICTIVE_POLICY: &str = r#"{"event":["ok"]}"#;

/// The message attribute the restrictive policy judges, as the withheld publish carries it.
const WITHHELD_ATTRIBUTE: Attribute = Attribute {
    name: "event",
    data_type: "String",
    value: "nope",
};

/// One message attribute of a publish.
#[derive(Debug, Clone, Copy)]
struct Attribute {
    name: &'static str,
    data_type: &'static str,
    value: &'static str,
}

/// One row: the relaxation written after the first publish was withheld, and the control
/// message published under it.
#[derive(Debug)]
struct Relaxation {
    /// What the row is called, which every assertion names.
    case: &'static str,
    /// The scope the subscription matches its policy under, or `None` for the default.
    scope: Option<&'static str>,
    /// The policy the subscription starts with.
    restrictive: &'static str,
    /// The policy written afterwards — `None` writes `SetSubscriptionAttributes` with no
    /// `AttributeValue` at all, which clears the attribute.
    relaxed: Option<&'static str>,
    /// The body the withheld publish carries.
    withheld_body: &'static str,
    /// The attributes the withheld publish carries.
    withheld_attributes: &'static [Attribute],
    /// The body the control publish carries, which the relaxed policy admits.
    control_body: &'static str,
    /// The attributes the control publish carries.
    control_attributes: &'static [Attribute],
}

/// The rows: one per way a policy is relaxed, over both scopes.
const RELAXATIONS: &[Relaxation] = &[
    Relaxation {
        case: "cleared by an empty value",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(""),
        withheld_body: "withheld-empty-value",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-empty-value",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "cleared by no value at all",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: None,
        withheld_body: "withheld-no-value",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-no-value",
        control_attributes: &[],
    },
    Relaxation {
        case: "cleared by an empty policy",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some("{}"),
        withheld_body: "withheld-empty-policy",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-empty-policy",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "widened to admit the value too",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"event":["ok","nope"]}"#),
        withheld_body: "withheld-widened",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-widened",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "replaced by an exists condition",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"event":[{"exists":true}]}"#),
        withheld_body: "withheld-exists",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-exists",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "replaced by an anything-but condition",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"event":[{"anything-but":"ok"}]}"#),
        withheld_body: "withheld-anything-but",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-anything-but",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "replaced by a prefix condition",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"event":[{"prefix":"no"}]}"#),
        withheld_body: "withheld-prefix",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-prefix",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "replaced by a suffix condition",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"event":[{"suffix":"pe"}]}"#),
        withheld_body: "withheld-suffix",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-suffix",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "replaced by a case-insensitive equality",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"event":[{"equals-ignore-case":"NOPE"}]}"#),
        withheld_body: "withheld-ignore-case",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-ignore-case",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "replaced by a wildcard condition",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"event":[{"wildcard":"n*e"}]}"#),
        withheld_body: "withheld-wildcard",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-wildcard",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "replaced by a numeric range on another attribute",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"count":[{"numeric":[">",1]}]}"#),
        withheld_body: "withheld-numeric",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-numeric",
        control_attributes: &[Attribute {
            name: "count",
            data_type: "Number",
            value: "5",
        }],
    },
    Relaxation {
        case: "replaced by a cidr condition on another attribute",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"ip":[{"cidr":"10.0.0.0/8"}]}"#),
        withheld_body: "withheld-cidr",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-cidr",
        control_attributes: &[Attribute {
            name: "ip",
            data_type: "String",
            value: "10.1.2.3",
        }],
    },
    Relaxation {
        case: "replaced by alternatives under $or",
        scope: None,
        restrictive: RESTRICTIVE_POLICY,
        relaxed: Some(r#"{"$or":[{"event":["ok"]},{"event":["nope"]}]}"#),
        withheld_body: "withheld-or",
        withheld_attributes: &[WITHHELD_ATTRIBUTE],
        control_body: "control-or",
        control_attributes: &[WITHHELD_ATTRIBUTE],
    },
    Relaxation {
        case: "a body-scoped policy widened",
        scope: Some("MessageBody"),
        restrictive: r#"{"kind":["ok"]}"#,
        relaxed: Some(r#"{"kind":["ok","control"]}"#),
        withheld_body: r#"{"kind":"nope"}"#,
        withheld_attributes: &[],
        control_body: r#"{"kind":"control"}"#,
        control_attributes: &[],
    },
    Relaxation {
        case: "a body-scoped policy cleared",
        scope: Some("MessageBody"),
        restrictive: r#"{"kind":["ok"]}"#,
        relaxed: Some(""),
        withheld_body: r#"{"kind":"nope"}"#,
        withheld_attributes: &[],
        control_body: "not-a-document-at-all",
        control_attributes: &[],
    },
];

/// Every row, each on its own topic, queue and subscription.
#[tokio::test]
async fn a_withheld_message_is_never_redelivered_when_the_policy_is_relaxed() {
    let emulator = emulator();
    for (at, row) in RELAXATIONS.iter().enumerate() {
        let case = row.case;
        let name = format!("relaxed-{at}");
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", &name)]).await,
            "TopicArn",
        );
        let (url, arn) = given_queue(&emulator, &format!("{name}-queue")).await;
        let mut attributes: Vec<(&str, &str)> = vec![("FilterPolicy", row.restrictive)];
        if let Some(scope) = row.scope {
            attributes.push(("FilterPolicyScope", scope));
        }
        let subscription = subscribe(&emulator, &topic, &arn, &attributes).await;

        // The publish the policy in force rejects: nothing is delivered, and nothing is
        // kept for the subscription either — which is what the rest of the row shows.
        publish(
            &emulator,
            &topic,
            row.withheld_body,
            row.withheld_attributes,
        )
        .await;
        let withheld = delivered_now(&emulator, &url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: the policy in force must withhold that publish, got {withheld:?}"
        );

        // The policy is relaxed, which releases no backlog: the queue is still empty.
        let mut write: Vec<(&str, &str)> = vec![
            ("SubscriptionArn", subscription.as_str()),
            ("AttributeName", "FilterPolicy"),
        ];
        if let Some(value) = row.relaxed {
            write.push(("AttributeValue", value));
        }
        let (status, body) = sns_call(&emulator, "SetSubscriptionAttributes", &write).await;
        assert_eq!(
            status, 200,
            "case {case:?}: relaxing the policy must be accepted, got {status} / {body}"
        );
        let after_relaxing = delivered_now(&emulator, &url).await;
        assert!(
            after_relaxing.is_empty(),
            "case {case:?}: relaxing the policy must redeliver nothing, got \
             {after_relaxing:?}"
        );

        // And the next publish is delivered under the policy the subscription holds now.
        publish(&emulator, &topic, row.control_body, row.control_attributes).await;
        let control = delivered_eventually(&emulator, &url, case).await;
        let carried: Vec<String> = control.iter().map(|body| message_of(body)).collect();
        assert_eq!(
            carried,
            vec![row.control_body.to_string()],
            "case {case:?}: the publish after the relaxation must be the only delivery, got \
             {carried:?}"
        );
    }
}

/// A publish withheld from one subscription is delivered to another whose policy admits it,
/// so what the policy decides is ONE subscription's delivery of ONE message.
#[tokio::test]
async fn the_policy_decides_each_subscriptions_own_delivery() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "per-subscription")]).await,
        "TopicArn",
    );
    let (filtered_url, filtered_arn) = given_queue(&emulator, "per-subscription-filtered").await;
    let (open_url, open_arn) = given_queue(&emulator, "per-subscription-open").await;
    let filtered = subscribe(
        &emulator,
        &topic,
        &filtered_arn,
        &[("FilterPolicy", RESTRICTIVE_POLICY)],
    )
    .await;
    subscribe(&emulator, &topic, &open_arn, &[]).await;

    publish(&emulator, &topic, "fanned-out", &[WITHHELD_ATTRIBUTE]).await;
    let withheld = delivered_now(&emulator, &filtered_url).await;
    assert!(
        withheld.is_empty(),
        "the filtered subscription must receive nothing, got {withheld:?}"
    );
    let open = delivered_eventually(&emulator, &open_url, "fan-out").await;
    let carried: Vec<String> = open.iter().map(|body| message_of(body)).collect();
    assert_eq!(
        carried,
        vec!["fanned-out".to_string()],
        "the unfiltered subscription must receive that same publish, got {carried:?}"
    );

    // Clearing the filtered subscription's policy delivers nothing of what it missed.
    sns(
        &emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", &filtered),
            ("AttributeName", "FilterPolicy"),
        ],
    )
    .await;
    let after_relaxing = delivered_now(&emulator, &filtered_url).await;
    assert!(
        after_relaxing.is_empty(),
        "clearing the policy must redeliver nothing, got {after_relaxing:?}"
    );
}

/// A policy written to a RUNNING subscription governs the publishes that follow it, and
/// clearing it again delivers nothing of what it withheld: the whole arc a client walks.
#[tokio::test]
async fn a_policy_written_after_the_subscribe_governs_the_publishes_that_follow() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "written-later")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "written-later-queue").await;
    let subscription = subscribe(&emulator, &topic, &arn, &[]).await;

    // With no policy, every publish is delivered.
    publish(&emulator, &topic, "before-the-policy", &[WITHHELD_ATTRIBUTE]).await;
    let before = delivered_eventually(&emulator, &url, "before the policy").await;
    assert_eq!(
        before.iter().map(|body| message_of(body)).collect::<Vec<_>>(),
        vec!["before-the-policy".to_string()],
        "an unfiltered subscription receives every publish, got {before:?}"
    );

    // The policy written now governs the next publish.
    sns(
        &emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", &subscription),
            ("AttributeName", "FilterPolicy"),
            ("AttributeValue", RESTRICTIVE_POLICY),
        ],
    )
    .await;
    publish(&emulator, &topic, "under-the-policy", &[WITHHELD_ATTRIBUTE]).await;
    let under = delivered_now(&emulator, &url).await;
    assert!(
        under.is_empty(),
        "the policy written after the subscribe must withhold that publish, got {under:?}"
    );

    // And clearing it releases no backlog while the next publish arrives.
    sns(
        &emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", &subscription),
            ("AttributeName", "FilterPolicy"),
            ("AttributeValue", ""),
        ],
    )
    .await;
    let released = delivered_now(&emulator, &url).await;
    assert!(
        released.is_empty(),
        "clearing the policy must redeliver nothing, got {released:?}"
    );
    publish(&emulator, &topic, "after-the-policy", &[WITHHELD_ATTRIBUTE]).await;
    let after = delivered_eventually(&emulator, &url, "after the policy").await;
    assert_eq!(
        after.iter().map(|body| message_of(body)).collect::<Vec<_>>(),
        vec!["after-the-policy".to_string()],
        "the publish after the policy was cleared must be the only delivery, got {after:?}"
    );
}

/// Subscribe the queue `arn` to `topic` with the attributes in `extra`, answering the
/// subscription's ARN.
async fn subscribe(
    emulator: &super::sqs_subscription_delivery::Emulator,
    topic: &str,
    arn: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Protocol".to_string(), "sqs".to_string()),
        ("Endpoint".to_string(), arn.to_string()),
        ("ReturnSubscriptionArn".to_string(), "true".to_string()),
    ];
    for (at, (name, value)) in extra.iter().enumerate() {
        let entry = at + 1;
        params.push((format!("Attributes.entry.{entry}.key"), (*name).to_string()));
        params.push((
            format!("Attributes.entry.{entry}.value"),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    element(
        &sns(emulator, "Subscribe", &borrowed).await,
        "SubscriptionArn",
    )
}

/// Publish `body` with `attributes` to `topic`.
async fn publish(
    emulator: &super::sqs_subscription_delivery::Emulator,
    topic: &str,
    body: &str,
    attributes: &[Attribute],
) {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Message".to_string(), body.to_string()),
    ];
    for (at, attribute) in attributes.iter().enumerate() {
        let entry = at + 1;
        params.push((
            format!("MessageAttributes.entry.{entry}.Name"),
            attribute.name.to_string(),
        ));
        params.push((
            format!("MessageAttributes.entry.{entry}.Value.DataType"),
            attribute.data_type.to_string(),
        ));
        params.push((
            format!("MessageAttributes.entry.{entry}.Value.StringValue"),
            attribute.value.to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns(emulator, "Publish", &borrowed).await;
}

/// Whatever notification is waiting on the queue at `url` NOW, as the text its subscriber
/// receives.
///
/// A publishing call is answered once its deliveries have been made, so what a queue holds
/// when that call returns is what the publish delivered to it: this is what an absence of
/// delivery is read from, without waiting for a span nothing is owed over.
async fn delivered_now(
    emulator: &super::sqs_subscription_delivery::Emulator,
    url: &str,
) -> Vec<String> {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// What the queue at `url` holds once a delivery has reached it: looked for again until it
/// has one, or until [`DELIVERY_DEADLINE`] has passed — the condition is waited FOR rather
/// than a span waited out.
async fn delivered_eventually(
    emulator: &super::sqs_subscription_delivery::Emulator,
    url: &str,
    case: &str,
) -> Vec<String> {
    let until = ::std::time::Instant::now() + DELIVERY_DEADLINE;
    loop {
        let held = delivered_now(emulator, url).await;
        if !held.is_empty() {
            return held;
        }
        assert!(
            ::std::time::Instant::now() < until,
            "case {case:?}: no delivery reached the queue"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// How long a delivery is looked for before the queue is reported empty: far longer than a
/// delivery made in process takes, so a loaded host changes no verdict.
const DELIVERY_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(60);

/// How long the poll above waits between looks at the queue.
const LOOK_AGAIN_AFTER: ::std::time::Duration = ::std::time::Duration::from_millis(20);

/// The published message the notification `body` carries.
fn message_of(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|envelope| {
            envelope
                .get("Message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.to_string())
}
