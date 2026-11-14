// SPDX-License-Identifier: CC0-1.0

//! The EVENT a control-plane write raises, and the notification each subscription
//! scoped to it receives.
//!
//! An event notification subscription exists to be told: `CreateEventSubscription`
//! "creates an Amazon Redshift event notification subscription […] Notifications
//! will be sent for all events you want that match those criteria", so storing a
//! subscription and never contacting its topic leaves the subscription pointless.
//! This module is the other half: the write that happened raises the event the
//! service's published catalog ([`super::EVENT_CATALOG`]) lists for it, and every
//! subscription whose scope admits that event is published to.
//!
//! What a client can observe is one `sns:Publish` onto the topic the subscription's
//! `SnsTopicArn` names, made the way every emulated service calls another: over the
//! IN-PROCESS callback into this same emulator
//! ([`smithy_server_runtime::OperationContext::callback`]). Nothing here opens a
//! socket or leaves the host — the ARN a caller supplied is not a URL this code
//! dials, it is a topic the request is routed to INSIDE this process, so the topic
//! has to be one this emulator itself holds or Amazon SNS answers that it does not
//! exist.
//!
//! The work splits in two so that no publish is made while the store is locked:
//! [`owed`] decides which topics a raised event is owed to (under the lock, from
//! the subscriptions the store holds) and [`publish`] performs the publishes once
//! the lock is released. [`raise`] is the whole path, and the one call a handler
//! makes after its own write is complete and its guard dropped — and, because it
//! is the one place every raised event passes through, it is also where the event
//! is RECORDED in the store's event log, which is what an event read reports.
//!
//! A publish the topic's own service refuses — most often a topic that does not
//! exist, or one no SNS is mounted to hold — is not the control-plane operation's
//! failure: Amazon Redshift does not refuse a write because a notification could
//! not be delivered, so the refusal is logged and the remaining notifications are
//! still published.

use std::time::SystemTime;

use crate::ops::types::{Event, EventSubscription};

use super::{CatalogEvent, EVENT_CATALOG, SNS_ARN_SERVICE};

/// The placeholder the published catalog writes the event's own instant as.
///
/// Every entry of [`EVENT_CATALOG`] renders each value an occurrence fills in as
/// a `<placeholder>`, the form the published catalog's description text carries;
/// this is the one filled from the CLOCK rather than from the resource the event
/// is about, and the catalog names it after the instant it stands for — the time
/// in UTC — so the rendered message reads `<date> <minute> UTC` where the
/// template reads `<time in UTC>`.
const TIME_PLACEHOLDER: &str = "time in UTC";

/// The catalog placeholder a cluster parameter group's name fills.
pub const PARAMETER_GROUP_NAME: &str = "parameter group name";
/// The catalog placeholder a cluster's identifier fills.
pub const CLUSTER_NAME: &str = "cluster name";
/// The catalog placeholder the identifier a cluster was renamed TO fills.
pub const NEW_CLUSTER_NAME: &str = "new cluster name";
/// The catalog placeholder a cluster snapshot's identifier fills.
pub const SNAPSHOT_NAME: &str = "snapshot name";

/// One event an occurrence of a catalog entry raises: the entry, the source it
/// happened to, the instant it happened at, and the message it carries.
#[derive(Debug, Clone)]
pub struct RaisedEvent {
    /// The published catalog entry: the event id, source type, categories and
    /// severity a subscription is matched against.
    pub event: &'static CatalogEvent,
    /// The identifier of the source object the event happened to — a cluster
    /// identifier, a parameter group name, a snapshot identifier — which is what
    /// a subscription's `SourceIds` scopes it to.
    pub source_id: String,
    /// When it happened.
    pub at: SystemTime,
    /// The event message, as [`message`] renders the catalog's description for
    /// this occurrence.
    pub message: String,
}

