// SPDX-License-Identifier: CC0-1.0

//! The ALERTING monitor of a domain, driven over the real wire path, with Amazon SNS and
//! Amazon SQS mounted beside this service on ONE shared engine — so a notification is
//! asserted where a client sees it: on the queue subscribed to the channel's topic.
//!
//! Three rules, each over a SET of inputs rather than one recorded call:
//!
//! * **a monitor's trigger fires exactly when its own condition holds over the results of
//!   that monitor's run, and an action of a firing trigger publishes its RENDERED message to
//!   the channel its destination names** — so a trigger whose condition is false publishes
//!   nothing at all. The rows differ along the condition (a comparison on the total, an
//!   equality, a conjunction, a negation, a condition over a query that selects a subset),
//!   the documents the index holds, the template (a count, the monitor's and the trigger's
//!   own names, a literal carrying no substitution), and the channel kind (a notifications
//!   plugin channel and a legacy alerting destination, each with its own topic and queue);
//! * **a monitor is a revisioned record**: the create answers `201` with the revision
//!   counters of the write and the stored document, with an id minted for every trigger and
//!   action that arrived without one, and a read reports the same document back — and a
//!   monitor the write cannot store (one missing a member a run needs, or carrying more of
//!   something than this cluster runs) is refused;
//! * **a dry run decides the trigger and sends nothing**: it reports what each action would
//!   render, publishes no notification and leaves no alert.
//!
//! and one more the second of those rests on: **each run is decided by the documents the
//! index holds AT THAT RUN**, so emptying the index with `_delete_by_query` turns a trigger
//! that fired into one that withholds its notification, and completes the alert its last
//! firing left. Its rows differ along the query the delete selects by.
//!
//! A fourth test pins the BOUNDS the cluster holds a monitor's own source to: a condition
//! that never returns, and a template whose work the cluster will not do — a block nested in
//! either spelling, or partials that reference each other — are each reported where the
//! plugin reports a failure, and none publishes anything.
//!
//! The channel configuration and the monitor are written through the plugins' own APIs, the
//! topic and the queue through Amazon SNS's and Amazon SQS's own operations, and the
//! notification is read off the queue — nothing here reaches into a store to arrange what it
//! then asserts. The harness is this crate's own ([`super::domain_lifecycle`],
//! [`super::domain_rest_data_plane`]).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{Emulator, REGION};
use super::domain_rest_data_plane::rest_prefixed;

/// How long a wait for a notification to arrive on the subscribed queue may take at most.
///
/// Generous: the publish happens in process, before the request that ran the monitor is
/// answered, so the first look finds it — this is only what keeps a loaded machine from
/// hanging the suite, and nothing here asserts on how long anything took.
const ARRIVAL_DEADLINE: Duration = Duration::from_secs(30);

/// The index every row below queries.
const INDEX: &str = "alerting-source";

/// An index holding no document at all, for the rows whose condition must not hold.
const EMPTY_INDEX: &str = "alerting-empty";

/// The Amazon SNS client for this emulator, over the in-process callback.
fn sns(emulator: &Emulator) -> ::aws_sdk_sns::Client {
    let callback = emulator.callback();
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://sns.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

/// The Amazon SQS client for this emulator, over the same callback.
fn sqs(emulator: &Emulator) -> ::aws_sdk_sqs::Client {
    let callback = emulator.callback();
    let config = ::aws_sdk_sqs::Config::builder()
        .behavior_version(::aws_sdk_sqs::config::BehaviorVersion::latest())
        .region(::aws_sdk_sqs::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://sqs.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_sqs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sqs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sqs::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sqs::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sqs::Client::from_conf(config)
}

/// One notification target: the topic a channel publishes to, and the queue subscribed to
/// it that the notification is read off.
struct Target {
    topic_arn: String,
    queue_url: String,
}

/// A topic named `name`, with a queue of the same name subscribed to it.
///
/// The queue carries the policy a subscribed queue has to carry — "you must ensure that the
/// Amazon SQS queue policy allows the Amazon SNS topic to send messages to the queue" — so
/// the delivery is one Amazon SNS is allowed to make.
async fn given_target(emulator: &Emulator, name: &str) -> Target {
    let account = ::engine::emulator_account_id();
    let queue_arn = format!("arn:aws:sqs:{REGION}:{account}:{name}");
    let topic_arn = format!("arn:aws:sns:{REGION}:{account}:{name}");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": topic_arn } },
        }],
    })
    .to_string();
    let queue = sqs(emulator)
        .create_queue()
        .queue_name(name)
        .attributes(::aws_sdk_sqs::types::QueueAttributeName::Policy, policy)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: CreateQueue({name}) failed: {error:?}"));
    let queue_url = queue
        .queue_url()
        .unwrap_or_else(|| panic!("setup: CreateQueue({name}) answered no queue url"))
        .to_string();
    let created = sns(emulator)
        .create_topic()
        .name(name)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: CreateTopic({name}) failed: {error:?}"));
    let answered = created
        .topic_arn()
        .unwrap_or_else(|| panic!("setup: CreateTopic({name}) answered no ARN"))
        .to_string();
    assert_eq!(
        answered, topic_arn,
        "setup: CreateTopic({name}) answered an ARN this suite does not name"
    );
    sns(emulator)
        .subscribe()
        .topic_arn(&topic_arn)
        .protocol("sqs")
        .endpoint(&queue_arn)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: Subscribe({name}) failed: {error:?}"));
    Target {
        topic_arn,
        queue_url,
    }
}

