// SPDX-License-Identifier: CC0-1.0

//! The CONFIGURATION ITEM CHANGE notification a delivery channel streams: whenever Config
//! RECORDS a configuration item for a resource, the item and how it differs from the item
//! it replaced are published to the channel's Amazon SNS topic.
//!
//! A delivery channel "sends notifications and updated configuration states"
//! (`PutDeliveryChannelRequest$DeliveryChannel`) to the topic its `snsTopicARN` names
//! (`DeliveryChannel$snsTopicARN`: "The Amazon Resource Name (ARN) of the Amazon SNS topic
//! to which Config sends notifications about configuration changes"), so the notification
//! is owed by the WRITE that records the item and by nothing a caller reads: the recorder's
//! own pass ([`super::recording`]), and the two operations that record the configuration
//! state of one resource themselves. A channel carrying no topic streams nothing, and an
//! observation that records NO configuration item — the resource is still in the state the
//! held item records — notifies nobody.
//!
//! What a consumer reads is the `configurationItemDiff`: it carries the `changeType` of the
//! item (the resource's first item, a changed one, or the one recording its deletion) and,
//! for a changed one, the properties that changed under `changedProperties`, each with the
//! value it held and the value it now holds. That is the only place the property that
//! changed is NAMED, so it is computed from the item this one replaced rather than restated
//! from a request.
//!
//! The publish is made the way every other cross-service call of this crate is made, over
//! the in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so this
//! crate names no other service crate. It is BEST EFFORT and changes nothing an operation
//! answers: AWS streams a notification after the item was recorded, so a topic that cannot
//! be reached leaves the item recorded and is logged.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use ::serde_json::{Map, Value};

use super::{
    delivered_configuration_item, event_instant, service_state,
    ComAmazonawsConfigserviceStarlingdoveserviceState, RecordedConfigurationState,
};

/// The `messageType` every configuration-item change notification carries, which is what
/// a consumer of the channel's topic branches on (the AWS Config Developer Guide's
/// *Example configuration item change notification*).
pub const CONFIGURATION_ITEM_CHANGE_MESSAGE_TYPE: &str = "ConfigurationItemChangeNotification";

/// The `recordVersion` such a notification carries, and the `configurationItemVersion` of
/// the item inside it: the version of the NOTIFICATION format, which is the same format
/// version a delivered configuration snapshot's items carry (AWS Config Developer Guide).
pub const CONFIGURATION_ITEM_RECORD_VERSION: &str = "1.3";

/// The `changeType` of the notification for a resource's FIRST configuration item — the
/// resource was discovered — and of a property the new item carries and the old one did
/// not.
pub const CHANGE_TYPE_CREATE: &str = "CREATE";
/// The `changeType` of the notification for an item that records a state the resource
/// CHANGED into, and of a property whose value the new item changed.
pub const CHANGE_TYPE_UPDATE: &str = "UPDATE";
/// The `changeType` of the notification for the item recording that the resource is gone,
/// and of a property the old item carried and the new one does not.
pub const CHANGE_TYPE_DELETE: &str = "DELETE";

/// The property path prefix of the configuration document's own members, under which the
/// diff names every property of it that changed.
const CONFIGURATION_PROPERTY: &str = "Configuration";
/// The prefix of a `supplementaryConfiguration` block's members.
const SUPPLEMENTARY_PROPERTY: &str = "SupplementaryConfiguration";
/// The prefix of the tags recorded with the configuration item.
const TAGS_PROPERTY: &str = "Tags";

/// The most properties one notification's `changedProperties` names.
///
/// A configuration document is bounded in bytes
/// ([`crate::ops::put_resource_config::MAX_RESOURCE_CONFIGURATION_BYTES`]) but not in the
/// NUMBER of members it nests, so the diff of two such documents is bounded here: past
/// this many changed properties the notification carries the first of them, which keeps one
/// recorded item's notification bounded whatever was recorded.
const MAX_CHANGED_PROPERTIES: usize = 500;