/// The event `event_id` happening to `source_id` now, or `None` when this service
/// publishes no such event or the catalog description names a value `fills` does
/// not carry.
///
/// `fills` names the catalog placeholders this occurrence fills, without their
/// brackets: `[("parameter group name", "my-group")]` fills
/// `<parameter group name>`. The clock fills [`TIME_PLACEHOLDER`], so no caller
/// passes an instant.
pub fn raised(event_id: &str, source_id: &str, fills: &[(&str, &str)]) -> Option<RaisedEvent> {
    let event = EVENT_CATALOG.iter().find(|entry| entry.event_id == event_id)?;
    let at = SystemTime::now();
    Some(RaisedEvent {
        event,
        source_id: source_id.to_string(),
        at,
        message: message(event, at, fills)?,
    })
}

impl RaisedEvent {
    /// This occurrence as the modelled `Event` an event read reports: the
    /// catalog entry's own identity (its id, the source type that raises it, the
    /// categories and the severity it is published under) and the three values
    /// this occurrence supplies — the source it happened to, the instant it
    /// happened at, and the message it rendered.
    pub fn to_api(&self) -> Event {
        Event {
            source_identifier: Some(self.source_id.clone()),
            source_type: Some(self.event.source_type.to_string()),
            message: Some(self.message.clone()),
            event_categories: Some(
                self.event
                    .event_categories
                    .iter()
                    .map(|category| (*category).to_string())
                    .collect(),
            ),
            severity: Some(self.event.severity.to_string()),
            date: Some(::aws_smithy_types::DateTime::from(self.at)),
            event_id: Some(self.event.event_id.to_string()),
        }
    }
}

/// The message one occurrence of `event` carries: the catalog's description with
/// every `<placeholder>` filled.
///
/// The description is the published text and the fixed half of the message; what
/// an occurrence supplies is the values. A placeholder naming the event's own
/// instant is filled from the clock, in the `YYYY-MM-DD HH:MM UTC` form the
/// catalog text names it by; any other placeholder is filled from `fills` and
/// rendered in single quotes, the form the published message takes for the name of
/// the resource an event happened to.
///
/// `None` when a placeholder has no fill: a message naming `<a placeholder>` where
/// a value belongs reports nothing, so no event is raised at all rather than one
/// carrying an unfilled template.
fn message(event: &CatalogEvent, at: SystemTime, fills: &[(&str, &str)]) -> Option<String> {
    let description = event.event_description;
    let mut rendered = String::with_capacity(description.len());
    let mut rest = description;
    while let Some(open) = rest.find('<') {
        rendered.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('>')?;
        let placeholder = &after[..close];
        if placeholder == TIME_PLACEHOLDER {
            rendered.push_str(&minute_text(at)?);
        } else {
            let (_, value) = fills.iter().find(|(named, _)| *named == placeholder)?;
            rendered.push('\'');
            rendered.push_str(value);
            rendered.push('\'');
        }
        rest = &after[close + 1..];
    }
    rendered.push_str(rest);
    Some(rendered)
}

/// `at` to the minute, as `YYYY-MM-DD HH:MM UTC`.
///
/// The calendar conversion is the LIBRARY's: `DateTime::fmt(Format::DateTime)`
/// renders an instant as RFC 3339 in UTC (`YYYY-MM-DDThh:mm:ss[.fff]Z`), so the
/// date and the hour and minute are read off that rendering rather than computed
/// here. The zone is spelled out because the placeholder the catalog fills with
/// it is the time IN UTC.
fn minute_text(at: SystemTime) -> Option<String> {
    let rendered = ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    let (date, clock) = rendered.split_once('T')?;
    let minutes = clock.get(..5)?;
    Some(format!("{date} {minutes} UTC"))
}

