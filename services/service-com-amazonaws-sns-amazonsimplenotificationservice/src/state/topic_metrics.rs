// SPDX-License-Identifier: CC0-1.0

//! The `AWS/SNS` series a topic publishes about its OWN fan-out: the publishes it accepted,
//! and what each notification those publishes owed came to.
//!
//! # What the series are
//!
//! A publish is not only the `MessageId` the caller is handed: Amazon SNS reports what the
//! topic did with it as the metrics [`NUMBER_OF_MESSAGES_PUBLISHED`],
//! [`NUMBER_OF_NOTIFICATIONS_DELIVERED`], [`NUMBER_OF_NOTIFICATIONS_FAILED`] and
//! [`NUMBER_OF_NOTIFICATIONS_FILTERED_OUT`], each dimensioned by the topic it is about
//! ([`TOPIC_NAME_DIMENSION`]). They are what an SNS dashboard plots and what a
//! delivery-health alarm is evaluated against, and they are INDEPENDENT of one another: a
//! message a subscription's filter policy withheld is counted as filtered out and nowhere
//! else, a notification an endpoint refused is counted as failed and not as delivered, and
//! neither absorbs the other — which is the whole reason a client can tell "the filter
//! withheld it" from "the endpoint refused it" rather than seeing one topic with no traffic.
//!
//! # What one observation counts
//!
//! Each of them counts a real event of the publish path, so a `Sum` over a period is what
//! that period carried and a topic nothing published to has no datapoint in it rather than a
//! zero:
//!
//! * one per publish the topic ACCEPTED — the call that was answered with a message id,
//!   whatever its fan-out then came to;
//! * one per subscription the message was OFFERED to whose filter policy did not admit it
//!   ([`crate::state::filter_policy`]), counted for a replayed message as for a published
//!   one, because a replay offers the archived message to the subscription the same way;
//! * one per notification the subscribed endpoint TOOK, and one per notification it did not
//!   — a delivery nothing granted, an endpoint that is not there, or one that refused the
//!   message itself. The outcome is the one the fan-out resolved
//!   ([`crate::ops::publish`]), so what the counters say and what the subscriber holds
//!   cannot disagree. A protocol this emulator has no delivery path for resolves no
//!   outcome, so it is counted in neither: nothing may claim a delivery, or a failure, that
//!   nothing attempted.
//!
//! The contributions of ONE request are accumulated as they happen ([`TopicCounters`]) and
//! published together once that request's fan-out is over, with no store lock held: the
//! counting happens where the decision is made, and the write happens where there is
//! somewhere to write to.
//!
//! # Where they go
//!
//! Into the CloudWatch emulator's OWN metric store — the very series `ListMetrics`,
//! `GetMetricStatistics`, `GetMetricData` and the alarm evaluation read — so one metric has
//! one home and a publisher cannot drift from its readers. They never travel over
//! `PutMetricData`, which reserves the namespaces the services own ("you should not specify
//! a namespace that begins with `AWS/`", `PutMetricData$Namespace`).
//!
//! Nothing here fails a request: the datapoints are published off the answer, so an engine
//! carrying no CloudWatch store, or one at capacity, drops the observation and the request
//! is answered exactly as it was.

use std::time::SystemTime;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// The namespace Amazon SNS publishes a topic's own metrics under.
pub const NAMESPACE: &str = "AWS/SNS";

/// The dimension that names the topic a series belongs to.
pub const TOPIC_NAME_DIMENSION: &str = "TopicName";

/// How many messages were published to the topic.
pub const NUMBER_OF_MESSAGES_PUBLISHED: &str = "NumberOfMessagesPublished";

/// How many notifications of those messages the subscribed endpoints took.
pub const NUMBER_OF_NOTIFICATIONS_DELIVERED: &str = "NumberOfNotificationsDelivered";

/// How many notifications the subscribed endpoints did NOT take.
pub const NUMBER_OF_NOTIFICATIONS_FAILED: &str = "NumberOfNotificationsFailed";

/// How many messages a subscription's filter policy withheld from it.
pub const NUMBER_OF_NOTIFICATIONS_FILTERED_OUT: &str = "NumberOfNotificationsFilteredOut";

/// The unit a count of things is published in: `Count`, the `StandardUnit` value for a
/// number of things.
pub const COUNT_UNIT: &str = "Count";

/// The `StorageResolution` these datapoints are published at, in seconds: the regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// Which counter one event of the publish path is counted into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicActivity {
    /// A publish the topic accepted: [`NUMBER_OF_MESSAGES_PUBLISHED`].
    MessagePublished,
    /// A notification the subscribed endpoint took: [`NUMBER_OF_NOTIFICATIONS_DELIVERED`].
    NotificationDelivered,
    /// A notification the subscribed endpoint did not take:
    /// [`NUMBER_OF_NOTIFICATIONS_FAILED`].
    NotificationFailed,
    /// A message a subscription's filter policy withheld:
    /// [`NUMBER_OF_NOTIFICATIONS_FILTERED_OUT`].
    NotificationFilteredOut,
}