/// Every message now on `queue`, with the Amazon SNS notification envelope parsed — the
/// `Message` each carries is what the trigger's action published.
async fn received(emulator: &Emulator, queue: &str) -> Vec<Value> {
    let answered = sqs(emulator)
        .receive_message()
        .queue_url(queue)
        .max_number_of_messages(10)
        .send()
        .await
        .unwrap_or_else(|error| panic!("ReceiveMessage({queue}) failed: {error:?}"));
    answered
        .messages()
        .iter()
        .map(|message| {
            let body = message.body().unwrap_or_default();
            ::serde_json::from_str::<Value>(body).unwrap_or_else(|error| {
                panic!("a delivered notification is not JSON ({error}): {body:?}")
            })
        })
        .collect()
}

/// The ONE notification the firing trigger published, waited for until it is there.
///
/// The wait is for the CONDITION — a message on the queue — and not for a span of time: the
/// publish is in process and already happened by the time the run answered, so this looks
/// again only to keep a loaded machine from failing the suite.
async fn published_message(emulator: &Emulator, case: &str, queue: &str) -> String {
    let deadline = Instant::now() + ARRIVAL_DEADLINE;
    loop {
        let messages = received(emulator, queue).await;
        if let [one] = messages.as_slice() {
            return one["Message"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a notification carrying a message, got {one:?}")
                })
                .to_string();
        }
        assert!(
            messages.len() <= 1,
            "case {case:?}: expected one notification on the queue, got {}",
            messages.len()
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no notification arrived on the subscribed queue"
        );
        ::tokio::task::yield_now().await;
    }
}

/// The monitor document a row asks for.
fn monitor_document(
    name: &str,
    index: &str,
    query: Value,
    condition: &str,
    destination_id: &str,
    message: &str,
) -> Value {
    json!({
        "type": "monitor",
        "name": name,
        "monitor_type": "query_level_monitor",
        "enabled": true,
        "schedule": { "period": { "interval": 1, "unit": "MINUTES" } },
        "inputs": [{ "search": { "indices": [index], "query": query } }],
        "triggers": [{
            "query_level_trigger": {
                "name": format!("{name}-trigger"),
                "severity": "1",
                "condition": {
                    "script": { "source": condition, "lang": "painless" },
                },
                "actions": [{
                    "name": format!("{name}-action"),
                    "destination_id": destination_id,
                    "subject_template": {
                        "source": format!("{name} fired"),
                        "lang": "mustache",
                    },
                    "message_template": { "source": message, "lang": "mustache" },
                }],
            },
        }],
    })
}

/// `POST /_plugins/_alerting/monitors`, answering the id the write minted.
async fn given_monitor(emulator: &Emulator, domain: &str, case: &str, document: Value) -> String {
    let created = rest_prefixed(
        emulator,
        domain,
        "POST",
        "/_plugins/_alerting/monitors",
        Some(document),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected 201 from the monitor create, got {} with {:?}",
        created.status, created.body
    );
    created.body["_id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an _id, got {:?}", created.body))
        .to_string()
}