/// `at` as the notification's own `Event Time`: the RFC 3339 instant in UTC, the
/// one form of a timestamp this emulator writes into a document it renders itself.
fn event_time_text(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// Whether `subscription` is told about `event`.
///
/// The four scoping members a subscription carries, each read as its own
/// documentation defines it:
///
/// * it has to be ACTIVE — `Enabled` is "set to true to activate the subscription,
///   and set to false to create the subscription but not activate it", and
///   `Status` is `active` only while the topic still accepts Amazon Redshift's
///   publishes;
/// * `SourceType` is "the type of source that will be generating the events […]
///   If this value is not specified, events are returned for all Amazon Redshift
///   objects in your Amazon Web Services account";
/// * `SourceIds` names "one or more identifiers of Amazon Redshift source objects
///   […] If not specified, then events are returned for all objects within the
///   source type specified";
/// * `EventCategories` and `Severity` are what the subscription publishes, so an
///   event outside either is not its event — and a subscription naming none takes
///   every category and both severities.
fn admits(subscription: &EventSubscription, event: &CatalogEvent, source_id: &str) -> bool {
    if !subscription
        .enabled
        .unwrap_or(super::EVENT_SUBSCRIPTION_ENABLED_BY_DEFAULT)
    {
        return false;
    }
    if subscription.status.as_deref() != Some(super::SUBSCRIPTION_STATUS_ACTIVE) {
        return false;
    }
    if subscription
        .source_type
        .as_deref()
        .is_some_and(|scoped| scoped != event.source_type)
    {
        return false;
    }
    let scoped_ids = subscription.source_ids_list.as_deref().unwrap_or_default();
    if !scoped_ids.is_empty() && !scoped_ids.iter().any(|scoped| scoped == source_id) {
        return false;
    }
    let scoped_categories = subscription
        .event_categories_list
        .as_deref()
        .unwrap_or_default();
    if !scoped_categories.is_empty()
        && !scoped_categories
            .iter()
            .any(|scoped| event.event_categories.contains(&scoped.as_str()))
    {
        return false;
    }
    if subscription
        .severity
        .as_deref()
        .is_some_and(|scoped| scoped != event.severity)
    {
        return false;
    }
    true
}

/// One notification a raised event owes: the topic it is published to and the
/// document published onto it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventNotification {
    /// The topic a subscription's `SnsTopicArn` named.
    pub topic_arn: String,
    /// The notification, as [`notification_document`] renders it.
    pub message: String,
}

/// The notifications `raised` owes, given the subscriptions `store` holds.
///
/// One per ADMITTING subscription, so a topic two subscriptions of one account
/// name is published to once for each: each subscription is told about the event
/// it subscribed to. A subscription whose `SnsTopicArn` is not the ARN of an SNS
/// topic in a Region names nothing this emulator can publish to and is skipped
/// here rather than attempted.
///
/// The fan-out one write owes is therefore bounded by the number of
/// subscriptions the store admits at all ([`super::EVENT_SUBSCRIPTION_QUOTA`]),
/// each of which is a record a caller created and `DeleteEventSubscription`
/// releases: no single request can grow the work a raised event does.
pub fn owed(
    store: &super::ComAmazonawsRedshiftRedshiftserviceversion20121201State,
    raised: &RaisedEvent,
) -> Vec<EventNotification> {
    let Some(message) = notification_document(raised) else {
        return Vec::new();
    };
    store
        .event_subscriptions
        .iter()
        .filter(|subscription| admits(subscription, raised.event, &raised.source_id))
        .filter_map(|subscription| subscription.sns_topic_arn.as_deref())
        .filter(|topic_arn| topic_region(topic_arn).is_some())
        .map(|topic_arn| EventNotification {
            topic_arn: topic_arn.to_string(),
            message: message.clone(),
        })
        .collect()
}

/// The JSON document a subscriber receives for `raised`.
///
/// Amazon Redshift publishes an event notification as a JSON document naming the
/// source the event happened to, the event itself and the message it carries; the
/// model declares no shape for it, so every member here is read off the raised
/// event and the published catalog rather than copied from any one notification.
/// The source object is named twice because the notification names it twice, as
/// the event's `Source ID` and as the `Resource` it happened to, and both are
/// the one identifier the write addressed.
fn notification_document(raised: &RaisedEvent) -> Option<String> {
    Some(
        ::serde_json::json!({
            "Event Source": raised.event.source_type,
            "Event Time": event_time_text(raised.at)?,
            "Source ID": raised.source_id,
            "Resource": raised.source_id,
            "Event ID": raised.event.event_id,
            "Severity": raised.event.severity,
            "Event Message": raised.message,
        })
        .to_string(),
    )
}