/// One configuration item Config RECORDED, as the change notification for it is derived:
/// which resource it records, what KIND of change it is, and the state the resource left.
///
/// Produced by the ONE place the inventory is written
/// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::record_discovered_resource`]), so
/// every writer of a configuration item owes the same notification and an observation that
/// records no item produces none.
#[derive(Debug, Clone)]
pub struct RecordedItemChange {
    /// The type of the resource whose item was recorded.
    pub resource_type: String,
    /// The id of that resource.
    pub resource_id: String,
    /// The `configurationItemDiff$changeType` of the recorded item.
    pub change_type: &'static str,
    /// The state the resource LEFT — the one the item this item replaced recorded — which
    /// is what the changed properties are computed against. `None` for the resource's first
    /// item, which replaced none.
    pub previous: Option<RecordedConfigurationState>,
}

/// One notification a delivery channel owes: the topic it is streamed to and the message
/// body it carries.
#[derive(Debug, Clone)]
pub struct StreamedNotification {
    /// The channel's `snsTopicARN`.
    pub topic_arn: String,
    /// The notification body, as the topic's subscribers receive it.
    pub message: Value,
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// The notifications this account's delivery channels owe for the configuration items
    /// `changes` records.
    ///
    /// Read-only: it derives the bodies from the items the inventory now holds, so the
    /// store is untouched and the publishing happens with the lock released. A channel that
    /// names no topic has SNS delivery turned off and so owes nothing
    /// (`ConfigStreamDeliveryInfo$lastStatus`: "Providing an SNS topic on a DeliveryChannel
    /// for Config is optional").
    pub fn change_notifications(
        &self,
        changes: &[RecordedItemChange],
        region: &str,
        now: SystemTime,
    ) -> Vec<StreamedNotification> {
        let topics: Vec<&str> = self
            .delivery_channels
            .values()
            .filter_map(|channel| channel.sns_topic_arn.as_deref())
            .filter(|topic| !topic.is_empty())
            .collect();
        if topics.is_empty() {
            return Vec::new();
        }
        let mut streamed = Vec::new();
        for change in changes {
            let key = (change.resource_type.clone(), change.resource_id.clone());
            // The item the notification carries is the one the inventory holds for the
            // resource: a resource the inventory has since given up has no item to stream.
            let Some(entry) = self.discovered_resources.get(&key) else {
                continue;
            };
            let item = entry.to_configuration_item(
                region,
                self.recording_frequency_of(&entry.resource_type),
            );
            let message = configuration_item_change_notification(
                &delivered_configuration_item(&item),
                change.change_type,
                changed_properties(
                    change.change_type,
                    change.previous.as_ref(),
                    &recorded_properties(
                        entry.configuration.as_deref(),
                        &entry.supplementary_configuration,
                        &entry.tags,
                    ),
                ),
                now,
            );
            for topic in &topics {
                streamed.push(StreamedNotification {
                    topic_arn: (*topic).to_string(),
                    message: message.clone(),
                });
            }
        }
        streamed
    }
}

/// The body of one configuration-item change notification: the item, its diff from the
/// item it replaced, the instant the notification was created and the format it is written
/// in.
///
/// The members are the ones such a notification carries (AWS Config Developer Guide,
/// *Example configuration item change notification*), each spelled as the notification
/// spells it; `item` is the configuration item in the spelling a DELIVERED item carries
/// ([`delivered_configuration_item`]), whose `configuration` is the document it encodes
/// rather than a JSON-encoded string.
pub fn configuration_item_change_notification(
    item: &Value,
    change_type: &str,
    changed: Map<String, Value>,
    now: SystemTime,
) -> Value {
    let mut item = item.clone();
    if let Some(members) = item.as_object_mut() {
        // The version of the notification format the item is written in, which an item
        // recorded under a caller's own `SchemaVersionId` already carries.
        members
            .entry("configurationItemVersion")
            .or_insert_with(|| Value::String(CONFIGURATION_ITEM_RECORD_VERSION.to_string()));
    }
    ::serde_json::json!({
        "configurationItemDiff": {
            "changedProperties": changed,
            "changeType": change_type,
        },
        "configurationItem": item,
        "notificationCreationTime": event_instant(now),
        "messageType": CONFIGURATION_ITEM_CHANGE_MESSAGE_TYPE,
        "recordVersion": CONFIGURATION_ITEM_RECORD_VERSION,
    })
}