/// `POST /_plugins/_notifications/configs` for an Amazon SNS channel on `topic_arn`,
/// answering the `config_id` the create minted.
async fn given_channel(emulator: &Emulator, domain: &str, name: &str, topic_arn: &str) -> String {
    let created = rest_prefixed(
        emulator,
        domain,
        "POST",
        "/_plugins/_notifications/configs",
        Some(json!({
            "config": {
                "name": name,
                "description": "the suite's own channel",
                "config_type": "sns",
                "is_enabled": true,
                "sns": {
                    "topic_arn": topic_arn,
                    "role_arn": format!(
                        "arn:aws:iam::{}:role/alerting-publisher",
                        ::engine::emulator_account_id()
                    ),
                },
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating the notification channel {name:?} returned {} with {:?}",
        created.status, created.body
    );
    created.body["config_id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the channel create answered no config_id: {:?}", created.body))
        .to_string()
}

/// `POST /_plugins/_alerting/destinations` for a LEGACY Amazon SNS destination on
/// `topic_arn`, answering the id the create minted.
async fn given_destination(
    emulator: &Emulator,
    domain: &str,
    name: &str,
    topic_arn: &str,
) -> String {
    let created = rest_prefixed(
        emulator,
        domain,
        "POST",
        "/_plugins/_alerting/destinations",
        Some(json!({
            "name": name,
            "type": "sns",
            "sns": {
                "topic_arn": topic_arn,
                "role_arn": format!(
                    "arn:aws:iam::{}:role/alerting-publisher",
                    ::engine::emulator_account_id()
                ),
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: creating the legacy destination {name:?} returned {} with {:?}",
        created.status, created.body
    );
    created.body["_id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("setup: the destination create answered no _id: {:?}", created.body)
        })
        .to_string()
}

/// Write `documents` into `index`, each by its own id, and assert each landed.
async fn given_documents(emulator: &Emulator, domain: &str, index: &str, documents: &[Value]) {
    let created = rest_prefixed(emulator, domain, "PUT", &format!("/{index}"), None).await;
    assert_eq!(
        created.status, 200,
        "setup: creating index {index:?} returned {} with {:?}",
        created.status, created.body
    );
    for (at, document) in documents.iter().enumerate() {
        let written = rest_prefixed(
            emulator,
            domain,
            "PUT",
            &format!("/{index}/_doc/{at}?refresh=true"),
            Some(document.clone()),
        )
        .await;
        assert!(
            written.status == 201 || written.status == 200,
            "setup: writing document {at} of {index:?} returned {} with {:?}",
            written.status,
            written.body
        );
    }
}

/// One row of the trigger table.
struct Row {
    case: &'static str,
    /// The index the monitor queries.
    index: &'static str,
    /// The query its input asks.
    query: Value,
    /// The trigger condition, as the caller wrote it.
    condition: &'static str,
    /// The message template the action sends.
    message: &'static str,
    /// Whether the condition holds over the run's results.
    fires: bool,
    /// What the template renders to, for a row that fires.
    rendered: String,
    /// Whether the action addresses the LEGACY destination rather than the channel.
    legacy_destination: bool,
}

/// The table: the conditions a trigger is written with, over the results its own query
/// produces. Three documents are in [`INDEX`], two of them `status: "error"`, and
/// [`EMPTY_INDEX`] holds none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a total above zero, with the count rendered into the message",
            index: INDEX,
            query: json!({ "size": 0, "query": { "match_all": {} } }),
            condition: "ctx.results[0].hits.total.value > 0",
            message: "there are {{ctx.results.0.hits.total.value}} documents",
            fires: true,
            rendered: "there are 3 documents".to_string(),
            legacy_destination: false,
        },
        Row {
            case: "the same condition over an index holding nothing",
            index: EMPTY_INDEX,
            query: json!({ "size": 0, "query": { "match_all": {} } }),
            condition: "ctx.results[0].hits.total.value > 0",
            message: "there are {{ctx.results.0.hits.total.value}} documents",
            fires: false,
            rendered: String::new(),
            legacy_destination: false,
        },
        Row {
            case: "an equality on the total that holds",
            index: INDEX,
            query: json!({ "size": 0, "query": { "match_all": {} } }),
            condition: "ctx.results[0].hits.total.value == 3",
            message: "{{ctx.monitor.name}}/{{ctx.trigger.name}} saw \
                      {{ctx.results.0.hits.total.value}}",
            fires: true,
            rendered: "equal-total/equal-total-trigger saw 3".to_string(),
            legacy_destination: false,
        },
        Row {
            case: "an equality on the total that does not hold",
            index: INDEX,
            query: json!({ "size": 0, "query": { "match_all": {} } }),
            condition: "ctx.results[0].hits.total.value == 7",
            message: "never sent",
            fires: false,
            rendered: String::new(),
            legacy_destination: false,
        },
        Row {
            case: "a conjunction over a query selecting a subset",
            index: INDEX,
            query: json!({ "size": 0, "query": { "term": { "status": "error" } } }),
            condition: "ctx.results[0].hits.total.value > 1 && \
                        ctx.results[0].hits.total.value < 3",
            message: "{{ctx.results.0.hits.total.value}} failing",
            fires: true,
            rendered: "2 failing".to_string(),
            legacy_destination: false,
        },
        Row {
            case: "a negation that does not hold",
            index: INDEX,
            query: json!({ "size": 0, "query": { "term": { "status": "error" } } }),
            condition: "!(ctx.results[0].hits.total.value > 1)",
            message: "never sent",
            fires: false,
            rendered: String::new(),
            legacy_destination: false,
        },
        Row {
            case: "a template carrying no substitution, sent to the legacy destination",
            index: INDEX,
            query: json!({ "size": 0, "query": { "match_all": {} } }),
            condition: "ctx.results[0].hits.total.value >= 3",
            message: "the index is not empty",
            fires: true,
            rendered: "the index is not empty".to_string(),
            legacy_destination: true,
        },
        Row {
            case: "a condition written as statements ending in a return",
            index: INDEX,
            query: json!({ "size": 0, "query": { "match_all": {} } }),
            condition: "return ctx.results[0].hits.total.value > 2;",
            message: "statements saw {{ctx.results.0.hits.total.value}}",
            fires: true,
            rendered: "statements saw 3".to_string(),
            legacy_destination: true,
        },
        Row {
            case: "a condition the cluster cannot compile at all",
            index: INDEX,
            query: json!({ "size": 0, "query": { "match_all": {} } }),
            condition: "ctx.results[0].hits.total.",
            message: "never sent",
            fires: false,
            rendered: String::new(),
            legacy_destination: false,
        },
        Row {
            case: "a condition that is not a boolean at all",
            index: INDEX,
            query: json!({ "size": 0, "query": { "match_all": {} } }),
            condition: "ctx.results[0].hits.total.value",
            message: "never sent",
            fires: false,
            rendered: String::new(),
            legacy_destination: false,
        },
    ]
}

/// The documents [`INDEX`] holds.
fn source_documents() -> Vec<Value> {
    vec![
        json!({ "status": "error", "service": "checkout" }),
        json!({ "status": "error", "service": "search" }),
        json!({ "status": "ok", "service": "checkout" }),
    ]
}

/// RULE: a monitor's trigger fires exactly when its own condition holds over the results of
/// that monitor's run, and an action of a firing trigger publishes its rendered message to
/// the channel its destination names — so a trigger whose condition is false publishes
/// nothing at all.
#[tokio::test]
async fn a_trigger_publishes_its_rendered_message_only_while_its_condition_holds() {
    let emulator = Emulator::with_notification_channels();
    let domain = "alerting-domain";
    created_domain(&emulator, domain).await;
    given_documents(&emulator, domain, INDEX, &source_documents()).await;
    given_documents(&emulator, domain, EMPTY_INDEX, &[]).await;

    let channel_target = given_target(&emulator, "alerting-channel").await;
    let legacy_target = given_target(&emulator, "alerting-legacy").await;
    let channel_id = given_channel(
        &emulator,
        domain,
        "alerting-channel",
        &channel_target.topic_arn,
    )
    .await;
    let destination_id = given_destination(
        &emulator,
        domain,
        "alerting-legacy",
        &legacy_target.topic_arn,
    )
    .await;

    for (at, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let name = match row.case {
            "an equality on the total that holds" => "equal-total".to_string(),
            "a condition written as statements ending in a return" => "statements".to_string(),
            _ => format!("monitor-{at}"),
        };
        let target = if row.legacy_destination {
            &legacy_target
        } else {
            &channel_target
        };
        let destination = if row.legacy_destination {
            &destination_id
        } else {
            &channel_id
        };
        let monitor_id = given_monitor(
            &emulator,
            domain,
            case,
            monitor_document(
                &name,
                row.index,
                row.query.clone(),
                row.condition,
                destination,
                row.message,
            ),
        )
        .await;

        let run = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/_plugins/_alerting/monitors/{monitor_id}/_execute"),
            None,
        )
        .await;
        assert_eq!(
            run.status, 200,
            "case {case:?}: expected 200 from the run, got {} with {:?}",
            run.status, run.body
        );
        let results = run.body["trigger_results"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected trigger results, got {:?}", run.body));
        let (_, result) = results
            .iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected one trigger result, got {results:?}"));
        assert_eq!(
            result["triggered"].as_bool(),
            Some(row.fires),
            "case {case:?}: expected triggered {} , got {result:?}",
            row.fires
        );

        if row.fires {
            // What the action rendered is reported by the run AND published to the channel's
            // topic, which is where the subscribed queue reads it.
            let action = result["action_results"]
                .as_object()
                .and_then(|actions| actions.values().next().cloned())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected an action result, got {result:?}")
                });
            assert_eq!(
                action["output"]["message"].as_str(),
                Some(row.rendered.as_str()),
                "case {case:?}: expected the rendered message, got {action:?}"
            );
            assert_eq!(
                action["error"],
                Value::Null,
                "case {case:?}: expected the action to report no error, got {action:?}"
            );
            let delivered = published_message(&emulator, case, &target.queue_url).await;
            assert_eq!(
                delivered, row.rendered,
                "case {case:?}: expected the queue subscribed to the channel's topic to carry \
                 the rendered message, got {delivered:?}"
            );
            // The firing left an ACTIVE alert behind, which the alerting API reports.
            let alerts = rest_prefixed(
                &emulator,
                domain,
                "GET",
                &format!("/_plugins/_alerting/monitors/alerts?monitorId={monitor_id}"),
                None,
            )
            .await;
            assert_eq!(
                alerts.status, 200,
                "case {case:?}: expected 200 from the alert listing, got {} with {:?}",
                alerts.status, alerts.body
            );
            let states: Vec<&str> = alerts.body["alerts"]
                .as_array()
                .map(|alerts| {
                    alerts
                        .iter()
                        .filter_map(|alert| alert["state"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                states,
                vec!["ACTIVE"],
                "case {case:?}: expected one ACTIVE alert, got {:?}",
                alerts.body
            );
        } else {
            // Nothing is published for a condition that does not hold: the publish happens
            // before the run answers, so a queue that is empty now was never written to.
            let delivered = received(&emulator, &target.queue_url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: expected the trigger to withhold its notification, got \
                 {delivered:?} on the queue"
            );
            let alerts = rest_prefixed(
                &emulator,
                domain,
                "GET",
                &format!("/_plugins/_alerting/monitors/alerts?monitorId={monitor_id}"),
                None,
            )
            .await;
            assert_eq!(
                alerts.body["totalAlerts"].as_u64(),
                Some(0),
                "case {case:?}: expected a trigger that did not fire to leave no alert, got \
                 {:?}",
                alerts.body
            );
        }
    }
}

/// RULE: a monitor is a revisioned record — the create answers `201` with the revision
/// counters of the write and the stored document, with an id minted for every trigger and
/// action that arrived without one, and a read reports the same document back.
#[tokio::test]
async fn a_created_monitor_is_revisioned_and_read_back_with_the_ids_the_write_minted() {
    let emulator = Emulator::with_notification_channels();
    let domain = "monitor-records";
    created_domain(&emulator, domain).await;
    given_documents(&emulator, domain, INDEX, &source_documents()).await;
    let target = given_target(&emulator, "monitor-records-topic").await;
    let channel_id = given_channel(&emulator, domain, "records", &target.topic_arn).await;

    // The channel reads back through the plugin's own get, as it was written.
    let read_channel = rest_prefixed(
        &emulator,
        domain,
        "GET",
        &format!("/_plugins/_notifications/configs/{channel_id}"),
        None,
    )
    .await;
    assert_eq!(
        read_channel.status, 200,
        "case channel read: expected 200, got {} with {:?}",
        read_channel.status, read_channel.body
    );
    assert_eq!(
        read_channel.body["config_list"][0]["config"]["sns"]["topic_arn"].as_str(),
        Some(target.topic_arn.as_str()),
        "case channel read: expected the channel to carry the topic it was created with, got \
         {:?}",
        read_channel.body
    );

    let document = monitor_document(
        "records",
        INDEX,
        json!({ "size": 0, "query": { "match_all": {} } }),
        "ctx.results[0].hits.total.value > 0",
        &channel_id,
        "{{ctx.results.0.hits.total.value}}",
    );
    let created = rest_prefixed(
        &emulator,
        domain,
        "POST",
        "/_plugins/_alerting/monitors",
        Some(document.clone()),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case monitor create: expected 201, got {} with {:?}",
        created.status, created.body
    );
    assert_eq!(
        (
            created.body["_version"].as_i64(),
            created.body["_primary_term"].as_i64(),
        ),
        (Some(1), Some(1)),
        "case monitor create: expected the revision counters of a created monitor, got {:?}",
        created.body
    );
    let monitor_id = created.body["_id"]
        .as_str()
        .expect("the create answers an id")
        .to_string();
    let trigger = &created.body["monitor"]["triggers"][0]["query_level_trigger"];
    let trigger_id = trigger["id"].as_str().unwrap_or_default().to_string();
    let action_id = trigger["actions"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !trigger_id.is_empty() && !action_id.is_empty() && trigger_id != action_id,
        "case monitor create: expected an id minted for the trigger and for its action, got \
         {trigger:?}"
    );

    let read = rest_prefixed(
        &emulator,
        domain,
        "GET",
        &format!("/_plugins/_alerting/monitors/{monitor_id}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case monitor read: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["monitor"]["name"].as_str(),
        Some("records"),
        "case monitor read: expected the stored monitor, got {:?}",
        read.body
    );
    assert_eq!(
        read.body["monitor"]["triggers"][0]["query_level_trigger"]["id"]
            .as_str()
            .unwrap_or_default(),
        trigger_id,
        "case monitor read: expected the same trigger id the write minted, got {:?}",
        read.body
    );
    assert!(
        read.body["monitor"]["last_update_time"]
            .as_i64()
            .is_some_and(|at| at > 0),
        "case monitor read: expected the instant the write stamped, got {:?}",
        read.body
    );

    // A monitor the domain does not hold is not found, whichever way it is addressed.
    for (case, method) in [("read", "GET"), ("delete", "DELETE")] {
        let answer = rest_prefixed(
            &emulator,
            domain,
            method,
            "/_plugins/_alerting/monitors/no-such-monitor",
            None,
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case missing monitor {case:?}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // A monitor the write cannot store is refused, naming what is wrong with it: the members
    // a run needs, and the bounds this cluster runs one within.
    let bounded = |triggers: usize, inputs: usize| {
        let mut document = monitor_document(
            "bounded",
            INDEX,
            json!({ "size": 0, "query": { "match_all": {} } }),
            "ctx.results[0].hits.total.value > 0",
            &channel_id,
            "x",
        );
        let trigger = document["triggers"][0].clone();
        let input = document["inputs"][0].clone();
        document["triggers"] = Value::Array(vec![trigger; triggers]);
        document["inputs"] = Value::Array(vec![input; inputs]);
        document
    };
    let mut without_name = bounded(1, 1);
    without_name["name"] = Value::Null;
    let mut without_schedule = bounded(1, 1);
    without_schedule
        .as_object_mut()
        .expect("the document is an object")
        .remove("schedule");
    let mut enabled_not_a_boolean = bounded(1, 1);
    enabled_not_a_boolean["enabled"] = json!("false");
    for (case, document) in [
        ("more triggers than the cluster runs", bounded(11, 1)),
        ("more inputs than the cluster runs", bounded(1, 11)),
        ("a monitor carrying no name", without_name),
        ("a monitor carrying no schedule", without_schedule),
        (
            "a monitor whose enabled flag is not a boolean",
            enabled_not_a_boolean,
        ),
    ] {
        let refused = rest_prefixed(
            &emulator,
            domain,
            "POST",
            "/_plugins/_alerting/monitors",
            Some(document),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from the create, got {} with {:?}",
            refused.status, refused.body
        );
    }

    // The delete releases the record, and the read after it finds nothing.
    let deleted = rest_prefixed(
        &emulator,
        domain,
        "DELETE",
        &format!("/_plugins/_alerting/monitors/{monitor_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case monitor delete: expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    let gone = rest_prefixed(
        &emulator,
        domain,
        "GET",
        &format!("/_plugins/_alerting/monitors/{monitor_id}"),
        None,
    )
    .await;
    assert_eq!(
        gone.status, 404,
        "case monitor read after delete: expected 404, got {} with {:?}",
        gone.status, gone.body
    );
}

/// RULE: the same condition decides each run from the documents the index holds AT THAT RUN,
/// so emptying the index by query turns a trigger that fired into one that withholds its
/// notification — and the alert its last firing left is completed.
///
/// The rows differ along the query the delete selects by: every document, a term matching a
/// subset, and a term matching nothing.
#[tokio::test]
async fn a_delete_by_query_decides_the_next_run_and_completes_the_alert_it_leaves() {
    let emulator = Emulator::with_notification_channels();
    let domain = "delete-by-query-domain";
    created_domain(&emulator, domain).await;
    let target = given_target(&emulator, "delete-by-query-topic").await;
    let channel_id = given_channel(&emulator, domain, "delete-by-query", &target.topic_arn).await;

    // (the case, the query the delete selects by, how many documents it removes, whether the
    // next run still fires)
    let rows = [
        (
            "a term matching nothing",
            json!({ "term": { "status": "absent" } }),
            0_u64,
            true,
        ),
        (
            "a term matching a subset",
            json!({ "term": { "status": "error" } }),
            2,
            true,
        ),
        ("every document", json!({ "match_all": {} }), 3, false),
    ];
    for (at, (case, query, removed, fires_again)) in rows.into_iter().enumerate() {
        let index = format!("{INDEX}-{at}");
        given_documents(&emulator, domain, &index, &source_documents()).await;
        let monitor_id = given_monitor(
            &emulator,
            domain,
            case,
            monitor_document(
                &format!("delete-{at}"),
                &index,
                json!({ "size": 0, "query": { "match_all": {} } }),
                "ctx.results[0].hits.total.value > 0",
                &channel_id,
                "{{ctx.results.0.hits.total.value}} left",
            ),
        )
        .await;

        // The first run fires on the documents the index holds, which is what leaves an
        // ACTIVE alert and publishes a notification.
        let first = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/_plugins/_alerting/monitors/{monitor_id}/_execute"),
            None,
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200 from the first run, got {} with {:?}",
            first.status, first.body
        );
        let published = published_message(&emulator, case, &target.queue_url).await;
        assert_eq!(
            published, "3 left",
            "case {case:?}: expected the first run to publish the count it saw, got \
             {published:?}"
        );

        let deleted = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/{index}/_delete_by_query"),
            Some(json!({ "query": query })),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from the delete, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            (
                deleted.body["deleted"].as_u64(),
                deleted.body["total"].as_u64(),
            ),
            (Some(removed), Some(removed)),
            "case {case:?}: expected the delete to report the documents it removed, got {:?}",
            deleted.body
        );
        // What the delete removed is what a search no longer finds.
        let left = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/{index}/_count"),
            Some(json!({ "query": { "match_all": {} } })),
        )
        .await;
        assert_eq!(
            left.body["count"].as_u64(),
            Some(3 - removed),
            "case {case:?}: expected the documents the delete left, got {:?}",
            left.body
        );

        let again = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/_plugins/_alerting/monitors/{monitor_id}/_execute"),
            None,
        )
        .await;
        let result = again.body["trigger_results"]
            .as_object()
            .and_then(|results| results.values().next().cloned())
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a trigger result, got {:?}", again.body)
            });
        assert_eq!(
            result["triggered"].as_bool(),
            Some(fires_again),
            "case {case:?}: expected the second run to report triggered {fires_again}, got \
             {result:?}"
        );
        let states: Vec<String> = rest_prefixed(
            &emulator,
            domain,
            "GET",
            &format!("/_plugins/_alerting/monitors/alerts?monitorId={monitor_id}"),
            None,
        )
        .await
        .body["alerts"]
            .as_array()
            .map(|alerts| {
                alerts
                    .iter()
                    .filter_map(|alert| alert["state"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            states,
            vec![if fires_again { "ACTIVE" } else { "COMPLETED" }],
            "case {case:?}: expected the alert the runs left to stand or be completed"
        );
        if fires_again {
            let second = published_message(&emulator, case, &target.queue_url).await;
            assert_eq!(
                second,
                format!("{} left", 3 - removed),
                "case {case:?}: expected the second run to publish the count it saw"
            );
        } else {
            assert!(
                received(&emulator, &target.queue_url).await.is_empty(),
                "case {case:?}: expected the run whose condition no longer holds to publish \
                 nothing"
            );
        }
    }

    // A delete naming its own `max_docs` removes that many and no more, and a `max_docs`
    // that is no count at all is refused rather than read as "every document".
    let limited = format!("{INDEX}-limited");
    given_documents(&emulator, domain, &limited, &source_documents()).await;
    let one = rest_prefixed(
        &emulator,
        domain,
        "POST",
        &format!("/{limited}/_delete_by_query"),
        Some(json!({ "query": { "match_all": {} }, "max_docs": 1 })),
    )
    .await;
    assert_eq!(
        (one.status, one.body["deleted"].as_u64()),
        (200, Some(1)),
        "case a delete limited to one document: expected one deleted, got {} with {:?}",
        one.status,
        one.body
    );
    let left = rest_prefixed(
        &emulator,
        domain,
        "POST",
        &format!("/{limited}/_count"),
        Some(json!({ "query": { "match_all": {} } })),
    )
    .await;
    assert_eq!(
        left.body["count"].as_u64(),
        Some(2),
        "case a delete limited to one document: expected the other two to be left, got {:?}",
        left.body
    );
    let refused = rest_prefixed(
        &emulator,
        domain,
        "POST",
        &format!("/{limited}/_delete_by_query"),
        Some(json!({ "query": { "match_all": {} }, "max_docs": "all of them" })),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case a delete whose max_docs is no count: expected 400, got {} with {:?}",
        refused.status, refused.body
    );
    let untouched = rest_prefixed(
        &emulator,
        domain,
        "POST",
        &format!("/{limited}/_count"),
        Some(json!({ "query": { "match_all": {} } })),
    )
    .await;
    assert_eq!(
        untouched.body["count"].as_u64(),
        Some(2),
        "case a delete whose max_docs is no count: expected it to delete nothing, got {:?}",
        untouched.body
    );
}

/// RULE: the bounds this cluster holds a monitor's own source to are enforced in host code
/// the source cannot reach — a condition that never returns is stopped and reported as the
/// trigger's error, and a template whose work this cluster will not do is reported as the
/// action's error. Neither publishes anything.
///
/// The rows differ along which bound they reach: the condition's clock, and the template's
/// section bound.
#[tokio::test]
async fn a_condition_or_template_past_the_clusters_bounds_is_reported_and_sends_nothing() {
    let emulator = Emulator::with_notification_channels();
    let domain = "bounded-domain";
    created_domain(&emulator, domain).await;
    given_documents(&emulator, domain, INDEX, &source_documents()).await;
    let target = given_target(&emulator, "bounded-topic").await;
    let channel_id = given_channel(&emulator, domain, "bounded", &target.topic_arn).await;

    // (the case, the condition, the message template, whether the trigger fires)
    let rows = [
        (
            "a condition that never returns",
            "while (true) { }",
            "never sent",
            false,
        ),
        (
            "a template nesting a block inside a block",
            "ctx.results[0].hits.total.value > 0",
            "{{#each ctx.results}}{{#each ctx.results}}x{{/each}}{{/each}}",
            true,
        ),
        (
            // The same nesting written in the renderer's whitespace-control spelling, which
            // is a block too: the bound is read off what the renderer PARSED, so no spelling
            // of a block gets past it.
            "a template nesting blocks in the whitespace-control spelling",
            "ctx.results[0].hits.total.value > 0",
            "{{~#each ctx.results~}}{{~#each ctx.results~}}x{{~/each~}}{{~/each~}}",
            true,
        ),
        (
            // Two inline partials that reference each other would expand without end as the
            // template renders, and an expansion that writes nothing would reach no output
            // ceiling: the cluster renders no partial at all, so the template is refused
            // before anything is rendered.
            "a template whose inline partials reference each other",
            "ctx.results[0].hits.total.value > 0",
            "{{#*inline \"a\"}}{{> b}}{{/inline}}{{#*inline \"b\"}}{{> a}}{{/inline}}{{> a}}",
            true,
        ),
    ];

    for (at, (case, condition, message, fires)) in rows.into_iter().enumerate() {
        let monitor_id = given_monitor(
            &emulator,
            domain,
            case,
            monitor_document(
                &format!("bounded-{at}"),
                INDEX,
                json!({ "size": 0, "query": { "match_all": {} } }),
                condition,
                &channel_id,
                message,
            ),
        )
        .await;
        let run = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/_plugins/_alerting/monitors/{monitor_id}/_execute"),
            None,
        )
        .await;
        assert_eq!(
            run.status, 200,
            "case {case:?}: expected 200 from the run, got {} with {:?}",
            run.status, run.body
        );
        let result = run.body["trigger_results"]
            .as_object()
            .and_then(|results| results.values().next().cloned())
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a trigger result, got {:?}", run.body)
            });
        assert_eq!(
            result["triggered"].as_bool(),
            Some(fires),
            "case {case:?}: expected triggered {fires}, got {result:?}"
        );
        // Whichever bound was reached, it is REPORTED — as the trigger's error where the
        // condition was stopped, and as the action's where the template was.
        let reported = if fires {
            result["action_results"]
                .as_object()
                .and_then(|actions| actions.values().next())
                .map(|action| action["error"].clone())
                .unwrap_or(Value::Null)
        } else {
            result["error"].clone()
        };
        assert!(
            reported.as_str().is_some_and(|error| !error.is_empty()),
            "case {case:?}: expected the bound that stopped it to be reported, got {result:?}"
        );
        assert!(
            received(&emulator, &target.queue_url).await.is_empty(),
            "case {case:?}: expected nothing to be published"
        );
    }
}

/// RULE: a dry run decides the trigger and sends nothing — it reports what each action would
/// render, publishes no notification and leaves no alert.
#[tokio::test]
async fn a_dry_run_reports_what_the_action_would_send_and_sends_nothing() {
    let emulator = Emulator::with_notification_channels();
    let domain = "dry-run-domain";
    created_domain(&emulator, domain).await;
    given_documents(&emulator, domain, INDEX, &source_documents()).await;
    let target = given_target(&emulator, "dry-run-topic").await;
    let channel_id = given_channel(&emulator, domain, "dry-run", &target.topic_arn).await;
    let monitor_id = given_monitor(
        &emulator,
        domain,
        "dry run",
        monitor_document(
            "dry-run",
            INDEX,
            json!({ "size": 0, "query": { "match_all": {} } }),
            "ctx.results[0].hits.total.value > 0",
            &channel_id,
            "{{ctx.results.0.hits.total.value}} documents",
        ),
    )
    .await;

    let run = rest_prefixed(
        &emulator,
        domain,
        "POST",
        &format!("/_plugins/_alerting/monitors/{monitor_id}/_execute?dryrun=true"),
        None,
    )
    .await;
    assert_eq!(
        run.status, 200,
        "case dry run: expected 200, got {} with {:?}",
        run.status, run.body
    );
    let result = run.body["trigger_results"]
        .as_object()
        .and_then(|results| results.values().next().cloned())
        .unwrap_or_else(|| panic!("case dry run: expected a trigger result, got {:?}", run.body));
    assert_eq!(
        result["triggered"].as_bool(),
        Some(true),
        "case dry run: expected the condition to hold, got {result:?}"
    );
    assert_eq!(
        result["action_results"]
            .as_object()
            .and_then(|actions| actions.values().next())
            .map(|action| action["output"]["message"].clone()),
        Some(json!("3 documents")),
        "case dry run: expected the message the action would send, got {result:?}"
    );
    assert!(
        received(&emulator, &target.queue_url).await.is_empty(),
        "case dry run: expected a dry run to publish nothing"
    );
    let alerts = rest_prefixed(
        &emulator,
        domain,
        "GET",
        &format!("/_plugins/_alerting/monitors/alerts?monitorId={monitor_id}"),
        None,
    )
    .await;
    assert_eq!(
        alerts.body["totalAlerts"].as_u64(),
        Some(0),
        "case dry run: expected a dry run to leave no alert, got {:?}",
        alerts.body
    );

    // The run's own context is what a template reads: the period the run covered, as the two
    // instants it covers, and the trigger it came from.
    let context_monitor = given_monitor(
        &emulator,
        domain,
        "run context",
        monitor_document(
            "dry-run-context",
            INDEX,
            json!({ "size": 0, "query": { "match_all": {} } }),
            "ctx.results[0].hits.total.value > 0",
            &channel_id,
            "{{ctx.periodStart}}|{{ctx.periodEnd}}|{{ctx.trigger.severity}}|\
             {{ctx.monitor.name}}",
        ),
    )
    .await;
    let run = rest_prefixed(
        &emulator,
        domain,
        "POST",
        &format!("/_plugins/_alerting/monitors/{context_monitor}/_execute?dryrun=true"),
        None,
    )
    .await;
    let rendered = run.body["trigger_results"]
        .as_object()
        .and_then(|results| results.values().next())
        .and_then(|result| result["action_results"].as_object())
        .and_then(|actions| actions.values().next())
        .and_then(|action| action["output"]["message"].as_str())
        .unwrap_or_default()
        .to_string();
    let parts: Vec<&str> = rendered.split('|').collect();
    assert_eq!(
        parts.len(),
        4,
        "case run context: expected every member of the context to render, got {rendered:?}"
    );
    for (at, instant) in parts.iter().take(2).enumerate() {
        assert!(
            instant.contains('T') && instant.ends_with('Z'),
            "case run context: expected part {at} to be the instant the period {} , got \
             {instant:?}",
            if at == 0 { "started" } else { "ended" }
        );
    }
    assert_eq!(
        (parts[2], parts[3]),
        ("1", "dry-run-context"),
        "case run context: expected the trigger's severity and the monitor's name, got \
         {rendered:?}"
    );
    assert!(
        parts[0] <= parts[1],
        "case run context: expected the period to start before it ended, got {rendered:?}"
    );
}