/// The Region of the Amazon SNS topic `arn` names, or `None` when it names no
/// topic.
///
/// Read through the one rule this service recognises a topic ARN by
/// ([`super::is_sns_topic_arn`]), and then held to the GRAMMAR of a Region code —
/// the lowercase letters, digits and hyphens an AWS Region label is made of — so
/// the only thing a caller-supplied ARN can decide is which Region of this
/// emulator the publish is addressed in, never what that address is.
fn topic_region(arn: &str) -> Option<&str> {
    if !super::is_sns_topic_arn(arn) {
        return None;
    }
    arn.split(':').nth(3).filter(|region| {
        region
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
    })
}

/// Publish every notification in `notifications` to the topic it names.
///
/// The client is built once per Region and reused, so a set of notifications
/// spanning one Region builds one client.
pub async fn publish(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    notifications: &[EventNotification],
) {
    let mut client: Option<(String, ::aws_sdk_sns::Client)> = None;
    for notification in notifications {
        // Re-read from the ARN rather than carried alongside it, so the Region
        // this publish is addressed in is always one that came out of the ARN.
        let Some(region) = topic_region(&notification.topic_arn) else {
            continue;
        };
        if client
            .as_ref()
            .is_none_or(|(held, _)| held.as_str() != region)
        {
            client = Some((region.to_string(), sns_client(ctx, region)));
        }
        let Some((_, sns)) = client.as_ref() else {
            continue;
        };
        let published = sns
            .publish()
            .topic_arn(&notification.topic_arn)
            .message(&notification.message)
            .send()
            .await;
        if let Err(error) = published {
            ::tracing::warn!(
                topic = %notification.topic_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an event notification could not be published to a subscription's topic"
            );
        }
    }
}

/// Raise the catalog event `event_id` for the source `source_id`, record it in
/// the store's event log, and publish it to every subscription scoped to it.
///
/// The whole path, so a control-plane handler raises an event in one line, AFTER
/// its own write is complete and its store guard dropped: this takes the lock
/// itself, for the recording and the matching, and holds none of it across the
/// publish.
///
/// `fills` names the catalog placeholders this occurrence fills; see [`raised`].
/// A request that raises an event no subscription is scoped to publishes nothing
/// and builds no client.
pub async fn raise<Eng: Send + Sync + 'static>(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
    event_id: &str,
    source_id: &str,
    fills: &[(&str, &str)],
) {
    let Some(raised) = raised(event_id, source_id, fills) else {
        ::tracing::warn!(
            event_id,
            "no published catalog event of this id could be rendered; nothing is raised"
        );
        return;
    };
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let notifications = {
        let mut store = state.lock();
        // The event HAPPENED, which an event read reports for as long as the
        // service keeps it — so it is recorded whether or not any subscription
        // is scoped to it, and before the matching that decides who is told.
        store.record_event(raised.to_api());
        owed(&store, &raised)
    };
    if notifications.is_empty() {
        return;
    }
    publish(ctx, &notifications).await;
}

/// What asking Amazon SNS about the topic a subscription names found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicResolution {
    /// Amazon SNS holds the topic the ARN names.
    Held,
    /// Amazon SNS was asked and holds no such topic, so the subscription names a
    /// topic nothing could ever be published to.
    Absent,
    /// Amazon SNS could not be ASKED at all (it is not mounted in this process, or
    /// the answer could not be read), so whether the topic exists is UNKNOWN. The
    /// reason travels for the operator-visible log.
    Undecided(String),
}