/// The properties of one recorded configuration item, as the diff names them: every
/// member of the configuration document under [`CONFIGURATION_PROPERTY`], every member of
/// each `supplementaryConfiguration` block under [`SUPPLEMENTARY_PROPERTY`] and that
/// block's name, and every recorded tag under [`TAGS_PROPERTY`].
///
/// A nested member is named by its PATH (`Configuration.VisibilityTimeout`), which is how
/// a changed property names the field that changed; a member that is not an object is a
/// value, so a list changes as a whole.
fn recorded_properties(
    configuration: Option<&str>,
    supplementary: &BTreeMap<String, String>,
    tags: &BTreeMap<String, String>,
) -> BTreeMap<String, Value> {
    let mut properties = BTreeMap::new();
    if let Some(configuration) = configuration {
        flatten(
            CONFIGURATION_PROPERTY,
            &document(configuration),
            &mut properties,
        );
    }
    for (block, value) in supplementary {
        flatten(
            &format!("{SUPPLEMENTARY_PROPERTY}.{block}"),
            &document(value),
            &mut properties,
        );
    }
    for (key, value) in tags {
        properties.insert(
            format!("{TAGS_PROPERTY}.{key}"),
            Value::String(value.clone()),
        );
    }
    properties
}

/// `text` as the JSON document it encodes, or as the string it is when it encodes none —
/// the same reading [`super::configuration_item_document`] gives a configuration document,
/// so a document that is not JSON is diffed as the one value it is.
fn document(text: &str) -> Value {
    ::serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// Name every value `value` holds by its path under `path`, walking into objects only.
fn flatten(path: &str, value: &Value, into: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(members) if !members.is_empty() => {
            for (member, value) in members {
                flatten(&format!("{path}.{member}"), value, into);
            }
        }
        value => {
            into.insert(path.to_string(), value.clone());
        }
    }
}

/// The `changedProperties` of one recorded item: one entry per property whose value the
/// item CHANGED, naming the value it held, the value it now holds and which of the two it
/// has.
///
/// Only an item that records a state the resource CHANGED into has changed properties: the
/// resource's first item changed nothing (there was no item to differ from), and the item
/// recording a deletion reports the resource is gone rather than which of its properties
/// went with it.
fn changed_properties(
    change_type: &str,
    previous: Option<&RecordedConfigurationState>,
    current: &BTreeMap<String, Value>,
) -> Map<String, Value> {
    let mut changed = Map::new();
    if change_type != CHANGE_TYPE_UPDATE {
        return changed;
    }
    let Some(previous) = previous else {
        return changed;
    };
    let previous = recorded_properties(
        previous.configuration.as_deref(),
        &previous.supplementary_configuration,
        &previous.tags,
    );
    let named: BTreeSet<&String> = previous.keys().chain(current.keys()).collect();
    for property in named {
        let held = previous.get(property);
        let now_held = current.get(property);
        if held == now_held {
            continue;
        }
        if changed.len() >= MAX_CHANGED_PROPERTIES {
            ::tracing::warn!(
                properties = changed.len(),
                "a recorded configuration item changed more properties than one \
                 notification carries"
            );
            break;
        }
        let property_change = match (held, now_held) {
            (None, Some(_)) => CHANGE_TYPE_CREATE,
            (Some(_), None) => CHANGE_TYPE_DELETE,
            _ => CHANGE_TYPE_UPDATE,
        };
        changed.insert(
            property.to_string(),
            ::serde_json::json!({
                "previousValue": held.cloned().unwrap_or(Value::Null),
                "updatedValue": now_held.cloned().unwrap_or(Value::Null),
                "changeType": property_change,
            }),
        );
    }
    changed
}

/// Stream the notifications the configuration items `changes` records owe to the delivery
/// channel's topic.
///
/// Takes the callback rather than an `OperationContext`, so the recorder's pump — which
/// outlives the request that started it — publishes through exactly the same path a handler
/// does. The store's guard is taken to DERIVE the bodies and released before the first
/// publish, because a publish is an in-process call into another service.
pub async fn stream_recorded_item_changes<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
    changes: &[RecordedItemChange],
    now: SystemTime,
) {
    if changes.is_empty() {
        return;
    }
    let Some(state) = service_state(engine) else {
        return;
    };
    let notifications = state.lock().change_notifications(changes, region, now);
    if notifications.is_empty() {
        return;
    }
    let client = sns_client(callback, region);
    for notification in notifications {
        let published = client
            .publish()
            .topic_arn(&notification.topic_arn)
            .message(notification.message.to_string())
            .send()
            .await;
        if let Err(error) = published {
            ::tracing::warn!(
                topic_arn = %notification.topic_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a configuration item change notification did not reach the delivery \
                 channel's topic"
            );
        }
    }
}