impl TopicActivity {
    /// The metric this activity is counted into.
    #[must_use]
    pub fn metric_name(self) -> &'static str {
        match self {
            Self::MessagePublished => NUMBER_OF_MESSAGES_PUBLISHED,
            Self::NotificationDelivered => NUMBER_OF_NOTIFICATIONS_DELIVERED,
            Self::NotificationFailed => NUMBER_OF_NOTIFICATIONS_FAILED,
            Self::NotificationFilteredOut => NUMBER_OF_NOTIFICATIONS_FILTERED_OUT,
        }
    }
}

/// What ONE counter of ONE topic has been contributed so far.
#[derive(Debug, Clone)]
struct Counted {
    /// The topic the counter belongs to, as [`TOPIC_NAME_DIMENSION`] names it.
    topic_name: String,
    /// Which counter it is.
    activity: TopicActivity,
    /// How many events of that kind have been counted into it.
    count: f64,
}

/// What ONE request has counted into the topics' own counters, folded per (topic, counter)
/// as it is counted.
///
/// Folded rather than kept event by event, so the bookkeeping of a fan-out is bounded by the
/// counters there ARE rather than by how large the fan-out was, and so one request
/// contributes one datapoint per series: its `Sum` is the events the request carried, its
/// `SampleCount` is the same number, and each event counted 1.
#[derive(Debug, Clone, Default)]
pub struct TopicCounters {
    counted: Vec<Counted>,
}

impl TopicCounters {
    /// Count `times` events of `activity` on the topic `topic_arn` names.
    ///
    /// A value that is not a topic ARN names no topic to report, and no event counts
    /// nothing: either way nothing is counted.
    pub fn count(&mut self, topic_arn: &str, activity: TopicActivity, times: usize) {
        if times == 0 {
            return;
        }
        let Some(topic_name) = crate::state::topic_name_from_arn(topic_arn) else {
            return;
        };
        let times = count_as_f64(times);
        if let Some(held) = self
            .counted
            .iter_mut()
            .find(|held| held.activity == activity && held.topic_name == topic_name)
        {
            held.count += times;
            return;
        }
        self.counted.push(Counted {
            topic_name: topic_name.to_string(),
            activity,
            count: times,
        });
    }

    /// Take on everything `other` counted.
    ///
    /// A request that publishes several messages, or replays several, accumulates the whole
    /// request's contributions into one set rather than publishing them a message at a time.
    pub fn absorb(&mut self, other: &Self) {
        for counted in &other.counted {
            if let Some(held) = self
                .counted
                .iter_mut()
                .find(|held| held.activity == counted.activity && held.topic_name == counted.topic_name)
            {
                held.count += counted.count;
                continue;
            }
            self.counted.push(counted.clone());
        }
    }

    /// Append what has been counted to the CloudWatch metric store of the SAME engine, and
    /// make the alarms watching those series due for evaluation.
    ///
    /// Call it with NO SNS store lock held: it takes CloudWatch's own. An engine carrying no
    /// CloudWatch store has nowhere to publish to, so nothing is published and nothing fails.
    pub fn publish<Eng: Send + Sync + 'static>(&self, engine: &Eng) {
        if self.counted.is_empty() {
            return;
        }
        let Some(state) = cloudwatch::state::service_state(engine) else {
            return;
        };
        let at_secs = epoch_secs(SystemTime::now());
        let mut store = state.lock();
        let mut written: Vec<cloudwatch::state::WrittenSeries> = Vec::new();
        for counted in &self.counted {
            // The series identity is the namespace, the metric and the topic's NAME: those
            // are the dimensions a reader selects a topic's series by.
            let key = cloudwatch::state::MetricKey {
                namespace: NAMESPACE.to_string(),
                metric_name: counted.activity.metric_name().to_string(),
                dimensions: [(
                    TOPIC_NAME_DIMENSION.to_string(),
                    counted.topic_name.clone(),
                )]
                .into_iter()
                .collect(),
            };
            let datum = cloudwatch::state::StoredDatum {
                timestamp_secs: at_secs,
                // Every event counted one, so the request's contribution is that many
                // observations of 1: a `Sum` over a window is the events it carried.
                sample_count: counted.count,
                sum: counted.count,
                minimum: 1.0,
                maximum: 1.0,
                unit: Some(COUNT_UNIT.to_string()),
                storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
            };
            if store.append_datapoints(key.clone(), vec![datum]).is_err() {
                // The metric store holds no more series. The request this observation came
                // off is answered either way; the dropped count is reported where a
                // developer can see it.
                ::tracing::warn!(
                    topic = %counted.topic_name,
                    metric = %counted.activity.metric_name(),
                    "the CloudWatch metric store is at capacity, so this topic's count was \
                     not published"
                );
                continue;
            }
            written.push(cloudwatch::state::WrittenSeries {
                key,
                units: vec![COUNT_UNIT.to_string()],
            });
        }
        // An SNS delivery-health alarm is an alarm on one of these series, so the arriving
        // datapoints are what makes its next evaluation due — exactly as a published
        // datapoint does.
        store.make_alarms_due(&written, at_secs);
    }
}

/// A count of events as the value its datapoint carries. The counts are bounded by the
/// fan-out budgets of one request, so the conversion is exact.
fn count_as_f64(count: usize) -> f64 {
    f64::from(u32::try_from(count).unwrap_or(u32::MAX))
}

/// `at` as the epoch seconds a stored datapoint carries, which is the unit CloudWatch's own
/// readers window by.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}