/// The error codes that say Amazon SNS answered about the VALUE the lookup named:
/// it holds no such topic (`NotFound`, the fault `GetTopicAttributes` models for an
/// ARN that resolves to nothing), or the value is not a topic ARN it could hold at
/// all (`InvalidParameter`), which therefore names no topic either.
///
/// Any other code, or none at all — a server fault, a transport failure, or a reply
/// that is not that service's answer because no Amazon SNS is mounted beside this
/// one — says nothing about the topic and leaves the question undecided. The codes
/// are matched by PREFIX of the modelled exception name, so `NotFound` and
/// `NotFoundException` are the same answer.
const TOPIC_NEGATIVE_ERROR_CODES: &[&str] = &["NotFound", "InvalidParameter"];

/// Whether Amazon SNS holds the topic `topic_arn`.
///
/// `CreateEventSubscription` takes "the Amazon Resource Name (ARN) of the Amazon
/// SNS topic used to transmit the event notifications" and models
/// `SNSTopicArnNotFoundFault` for one that does not exist, so the question has to
/// be put to the service that OWNS topics — over the in-process callback, exactly
/// as [`publish`] publishes to that same topic ([`sns_client`]).
///
/// This service keeps only the topic's ARN, which the caller supplied, so it is a
/// GATE on existence rather than a reader of the topic's own properties: a hop
/// that could not be made at all therefore refuses nothing (see
/// `services/common/src/network.rs` for the contract both readings come from), and
/// only Amazon SNS's own answer grounds a refusal.
///
/// Call it with NO store guard held — it awaits.
pub async fn resolve_topic(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    topic_arn: &str,
) -> TopicResolution {
    // The Region is the label read out of the topic's OWN ARN, so no
    // caller-supplied text names the host this hop is addressed to; an ARN
    // carrying none names no topic any Region holds.
    let Some(region) = topic_region(topic_arn) else {
        return TopicResolution::Absent;
    };
    let client = sns_client(ctx, region);
    match client
        .get_topic_attributes()
        .topic_arn(topic_arn)
        .send()
        .await
    {
        Ok(_) => TopicResolution::Held,
        Err(error) => {
            let reason =
                ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string();
            // The status that service answered with, when it answered at all: a
            // 5xx is about the service, not about the value, whatever code
            // travels with it.
            let client_error = error
                .raw_response()
                .is_some_and(|response| response.status().is_client_error());
            let code = ::aws_sdk_sns::error::ProvideErrorMetadata::code(&error).unwrap_or_default();
            let about_the_value = TOPIC_NEGATIVE_ERROR_CODES
                .iter()
                .any(|known| code.starts_with(known));
            if client_error && about_the_value {
                TopicResolution::Absent
            } else {
                TopicResolution::Undecided(reason)
            }
        }
    }
}

/// Why a subscription naming a topic Amazon SNS holds nothing under is refused,
/// naming the ARN the request carried.
///
/// `smithy_query(query="shape", shape="SNSTopicArnNotFoundFault")` — "An Amazon
/// SNS topic with the specified Amazon Resource Name (ARN) does not exist" — which
/// is the fixed text; the ARN the request named is the variable part, because that
/// is what the caller has to fix.
pub fn sns_topic_not_found_message(topic_arn: &str) -> String {
    format!("An Amazon SNS topic with the specified Amazon Resource Name (ARN) {topic_arn} does not exist.")
}

/// The Amazon SNS client an event notification is published through: the IN-PROCESS callback into this same emulator.
///
/// Nothing here opens a socket or leaves the host: `ctx.callback()` is the
/// framework's seam for calling another emulated service, and the `http_client` it
/// hands over dispatches the request inside this process to whichever mounted
/// service claims it. The `endpoint_url` below is therefore an ADDRESS the
/// in-process router reads, never a host any resolver looks up, and `region` is
/// the label [`topic_region`] read out of the topic's own ARN, so a caller cannot
/// steer that authority.
///
/// The credentials are the emulator's own root credentials, which only sign this
/// operation's in-process calls; they are never put in a response or a log.
fn sns_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{SNS_ARN_SERVICE}.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}