/// An Amazon SNS client over the in-process callback, built the way this crate's other
/// cross-service clients are: the emulator supplies the transport, so there is nothing to
/// retry, time out, cache an identity for or watch for a stalled stream.
fn sns_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorded state carrying `configuration` as its configuration document and `tags`
    /// as the tags recorded with it.
    fn recorded(configuration: &str, tags: &[(&str, &str)]) -> RecordedConfigurationState {
        RecordedConfigurationState {
            configuration: Some(configuration.to_string()),
            supplementary_configuration: BTreeMap::new(),
            tags: tags
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            relationships: Vec::new(),
            recorded_at: Some(SystemTime::UNIX_EPOCH),
        }
    }

    /// The changed properties between two recorded states.
    fn diff(
        previous: &RecordedConfigurationState,
        configuration: &str,
        tags: &[(&str, &str)],
    ) -> Map<String, Value> {
        let current = recorded(configuration, tags);
        changed_properties(
            CHANGE_TYPE_UPDATE,
            Some(previous),
            &recorded_properties(
                current.configuration.as_deref(),
                &current.supplementary_configuration,
                &current.tags,
            ),
        )
    }

    /// A changed property is NAMED by its path and carries the value it held and the value
    /// it now holds. The rows differ along what changed: a top-level member, a nested one,
    /// one that appeared, one that went away, a tag, and a document in which nothing
    /// changed.
    #[test]
    fn the_diff_names_every_property_the_new_item_changed() {
        let before = recorded(
            r#"{"VisibilityTimeout":"31","DelaySeconds":"0","Redrive":{"maxReceiveCount":4}}"#,
            &[("stage", "prod")],
        );
        type Case = (
            &'static str,
            &'static str,
            &'static [(&'static str, &'static str)],
            &'static [(&'static str, &'static str, &'static str, &'static str)],
        );
        let cases: [Case; 6] = [
            (
                "one top-level member changed",
                r#"{"VisibilityTimeout":"41","DelaySeconds":"0","Redrive":{"maxReceiveCount":4}}"#,
                &[("stage", "prod")],
                &[(
                    "Configuration.VisibilityTimeout",
                    "\"31\"",
                    "\"41\"",
                    CHANGE_TYPE_UPDATE,
                )],
            ),
            (
                "a nested member changed",
                r#"{"VisibilityTimeout":"31","DelaySeconds":"0","Redrive":{"maxReceiveCount":7}}"#,
                &[("stage", "prod")],
                &[(
                    "Configuration.Redrive.maxReceiveCount",
                    "4",
                    "7",
                    CHANGE_TYPE_UPDATE,
                )],
            ),
            (
                "a member the new item adds",
                r#"{"VisibilityTimeout":"31","DelaySeconds":"0","Redrive":{"maxReceiveCount":4},"Policy":"{}"}"#,
                &[("stage", "prod")],
                &[(
                    "Configuration.Policy",
                    "null",
                    "\"{}\"",
                    CHANGE_TYPE_CREATE,
                )],
            ),
            (
                "a member the new item drops",
                r#"{"VisibilityTimeout":"31","Redrive":{"maxReceiveCount":4}}"#,
                &[("stage", "prod")],
                &[(
                    "Configuration.DelaySeconds",
                    "\"0\"",
                    "null",
                    CHANGE_TYPE_DELETE,
                )],
            ),
            (
                "a tag changed",
                r#"{"VisibilityTimeout":"31","DelaySeconds":"0","Redrive":{"maxReceiveCount":4}}"#,
                &[("stage", "dev")],
                &[("Tags.stage", "\"prod\"", "\"dev\"", CHANGE_TYPE_UPDATE)],
            ),
            (
                "nothing changed",
                r#"{"DelaySeconds":"0","Redrive":{"maxReceiveCount":4},"VisibilityTimeout":"31"}"#,
                &[("stage", "prod")],
                &[],
            ),
        ];

        for (case, configuration, tags, expected) in cases {
            let changed = diff(&before, configuration, tags);
            assert_eq!(
                changed.len(),
                expected.len(),
                "case {case:?}: expected {} changed properties, got {:?}",
                expected.len(),
                changed
            );
            for (property, held, now_held, property_change) in expected {
                let entry = changed.get(*property).unwrap_or_else(|| {
                    panic!("case {case:?}: expected {property:?} to be named, got {changed:?}")
                });
                let expected_entry = ::serde_json::json!({
                    "previousValue": document(held),
                    "updatedValue": document(now_held),
                    "changeType": property_change,
                });
                assert_eq!(
                    entry, &expected_entry,
                    "case {case:?}: expected {property:?} to be {expected_entry}, got {entry}"
                );
            }
        }
    }

    /// Only an item recording a CHANGED state names changed properties: a first item and
    /// the item recording a deletion name none.
    #[test]
    fn only_a_changed_item_names_changed_properties() {
        let before = recorded(r#"{"VisibilityTimeout":"31"}"#, &[]);
        let current = recorded_properties(Some(r#"{"VisibilityTimeout":"41"}"#), &BTreeMap::new(), &BTreeMap::new());
        let cases: [(&str, &str, Option<&RecordedConfigurationState>, usize); 4] = [
            ("a first item", CHANGE_TYPE_CREATE, None, 0),
            ("a first item with a state before it", CHANGE_TYPE_CREATE, Some(&before), 0),
            ("a changed item", CHANGE_TYPE_UPDATE, Some(&before), 1),
            ("the deletion of the resource", CHANGE_TYPE_DELETE, Some(&before), 0),
        ];

        for (case, change_type, previous, expected) in cases {
            let changed = changed_properties(change_type, previous, &current);
            assert_eq!(
                changed.len(),
                expected,
                "case {case:?}: expected {expected} changed properties, got {changed:?}"
            );
        }
    }

    /// The notification carries the item, the diff, the message type and the record
    /// version, and the item's `configuration` is the document it encodes rather than a
    /// JSON-encoded string.
    #[test]
    fn the_notification_carries_the_item_and_its_diff() {
        let item = ::serde_json::json!({
            "resourceType": "AWS::SQS::Queue",
            "resourceId": "orders",
            "configuration": { "VisibilityTimeout": "41" },
        });
        let mut changed = Map::new();
        changed.insert(
            "Configuration.VisibilityTimeout".to_string(),
            ::serde_json::json!({
                "previousValue": "31",
                "updatedValue": "41",
                "changeType": CHANGE_TYPE_UPDATE,
            }),
        );
        let notification = configuration_item_change_notification(
            &item,
            CHANGE_TYPE_UPDATE,
            changed,
            SystemTime::UNIX_EPOCH,
        );

        assert_eq!(
            notification["messageType"],
            ::serde_json::json!(CONFIGURATION_ITEM_CHANGE_MESSAGE_TYPE),
            "the notification names the message type it is, got {notification}"
        );
        assert_eq!(
            notification["recordVersion"],
            ::serde_json::json!(CONFIGURATION_ITEM_RECORD_VERSION),
            "the notification names the format it is written in, got {notification}"
        );
        assert_eq!(
            notification["configurationItemDiff"]["changeType"],
            ::serde_json::json!(CHANGE_TYPE_UPDATE),
            "the diff names the change the item records, got {notification}"
        );
        assert_eq!(
            notification["configurationItemDiff"]["changedProperties"]
                ["Configuration.VisibilityTimeout"]["updatedValue"],
            ::serde_json::json!("41"),
            "the diff names the value the property now holds, got {notification}"
        );
        assert_eq!(
            notification["configurationItem"]["configuration"]["VisibilityTimeout"],
            ::serde_json::json!("41"),
            "the item's configuration is the document it encodes, got {notification}"
        );
        assert_eq!(
            notification["configurationItem"]["configurationItemVersion"],
            ::serde_json::json!(CONFIGURATION_ITEM_RECORD_VERSION),
            "the item names the format version it is written in, got {notification}"
        );
        assert_eq!(
            notification["notificationCreationTime"],
            ::serde_json::json!("1970-01-01T00:00:00Z"),
            "the notification names when it was created, got {notification}"
        );
    }
}
