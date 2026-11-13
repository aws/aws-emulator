// SPDX-License-Identifier: CC0-1.0

//! S3's EVENT NOTIFICATION EMITTER: what a bucket's notification configuration DOES —
//! when it is installed, and when an object changes.
//!
//! [`super::BucketNotificationConfiguration`] is what
//! `PutBucketNotificationConfiguration` records and `GetBucketNotificationConfiguration`
//! reports. This module is what that record DOES: "the event types that you want Amazon
//! S3 to publish and the destination where you want Amazon S3 to publish an event
//! notification when it detects an event of the specified type"
//! (`PutBucketNotificationConfiguration`'s own documentation). An operation that changes
//! an object names the change ([`ObjectEvent`]), the store answers which configured
//! destinations are notified of it ([`pending`]), and the deliveries are made after the
//! change is recorded ([`deliver`]).
//!
//! Three rules decide the delivery, and they are the same for every object event and
//! every destination:
//!
//! * a destination configuration is notified when its `Events` list names the event —
//!   either literally or through the `s3:<Type>:*` form, which stands for every event of
//!   that type ([`event_matches`]);
//! * and when the object's key passes the configuration's key-name `Filter`: every
//!   `prefix` and `suffix` rule it carries has to hold, and a configuration with no
//!   `Filter` restricts nothing ([`key_matches`]);
//! * EventBridge is notified of the event classes it publishes a detail-type for, and
//!   only while the bucket's configuration carries an `EventBridgeConfiguration`, which
//!   "enables delivery of events to Amazon EventBridge".
//!
//! Every notification NAMES THE CALL it came of, by the request-correlation pair that
//! call's own response carries ([`super::request_ids`]): the `Records` envelope reports
//! both in its `responseElements` and the EventBridge detail reports the request id as
//! its `request-id`, which is how a consumer tells which API call produced which event.
//! It also names WHERE that call was made from — the `Records` envelope under
//! `requestParameters.sourceIPAddress` and the EventBridge detail under
//! `source-ip-address` — read off the request's own connection
//! ([`request_source_address`]) rather than from anything the request said about itself.
//!
//! The SAME destinations are resolved when the configuration is INSTALLED, which is the
//! one thing a notification configuration does before any object changes: S3 "first
//! verifies that any Amazon Simple Notification Service (Amazon SNS) or Amazon Simple
//! Queue Service (Amazon SQS) destination exists, and that the bucket owner has permission
//! to publish to it by sending a test notification". That is
//! [`unvalidated_destinations`]: each destination has to BE there and its own resource
//! policy has to let the Amazon S3 service principal publish this bucket's notifications to
//! it ([`destination_granted`]) — the same question for a queue, a topic and a function —
//! and the test notification it then publishes is the `s3:TestEvent` a consumer of a
//! validated destination receives.
//!
//! Delivery is BEST EFFORT and never changes what the operation answers: S3 publishes a
//! notification asynchronously, after the write that produced it was accepted, so a write
//! into a bucket whose destination service is not mounted on this engine still succeeds.
//! A delivery that could not be made is logged, because a deployment where a configured
//! destination cannot be reached is worth seeing.
//!
//! Nothing here opens a socket or leaves the host: every destination is reached through
//! the in-process callback ([`smithy_server_runtime::OperationContext::callback`]), the
//! way any emulated service reaches another, so this crate names no other service crate.

use ::std::time::SystemTime;

use super::request_ids::RequestCorrelation;
use super::{
    bucket_arn, canonical_user_id, NotificationTarget, FILTER_RULE_PREFIX, FILTER_RULE_SUFFIX,
};

/// The `eventSource` of every record S3 publishes.
const EVENT_SOURCE: &str = "aws:s3";

/// The `eventVersion` of the record format this emitter writes — the version of the
/// Amazon S3 event message structure whose fields it fills.
const EVENT_VERSION: &str = "2.1";

/// The `s3SchemaVersion` of the `s3` container inside one record.
const S3_SCHEMA_VERSION: &str = "1.0";

/// The `source` of an S3 event on an EventBridge bus.
const EVENT_BRIDGE_SOURCE: &str = "aws.s3";

/// The `version` an S3 event's EventBridge `detail` carries.
const EVENT_BRIDGE_DETAIL_VERSION: &str = "0";

/// The event bus an Amazon Web Services service's own events are published onto.
const DEFAULT_EVENT_BUS_NAME: &str = "default";

/// The separator between an event's TYPE and its detail in an `Event` value
/// (`s3:ObjectCreated:Put`), and so the character the `s3:<Type>:*` form wildcards
/// after.
const EVENT_DETAIL_SEPARATOR: char = ':';

/// The `s3:<Type>:*` suffix that stands for every event of one type.
const EVENT_WILDCARD_SUFFIX: &str = ":*";

/// The header a proxy in front of this emulator states the address it forwarded a request
/// for in, whose FIRST entry is the client's own — the same reading every other service in
/// this tree takes of a request's source address.
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The address a request carries when nothing forwarded it: this emulator serves its API
/// over the loopback interface, so a connection that reached it came from the local host.
const LOOPBACK_SOURCE_ADDRESS: &str = "127.0.0.1";

/// The address the request `parts` was made from, as a published event states it.
///
/// Read from the CONNECTION rather than from anything the request body says: the first
/// entry of [`FORWARDED_FOR_HEADER`] when a proxy stated the client it forwarded for, and
/// otherwise [`LOOPBACK_SOURCE_ADDRESS`], the host every connection to this emulator is
/// made from — the same reading this tree's other services take of a request's source
/// address.
///
/// The member this fills is an ADDRESS ("`sourceIPAddress`: IP address where request came
/// from"), so the forwarded entry is published only when it IS one: a header carrying
/// anything else names no address this event could report and is answered with the
/// loopback host, which also bounds the value a record carries to one parsed address
/// however long a header the transport accepted.
#[must_use]
pub fn request_source_address(parts: &::http::request::Parts) -> String {
    parts
        .headers
        .get(FORWARDED_FOR_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .and_then(|address| address.parse::<::std::net::IpAddr>().ok())
        .map_or_else(
            || LOOPBACK_SOURCE_ADDRESS.to_string(),
            |address| address.to_string(),
        )
}

/// Which object change happened, which is what decides the `Event` value a configuration
/// has to name to be notified, the EventBridge detail-type the event is published under,
/// and the `reason` the detail carries.
///
/// One variant per event value in the model's `Event` enum
/// (`smithy_query(query="shape",shape="Event")`) that a CALLER's own request performs.
/// The values that enum also carries for work no request performs here — a lost
/// reduced-redundancy object, a replication outcome, a lifecycle transition or
/// expiration, an Intelligent-Tiering move — have no variant: nothing in this emulator
/// does that work, so no notification of it could be anything but invented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectEventKind {
    /// `s3:ObjectCreated:Put` — a `PutObject` wrote the object.
    CreatedByPut,
    /// `s3:ObjectCreated:Post` — a browser-form `POST Object` upload wrote it.
    CreatedByPost,
    /// `s3:ObjectCreated:Copy` — a `CopyObject` wrote it.
    CreatedByCopy,
    /// `s3:ObjectCreated:CompleteMultipartUpload` — a multipart upload completed into it.
    CreatedByMultipartUpload,
    /// `s3:ObjectRemoved:Delete` — a version (or an unversioned object) was permanently
    /// removed.
    Removed,
    /// `s3:ObjectRemoved:DeleteMarkerCreated` — a delete on a versioned bucket laid a
    /// delete marker instead of removing anything.
    DeleteMarkerCreated,
    /// `s3:ObjectTagging:Put` — a tag set was placed on the object.
    TaggingPut,
    /// `s3:ObjectTagging:Delete` — the object's tag set was removed.
    TaggingDelete,
    /// `s3:ObjectAcl:Put` — the object's ACL was replaced.
    AclPut,
    /// `s3:ObjectRetention:Put` — an Object Lock retention was placed on the object.
    RetentionPut,
    /// `s3:ObjectAnnotation:Put` — an annotation was placed on the object.
    AnnotationPut,
    /// `s3:ObjectAnnotation:Delete` — an annotation was removed from the object.
    AnnotationDelete,
    /// `s3:ObjectRestore:Post` — a restore of an archived copy was initiated.
    RestoreInitiated,
}

impl ObjectEventKind {
    /// The `Event` value a configuration names to be notified of this change, and the
    /// `eventName` the published record carries without its `s3:` prefix.
    ///
    /// Every string here is one of [`super::EVENT_VALUES`], which is the model's own
    /// value list for the shape; the unit test below pins that.
    #[must_use]
    pub fn event_value(self) -> &'static str {
        match self {
            Self::CreatedByPut => "s3:ObjectCreated:Put",
            Self::CreatedByPost => "s3:ObjectCreated:Post",
            Self::CreatedByCopy => "s3:ObjectCreated:Copy",
            Self::CreatedByMultipartUpload => "s3:ObjectCreated:CompleteMultipartUpload",
            Self::Removed => "s3:ObjectRemoved:Delete",
            Self::DeleteMarkerCreated => "s3:ObjectRemoved:DeleteMarkerCreated",
            Self::TaggingPut => "s3:ObjectTagging:Put",
            Self::TaggingDelete => "s3:ObjectTagging:Delete",
            Self::AclPut => "s3:ObjectAcl:Put",
            Self::RetentionPut => "s3:ObjectRetention:Put",
            Self::AnnotationPut => "s3:ObjectAnnotation:Put",
            Self::AnnotationDelete => "s3:ObjectAnnotation:Delete",
            Self::RestoreInitiated => "s3:ObjectRestore:Post",
        }
    }

    /// The `eventName` of the published record: the `Event` value without its `s3:`
    /// prefix, which is how a record names the event ("ObjectCreated:Put").
    #[must_use]
    pub fn record_event_name(self) -> &'static str {
        self.event_value()
            .strip_prefix("s3:")
            .unwrap_or(self.event_value())
    }

    /// The EventBridge `detail-type` this event is published under, or `None` for an
    /// event class EventBridge publishes no detail-type for.
    ///
    /// The detail-types are the ones the EventBridge reference publishes for `aws.s3`
    /// ("Object Created", "Object Deleted", "Object Restore Initiated", "Object ACL
    /// Updated", "Object Tags Added", "Object Tags Deleted" and the rest). An Object Lock
    /// retention and an object annotation are among the bucket events a notification
    /// configuration can name and NOT among the event types EventBridge publishes, so
    /// their destinations are notified and the bus is not: publishing them under an
    /// invented detail-type would be a rule no pattern could be written against.
    #[must_use]
    pub fn event_bridge_detail_type(self) -> Option<&'static str> {
        match self {
            Self::CreatedByPut
            | Self::CreatedByPost
            | Self::CreatedByCopy
            | Self::CreatedByMultipartUpload => Some("Object Created"),
            Self::Removed | Self::DeleteMarkerCreated => Some("Object Deleted"),
            Self::TaggingPut => Some("Object Tags Added"),
            Self::TaggingDelete => Some("Object Tags Deleted"),
            Self::AclPut => Some("Object ACL Updated"),
            Self::RestoreInitiated => Some("Object Restore Initiated"),
            Self::RetentionPut | Self::AnnotationPut | Self::AnnotationDelete => None,
        }
    }

    /// The `detail.reason` of the EventBridge event: the API call that made the change,
    /// as the EventBridge reference states it for each S3 event type.
    #[must_use]
    fn event_bridge_reason(self) -> &'static str {
        match self {
            Self::CreatedByPut => "PutObject",
            // The browser-based upload is the one object write the reference names by its
            // HTTP form rather than by an API name: the `reason` of an "Object Created"
            // event the form POST produced is "POST Object", which is what a rule scoped to
            // one of the two reasons is matched against.
            Self::CreatedByPost => "POST Object",
            Self::CreatedByCopy => "CopyObject",
            Self::CreatedByMultipartUpload => "CompleteMultipartUpload",
            Self::Removed | Self::DeleteMarkerCreated => "DeleteObject",
            Self::TaggingPut => "PutObjectTagging",
            Self::TaggingDelete => "DeleteObjectTagging",
            Self::AclPut => "PutObjectAcl",
            Self::RetentionPut => "PutObjectRetention",
            Self::AnnotationPut => "PutObjectAnnotation",
            Self::AnnotationDelete => "DeleteObjectAnnotation",
            Self::RestoreInitiated => "RestoreObject",
        }
    }

    /// The `detail.deletion-type` an "Object Deleted" event carries: which of the two
    /// things a delete did, as the EventBridge reference words them.
    #[must_use]
    fn deletion_type(self) -> Option<&'static str> {
        match self {
            Self::Removed => Some("Permanently Deleted"),
            Self::DeleteMarkerCreated => Some("Delete Marker Created"),
            _ => None,
        }
    }
}

/// One object change to notify about.
///
/// Built by the operation that made the change, from the object it wrote or removed —
/// never from the request alone, so a record reports the size, ETag and version id the
/// store actually holds.
#[derive(Debug, Clone)]
pub struct ObjectEvent {
    /// Which change happened.
    pub kind: ObjectEventKind,
    /// The object's key, as it is stored.
    pub key: String,
    /// The object's size in bytes, when the change left an object at the key. A removal
    /// reports none, exactly as S3's own record omits it.
    pub size: Option<i64>,
    /// The object's ETag, on the same terms as `size`.
    pub etag: Option<String>,
    /// The version id the change applies to, when the object carries one.
    pub version_id: Option<String>,
    /// When the change happened.
    pub at: SystemTime,
}

impl ObjectEvent {
    /// The event for `kind` on `key`, at this instant, carrying nothing about an object
    /// yet.
    #[must_use]
    pub fn new(kind: ObjectEventKind, key: impl Into<String>) -> Self {
        Self {
            kind,
            key: key.into(),
            size: None,
            etag: None,
            version_id: None,
            at: SystemTime::now(),
        }
    }

    /// The same event, reporting the size, ETag and version id of the STORED object the
    /// change left at the key.
    #[must_use]
    pub fn of_object(mut self, object: &super::Object) -> Self {
        self.size = Some(i64::try_from(object.body.len()).unwrap_or(i64::MAX));
        self.etag = Some(object.etag.clone());
        self.version_id = object.version_id.clone();
        self
    }

    /// The same event, reporting the version id the change applies to (the version a
    /// delete removed, or the delete marker it laid).
    #[must_use]
    pub fn of_version(mut self, version_id: Option<String>) -> Self {
        self.version_id = version_id;
        self
    }

    /// The `sequencer` of the published record: "a string representation of a
    /// hexadecimal value used to determine event sequence" for one key, so it is derived
    /// from the instant the change happened and increases with it.
    fn sequencer(&self) -> String {
        let nanos = self
            .at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        // Fixed width, so two sequencers of one key compare lexicographically the way
        // the instants they were taken at compare — which is what a consumer uses them
        // for.
        format!("{nanos:016X}")
    }
}

/// The characters the key in a published record is NOT percent-encoded over: RFC 3986's
/// unreserved set plus the `/` of a key prefix.
///
/// A record's key "is URL encoded" (Amazon S3 User Guide, *Event message structure*),
/// which is why the published value is encoded at all; the path separator is left alone
/// so a prefixed key reads as the path it is.
const RECORD_KEY_UNRESERVED: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// `key` as a published record spells it.
///
/// Percent-encoded over [`RECORD_KEY_UNRESERVED`], with a space as `+`: the User Guide's
/// own example of the encoding is a key whose space appears as a plus, which is why a
/// consumer decodes it the way an HTML form value is decoded.
fn record_key(key: &str) -> String {
    ::percent_encoding::utf8_percent_encode(key, RECORD_KEY_UNRESERVED)
        .to_string()
        .replace("%20", "+")
}

/// Whether `configured` — one `Event` value a configuration names — is notified of the
/// event `actual`.
///
/// Two forms: the value itself, and the `s3:<Type>:*` form, which stands for every event
/// of that type ("you can enable notifications for every event of a type by using the
/// wildcard"). The wildcard spans one type only, so it is matched by the type prefix up
/// to the detail separator rather than by a bare `starts_with`, and
/// `s3:ReducedRedundancyLostObject`, which has no detail part, matches nothing but
/// itself.
#[must_use]
pub fn event_matches(configured: &str, actual: &str) -> bool {
    if configured == actual {
        return true;
    }
    let Some(kind) = configured.strip_suffix(EVENT_WILDCARD_SUFFIX) else {
        return false;
    };
    actual
        .strip_prefix(kind)
        .is_some_and(|detail| detail.starts_with(EVENT_DETAIL_SEPARATOR))
}

/// Whether `key` passes the key-name filter rules of one configuration.
///
/// "The Amazon S3 API supports filtering based on the prefix and suffix of the object
/// key name" (`NotificationConfigurationFilter`): EVERY rule the filter carries has to
/// hold, so a filter naming both a prefix and a suffix notifies only the keys with both.
/// A configuration carrying no `Filter` — `None` — restricts nothing, and so does one
/// whose rule names no value.
///
/// A rule is read by its KIND ([`super::NotificationFilterRule::kind`]), not by the text
/// of its `Name`: the store holds a rule under the canonical capitalised spelling and the
/// request vocabulary spells it in lower case, and both name the same rule.
#[must_use]
pub fn key_matches(filter_rules: Option<&Vec<super::NotificationFilterRule>>, key: &str) -> bool {
    filter_rules.into_iter().flatten().all(|rule| {
        let value = rule.filtered_value();
        match rule.kind() {
            Some(FILTER_RULE_PREFIX) => key.starts_with(value),
            Some(FILTER_RULE_SUFFIX) => key.ends_with(value),
            // The store holds no other rule name: a put naming one is refused
            // (`super::validate_notification_configuration`), so a rule that is neither
            // filters on nothing this emitter can read and is not treated as a match.
            _ => false,
        }
    })
}

/// Whether `target` is notified of `event`.
fn notifies(target: &NotificationTarget, event: &ObjectEvent) -> bool {
    target
        .events
        .iter()
        .any(|configured| event_matches(configured, event.kind.event_value()))
        && key_matches(target.filter_rules.as_ref(), &event.key)
}

/// One notification to make, rendered: where it goes and what it carries.
#[derive(Debug, Clone)]
enum Delivery {
    /// A `PutEvents` onto the default bus of `region`.
    EventBridge {
        region: String,
        detail_type: &'static str,
        detail: String,
        /// The bucket the change happened in, which is the one resource an `aws.s3`
        /// event names.
        resource: String,
    },
    /// A `SendMessage` to the queue the configuration named.
    Queue {
        region: String,
        url: String,
        body: String,
    },
    /// A `Publish` to the topic the configuration named.
    Topic {
        region: String,
        arn: String,
        body: String,
    },
    /// An asynchronous `Invoke` of the function the configuration named.
    Function {
        region: String,
        arn: String,
        payload: String,
    },
}

/// The notifications one or more object events owe, rendered while the store is held and
/// delivered once it has been released.
///
/// Empty — and so free — for a bucket with no event notifications configured, which is
/// the state every bucket is created in.
#[derive(Debug, Clone, Default)]
pub struct Pending(Vec<Delivery>);

impl Pending {
    /// Whether there is nothing to deliver.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many deliveries are owed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// The most deliveries one request's object events may owe.
///
/// An EMULATOR ceiling, not an AWS quota: a single request can produce many events (one
/// `DeleteObjects` carries up to a thousand identifiers) and a bucket may carry up to
/// [`super::MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET`] destination configurations, so
/// the PRODUCT is what one call can ask this process to render and deliver. Past this
/// ceiling the remaining deliveries are dropped and logged rather than rendered, so no
/// one request can grow the work — or the memory holding it — without bound.
pub const MAX_DELIVERIES_PER_REQUEST: usize = 1_000;

/// The notifications `events` owe on the bucket named `bucket`, according to the
/// configuration that bucket carries, each naming the call they came of by `correlation`
/// and the address that call was made from (`source_address`, which
/// [`request_source_address`] reads off the request).
///
/// Computed while the caller still holds the store, because what is notified depends on
/// the bucket's configuration and on the object the change left behind. A bucket this
/// store does not hold, or one with no notifications configured, owes nothing, and no
/// request owes more than [`MAX_DELIVERIES_PER_REQUEST`].
#[must_use]
pub fn pending(
    store: &super::ComAmazonawsS3Amazons3State,
    bucket: &str,
    events: &[ObjectEvent],
    correlation: &RequestCorrelation,
    source_address: &str,
) -> Pending {
    let Some(record) = store.bucket(bucket) else {
        return Pending::default();
    };
    let configuration = record.notification_configuration();
    if configuration.is_empty() || events.is_empty() {
        return Pending::default();
    }
    let bucket_region = record.region.clone();
    let mut deliveries = Vec::new();
    // Whether the ceiling cut a delivery this configuration really owed, which is what
    // the warning below reports: a fan-out that lands exactly ON the ceiling dropped
    // nothing.
    let mut truncated = false;
    for event in events {
        if deliveries.len() >= MAX_DELIVERIES_PER_REQUEST {
            truncated = true;
            break;
        }
        if configuration.event_bridge_enabled {
            if let Some(detail_type) = event.kind.event_bridge_detail_type() {
                deliveries.push(Delivery::EventBridge {
                    region: bucket_region.clone(),
                    detail_type,
                    detail: event_bridge_detail(bucket, event, correlation, source_address)
                        .to_string(),
                    resource: bucket_arn(bucket),
                });
            }
        }
        for (destination, target) in configuration.targets() {
            if deliveries.len() >= MAX_DELIVERIES_PER_REQUEST {
                truncated = true;
                break;
            }
            if !notifies(target, event) {
                continue;
            }
            // Each destination is reached in the Region ITS OWN ARN names — a
            // destination ARN carries one (`super::validate_notification_configuration`
            // refuses one that does not) — falling back to the bucket's Region.
            let region = destination_region(&target.arn).unwrap_or(&bucket_region);
            let body = records_envelope(
                bucket,
                &bucket_region,
                target,
                event,
                correlation,
                source_address,
            )
            .to_string();
            deliveries.push(match destination {
                super::NotificationDestination::Queue => match queue_url(&target.arn) {
                    Some(url) => Delivery::Queue {
                        region: region.to_string(),
                        url,
                        body,
                    },
                    None => continue,
                },
                super::NotificationDestination::Topic => Delivery::Topic {
                    region: region.to_string(),
                    arn: target.arn.clone(),
                    body,
                },
                super::NotificationDestination::LambdaFunction => Delivery::Function {
                    region: region.to_string(),
                    arn: target.arn.clone(),
                    payload: body,
                },
            });
        }
    }
    if truncated {
        ::tracing::warn!(
            deliveries = deliveries.len(),
            events = events.len(),
            "this request's object events owe more notifications than one request \
             delivers; the rest are dropped"
        );
    }
    Pending(deliveries)
}

/// The Region a destination ARN names, or `None` when the value is not an ARN with one.
///
/// Read through the ONE shared ARN parse (`service_common::arn`), so this crate does not
/// split a stored ARN its own way.
fn destination_region(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    (!parsed.region.is_empty()).then_some(parsed.region)
}

/// The `QueueUrl` SQS addresses the queue named by `arn` at:
/// `<endpoint>/<account>/<QueueName>`, the form every SQS operation takes.
fn queue_url(arn: &str) -> Option<String> {
    let parsed = ::service_common::arn::parse(arn)?;
    let name = parsed.resource;
    // A queue's ARN names the queue directly: a resource field carrying a separator is
    // not one, so it addresses no queue.
    if name.contains('/') || name.contains(':') {
        return None;
    }
    Some(format!(
        "https://sqs.{}.amazonaws.com/{}/{name}",
        parsed.region, parsed.account
    ))
}

/// The message one notification carries to a topic, a queue or a function: the
/// `Records` envelope of the Amazon S3 event message structure, with the one record this
/// event produces.
fn records_envelope(
    bucket: &str,
    bucket_region: &str,
    target: &NotificationTarget,
    event: &ObjectEvent,
    correlation: &RequestCorrelation,
    source_address: &str,
) -> ::serde_json::Value {
    let mut object = ::serde_json::json!({
        "key": record_key(&event.key),
        "sequencer": event.sequencer(),
    });
    if let Some(size) = event.size {
        object["size"] = ::serde_json::json!(size);
    }
    if let Some(etag) = event.etag.as_deref() {
        object["eTag"] = ::serde_json::json!(super::unquoted_etag(etag));
    }
    if let Some(version_id) = event.version_id.as_deref() {
        object["versionId"] = ::serde_json::json!(version_id);
    }
    ::serde_json::json!({
        "Records": [{
            "eventVersion": EVENT_VERSION,
            "eventSource": EVENT_SOURCE,
            "awsRegion": bucket_region,
            "eventTime": event_instant(event.at),
            "eventName": event.kind.record_event_name(),
            // "The Amazon Web Services account ID or IAM user who made the request" —
            // this emulator serves one account, named by the canonical user id every
            // other S3 response names an owner by.
            "userIdentity": { "principalId": canonical_user_id() },
            // "The request parameters" of the call the event came of, whose one member is
            // the address it was made from: "`sourceIPAddress`: IP address where request
            // came from" (Amazon S3 User Guide, *Event message structure*). Read off the
            // request's own connection ([`request_source_address`]), so a consumer that
            // reads it is told where the change was made from rather than finding the
            // member absent.
            "requestParameters": { "sourceIPAddress": source_address },
            // "The response elements … useful if you want to trace a request by
            // following up with AWS Support": the pair the response of the call that
            // produced this event carried.
            "responseElements": {
                "x-amz-request-id": correlation.request_id.clone(),
                "x-amz-id-2": correlation.extended_request_id.clone(),
            },
            "s3": {
                "s3SchemaVersion": S3_SCHEMA_VERSION,
                // "The ID found in the bucket notification configuration" — the id of
                // the configuration this delivery is being made for, which every STORED
                // configuration carries.
                "configurationId": target.id.clone().unwrap_or_default(),
                "bucket": {
                    "name": bucket,
                    "ownerIdentity": { "principalId": canonical_user_id() },
                    "arn": bucket_arn(bucket),
                },
                "object": object,
            },
        }],
    })
}

/// The `detail` of the EventBridge event one object event is published with.
///
/// The members are the ones the EventBridge reference publishes for an `aws.s3` event:
/// the detail version, the bucket's name, the object's key with the facts S3 holds about
/// it, the account that made the change, the request that made it, the address that
/// request was made from and the reason it happened.
fn event_bridge_detail(
    bucket: &str,
    event: &ObjectEvent,
    correlation: &RequestCorrelation,
    source_address: &str,
) -> ::serde_json::Value {
    let mut object = ::serde_json::json!({
        "key": event.key,
        "sequencer": event.sequencer(),
    });
    if let Some(size) = event.size {
        object["size"] = ::serde_json::json!(size);
    }
    if let Some(etag) = event.etag.as_deref() {
        object["etag"] = ::serde_json::json!(super::unquoted_etag(etag));
    }
    if let Some(version_id) = event.version_id.as_deref() {
        object["version-id"] = ::serde_json::json!(version_id);
    }
    let mut detail = ::serde_json::json!({
        "version": EVENT_BRIDGE_DETAIL_VERSION,
        "bucket": { "name": bucket },
        "object": object,
        "request-id": correlation.request_id.clone(),
        "requester": ::engine::emulator_account_id(),
        // The address the call was made from, which this envelope states under the name
        // the EventBridge reference gives it for an `aws.s3` event.
        "source-ip-address": source_address,
        "reason": event.kind.event_bridge_reason(),
    });
    if let Some(deletion_type) = event.kind.deletion_type() {
        detail["deletion-type"] = ::serde_json::json!(deletion_type);
    }
    detail
}

/// `at` as an event states a time: the RFC 3339 instant the modelled timestamp format
/// renders, which is what a consumer parses `eventTime` as.
///
/// The fallback is for an instant that form cannot state (a year outside the four-digit
/// range), which this process's own clock does not produce; it is logged rather than
/// silently published, because a member with an empty value would be indistinguishable
/// from one a record does not carry.
fn event_instant(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| {
            ::tracing::warn!("an object event instant could not be stated as RFC 3339");
            String::new()
        })
}

/// How many delivery tasks this process may have in flight at once.
///
/// An EMULATOR ceiling, not an AWS quota, and the companion to
/// [`MAX_DELIVERIES_PER_REQUEST`]: that bounds the work ONE request can ask for, this
/// bounds how many requests' worth of it can be running at the same time, so a caller
/// looping on writes cannot spawn tasks without end. A request that finds every permit
/// taken delivers nothing and says so in the log, which is the same outcome as a
/// destination that cannot be reached — delivery is best effort either way, and what the
/// caller is told about its write does not depend on it.
pub const MAX_IN_FLIGHT_DELIVERY_TASKS: usize = 32;

/// How long one request's whole fan-out may take before it is abandoned.
///
/// An EMULATOR ceiling as well: the transport is in-process, so a delivery waits only on
/// another handler of this same emulator, and the clients below disable retries and the
/// SDK's own timeouts (there is nothing to retry or to watch for a stalled stream behind
/// an in-process transport). This is what keeps a task that is waiting on a slow handler
/// from holding its permit for the life of the process.
pub const DELIVERY_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(30);

/// How many delivery tasks are in flight right now; see [`MAX_IN_FLIGHT_DELIVERY_TASKS`].
static IN_FLIGHT_DELIVERY_TASKS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One task's claim on an in-flight permit, released when the task ends — including when
/// it ends by panicking or by being dropped at runtime shutdown, because the release is a
/// `Drop`.
struct DeliveryPermit;

impl DeliveryPermit {
    /// Claim a permit, or `None` when [`MAX_IN_FLIGHT_DELIVERY_TASKS`] are already in
    /// flight.
    //
    // The compare-and-update this bound needs is the long-standing atomic method,
    // which a newer toolchain renames and so reports as deprecated here. It is kept
    // rather than swapped for the new name, which an older toolchain does not have:
    // the name says nothing about the bound, and only the old one compiles on both.
    #[allow(deprecated)]
    fn claim() -> Option<Self> {
        IN_FLIGHT_DELIVERY_TASKS
            .try_update(
                ::std::sync::atomic::Ordering::SeqCst,
                ::std::sync::atomic::Ordering::SeqCst,
                |in_flight| (in_flight < MAX_IN_FLIGHT_DELIVERY_TASKS).then_some(in_flight + 1),
            )
            .ok()
            .map(|_| Self)
    }
}

impl Drop for DeliveryPermit {
    fn drop(&mut self) {
        IN_FLIGHT_DELIVERY_TASKS.fetch_sub(1, ::std::sync::atomic::Ordering::SeqCst);
    }
}

/// Deliver the notifications `pending` owes, on the runtime's own time.
///
/// The entry point every operation calls, with the store's guard released: S3 publishes
/// a notification ASYNCHRONOUSLY — the write that produced it is already recorded and
/// answered — so the deliveries are made on a task of their own rather than on the
/// request's own path, exactly as the account's other in-process pumps are. With no async
/// runtime to spawn onto there is nothing to deliver on, and nothing a caller is told
/// depends on it.
///
/// What bounds the task: it carries at most [`MAX_DELIVERIES_PER_REQUEST`] deliveries,
/// at most [`MAX_IN_FLIGHT_DELIVERY_TASKS`] of them run at once, each delivery is one
/// await with no retry loop behind it, and the whole fan-out is abandoned after
/// [`DELIVERY_DEADLINE`] — so one request's deliveries cannot outlive that deadline, hold
/// their permit past it, or keep the runtime from shutting down.
pub fn publish(ctx: &smithy_server_runtime::OperationContext<'_>, pending: Pending) {
    if pending.is_empty() {
        return;
    }
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        ::tracing::warn!(
            deliveries = pending.len(),
            "no async runtime is available to deliver this bucket's object events on"
        );
        return;
    };
    let Some(permit) = DeliveryPermit::claim() else {
        ::tracing::warn!(
            deliveries = pending.len(),
            in_flight = MAX_IN_FLIGHT_DELIVERY_TASKS,
            "this emulator is already delivering as many object-event fan-outs as it \
             runs at once; these deliveries are dropped"
        );
        return;
    };
    // The callback is an owned handle into this same emulator, so the task that makes the
    // deliveries outlives the request that produced them.
    let callback = ctx.callback().clone();
    let deliveries = pending.len();
    runtime.spawn(async move {
        // Moved into the task, so the permit is held for exactly as long as the task
        // runs and is released however it ends.
        let _permit = permit;
        if ::tokio::time::timeout(DELIVERY_DEADLINE, deliver(&callback, pending))
            .await
            .is_err()
        {
            ::tracing::warn!(
                deliveries,
                deadline_secs = DELIVERY_DEADLINE.as_secs(),
                "this bucket's object-event deliveries did not finish inside the \
                 deadline and were abandoned"
            );
        }
    });
}

/// Make the deliveries `pending` owes.
///
/// Every one is BEST EFFORT: a failure is logged and nothing is retried, because the
/// operation that produced the event has already been accepted and S3 answers it whether
/// or not a consumer is reachable.
pub async fn deliver(callback: &::smithy_server_runtime::EmbeddedEmulator, pending: Pending) {
    if pending.is_empty() {
        return;
    }
    let mut clients = Clients::default();
    for delivery in pending.0 {
        match delivery {
            Delivery::EventBridge {
                region,
                detail_type,
                detail,
                resource,
            } => {
                let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                    .event_bus_name(DEFAULT_EVENT_BUS_NAME)
                    .source(EVENT_BRIDGE_SOURCE)
                    .detail_type(detail_type)
                    .detail(detail)
                    .resources(resource)
                    .build();
                let answer = clients
                    .event_bridge(callback, &region)
                    .put_events()
                    .entries(entry)
                    .send()
                    .await;
                match answer {
                    Ok(output) if output.failed_entry_count() == 0 => {}
                    Ok(output) => ::tracing::warn!(
                        detail_type,
                        failed_entry_count = output.failed_entry_count(),
                        "EventBridge refused the object event this bucket published"
                    ),
                    Err(error) => {
                        report(DELIVERY_NOT_MADE, "EventBridge", detail_type, &error)
                    }
                }
            }
            Delivery::Queue { region, url, body } => {
                let answer = clients
                    .sqs(callback, &region)
                    .send_message()
                    .queue_url(&url)
                    .message_body(body)
                    .send()
                    .await;
                if let Err(error) = answer {
                    report(DELIVERY_NOT_MADE, "an Amazon SQS queue", &url, &error);
                }
            }
            Delivery::Topic { region, arn, body } => {
                let answer = clients
                    .sns(callback, &region)
                    .publish()
                    .topic_arn(&arn)
                    .message(body)
                    .send()
                    .await;
                if let Err(error) = answer {
                    report(DELIVERY_NOT_MADE, "an Amazon SNS topic", &arn, &error);
                }
            }
            Delivery::Function {
                region,
                arn,
                payload,
            } => {
                // S3 invokes a function destination ASYNCHRONOUSLY: the write does not
                // wait for the handler's answer, and "a success response (202 Accepted)
                // indicates that the request is queued for invocation"
                // (`Invoke$InvocationType`'s `Event` value).
                let answer = clients
                    .lambda(callback, &region)
                    .invoke()
                    .function_name(&arn)
                    .invocation_type(::aws_sdk_lambda::types::InvocationType::Event)
                    .payload(::aws_sdk_lambda::primitives::Blob::new(payload.into_bytes()))
                    .send()
                    .await;
                if let Err(error) = answer {
                    report(DELIVERY_NOT_MADE, "a Lambda function", &arn, &error);
                }
            }
        }
    }
}

/// The sentence [`report`] logs for a notification that did not reach its destination:
/// delivery is best effort, so the operation that produced the event was already answered.
const DELIVERY_NOT_MADE: &str = "an object event this bucket published did not reach its destination";

/// The sentence [`report`] logs for a destination the service that owns it gave NO verdict
/// on, which is why the configuration naming it is recorded as given.
const DESTINATION_NOT_RESOLVED: &str = "a notification destination could not be resolved \
                                        against the service that owns it; the configuration \
                                        naming it is recorded as given";

/// Log one call to a destination that did not succeed — `outcome` says what that means —
/// with the error rendered the way the SDK renders a failure.
fn report<E>(outcome: &'static str, destination: &str, named: &str, error: &E)
where
    E: ::std::error::Error + Send + Sync + 'static,
{
    ::tracing::warn!(
        destination,
        named,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "{outcome}"
    );
}

/// The `Event` value of the message S3 publishes to a destination to VALIDATE it.
///
/// "After Amazon S3 receives this request, it first verifies that any Amazon Simple
/// Notification Service (Amazon SNS) or Amazon Simple Queue Service (Amazon SQS)
/// destination exists, and that the bucket owner has permission to publish to it by
/// sending a test notification" (`PutBucketNotificationConfiguration`'s own
/// documentation): that test notification is this event, so a consumer of a destination
/// whose configuration was accepted finds it waiting on the destination.
const TEST_EVENT: &str = "s3:TestEvent";

/// The `Service` the test notification names as its publisher.
const TEST_EVENT_SERVICE: &str = "Amazon S3";

/// The test notification S3 publishes to one destination: the members the *Event message
/// structure*'s test message carries — the service and the event that produced it, when it
/// was published, the bucket it was configured on, and the pair naming the call that
/// configured it ([`super::request_ids`]), which is also the pair that call's own response
/// carries.
fn test_notification(bucket: &str, correlation: &RequestCorrelation) -> ::serde_json::Value {
    ::serde_json::json!({
        "Service": TEST_EVENT_SERVICE,
        "Event": TEST_EVENT,
        "Time": event_instant(SystemTime::now()),
        "Bucket": bucket,
        "RequestId": correlation.request_id.clone(),
        "HostId": correlation.extended_request_id.clone(),
    })
}

/// The destinations of `configuration` that could NOT be validated, in configuration order,
/// each with the per-destination DETAIL saying why — empty when every one of them was, which
/// is what lets the put be recorded.
///
/// Each destination is resolved against the service that owns it, through this same
/// emulator, and two things are asked about it, because S3 "first verifies that any Amazon
/// Simple Notification Service (Amazon SNS) or Amazon Simple Queue Service (Amazon SQS)
/// destination EXISTS, and that the bucket owner has PERMISSION to publish to it"
/// (`PutBucketNotificationConfiguration`'s own documentation): the destination is read from
/// its own service, and its resource policy is evaluated for the Amazon S3 service principal
/// publishing this bucket's notifications ([`destination_granted`]). A queue and a topic are
/// then sent the test notification ([`test_notification`]), which is what a consumer of a
/// validated destination finds waiting on it; a function is not invoked, because S3
/// "verifies that the Lambda function permissions grant Amazon S3 permission to invoke the
/// function" instead. Nothing else is written by any of them, so a put this refuses leaves
/// every service it asked exactly as it was.
///
/// A destination its own service REFUSES is not validated: "the PUT notification is an
/// atomic operation … if the message fails, the entire PUT action will fail, and Amazon S3
/// will not add the configuration to your bucket". Where this emulator could not reach that
/// service AT ALL ([`TRANSPORT_REFUSALS`]) nothing judged the destination, so it is treated
/// as validated and the failure logged — the outcome
/// `PutBucketNotificationConfigurationRequest$SkipDestinationValidation` asks for
/// explicitly.
///
/// Nothing here opens a socket or leaves the host: every destination is reached through the
/// in-process callback, the way a delivery to it is, and the ARN decides only which service
/// of this same emulator is asked and in which Region. The work is bounded by the container
/// itself — at most [`super::MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET`] destinations, two
/// awaits each, with the store's guard released — so one request cannot grow it.
pub async fn unvalidated_destinations(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    bucket: &str,
    bucket_region: &str,
    configuration: &super::BucketNotificationConfiguration,
    correlation: &RequestCorrelation,
) -> Vec<(String, String)> {
    let mut clients = Clients::default();
    let mut unvalidated = Vec::new();
    let notification = test_notification(bucket, correlation).to_string();
    for (destination, target) in configuration.targets() {
        // Each destination is reached in the Region ITS OWN ARN names, exactly as a
        // delivery to it is, falling back to the bucket's Region.
        let region = destination_region(&target.arn)
            .unwrap_or(bucket_region)
            .to_string();
        // Each arm is BOXED: the call it makes is another service's whole handler, reached
        // through the in-process callback, and a validation that runs inside a deployment is
        // already several handlers down — the stack-held future of this one must not carry
        // the called handler's as well, or a put made from a CloudFormation deployment takes
        // the thread past its stack.
        let rejected = match destination {
            super::NotificationDestination::Queue => {
                Box::pin(queue_rejection(
                    &mut clients,
                    callback,
                    &region,
                    &target.arn,
                    bucket,
                    &notification,
                ))
                .await
            }
            super::NotificationDestination::Topic => {
                Box::pin(topic_rejection(
                    &mut clients,
                    callback,
                    &region,
                    &target.arn,
                    bucket,
                    &notification,
                ))
                .await
            }
            super::NotificationDestination::LambdaFunction => {
                Box::pin(function_rejection(
                    &mut clients,
                    callback,
                    &region,
                    &target.arn,
                    bucket,
                ))
                .await
            }
        };
        if let Some(rejection) = rejected {
            unvalidated.push((target.arn.clone(), rejection.detail(destination)));
        }
    }
    unvalidated
}

/// Why ONE destination of a notification configuration did not validate.
///
/// The refusal's own message is the same sentence however many destinations failed and
/// whichever way they did, so this is what the per-destination detail beside it states (see
/// [`super::destination_absent_detail`] and [`super::destination_permissions_detail`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DestinationRejection {
    /// The service that owns the destination does not hold it.
    Absent,
    /// It is there, and its resource policy does not let Amazon S3 publish this bucket's
    /// notifications to it.
    Unpermitted,
}

impl DestinationRejection {
    /// The detail S3's refusal carries for this destination kind.
    fn detail(self, destination: super::NotificationDestination) -> String {
        match self {
            Self::Absent => super::destination_absent_detail(destination),
            Self::Unpermitted => super::destination_permissions_detail(destination),
        }
    }
}

/// The IAM action S3 needs on a QUEUE destination to publish a notification to it, and the
/// one it needs on a TOPIC destination — the grant each destination's own resource policy
/// has to state for the S3 service principal.
const SEND_MESSAGE_ACTION: &str = "sqs:SendMessage";
/// See [`SEND_MESSAGE_ACTION`].
const PUBLISH_ACTION: &str = "sns:Publish";

/// Why the queue `arn` names is not a destination this configuration may be recorded with,
/// or `None` when it is one.
async fn queue_rejection(
    clients: &mut Clients,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    arn: &str,
    bucket: &str,
    notification: &str,
) -> Option<DestinationRejection> {
    // A value that addresses no queue at all names no destination to validate. The stored
    // ARN is already known to be an SQS ARN (`super::validate_notification_configuration`),
    // so this is the resource field that does not name a queue.
    let Some(url) = queue_url(arn) else {
        return Some(DestinationRejection::Absent);
    };
    // The queue's `Policy` attribute is "the queue's policy. A valid Amazon Web Services
    // policy" (`GetQueueAttributes`), and `QueueArn` is the ARN that policy's statements
    // name — so the grant is judged against the ARN SQS itself resolved the queue to.
    let answer = clients
        .sqs(callback, region)
        .get_queue_attributes()
        .queue_url(&url)
        .attribute_names(::aws_sdk_sqs::types::QueueAttributeName::Policy)
        .attribute_names(::aws_sdk_sqs::types::QueueAttributeName::QueueArn)
        .send()
        .await;
    let attributes = match answer {
        Ok(answered) => answered,
        Err(error) => {
            if refused_by_its_service(&error) {
                return Some(DestinationRejection::Absent);
            }
            unverifiable("an Amazon SQS queue", &url, &error);
            return None;
        }
    };
    let attribute = |name: &::aws_sdk_sqs::types::QueueAttributeName| {
        attributes
            .attributes()
            .and_then(|held| held.get(name))
            .cloned()
            .unwrap_or_default()
    };
    let resolved = attribute(&::aws_sdk_sqs::types::QueueAttributeName::QueueArn);
    let document = attribute(&::aws_sdk_sqs::types::QueueAttributeName::Policy);
    let queue_arn = if resolved.is_empty() { arn } else { &resolved };
    if !destination_granted(&document, SEND_MESSAGE_ACTION, queue_arn, bucket) {
        return Some(DestinationRejection::Unpermitted);
    }
    let answer = clients
        .sqs(callback, region)
        .send_message()
        .queue_url(&url)
        .message_body(notification)
        .send()
        .await;
    match answer {
        Ok(_) => None,
        Err(error) => {
            if refused_by_its_service(&error) {
                return Some(DestinationRejection::Absent);
            }
            unverifiable("an Amazon SQS queue", &url, &error);
            None
        }
    }
}

/// Why the topic `arn` names is not a destination this configuration may be recorded with,
/// or `None` when it is one.
///
/// The topic's `Policy` attribute is the document that decides it — "the JSON serialization
/// of the topic's access control policy" (`GetTopicAttributes`) — which is where the grant
/// S3 needs lives, so a topic that exists and states no such grant is refused rather than
/// published to.
async fn topic_rejection(
    clients: &mut Clients,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    arn: &str,
    bucket: &str,
    notification: &str,
) -> Option<DestinationRejection> {
    let answer = clients
        .sns(callback, region)
        .get_topic_attributes()
        .topic_arn(arn)
        .send()
        .await;
    let document = match answer {
        Ok(answered) => answered
            .attributes()
            .and_then(|held| held.get(TOPIC_POLICY_ATTRIBUTE))
            .cloned()
            .unwrap_or_default(),
        Err(error) => {
            if refused_by_its_service(&error) {
                return Some(DestinationRejection::Absent);
            }
            unverifiable("an Amazon SNS topic", arn, &error);
            return None;
        }
    };
    if !destination_granted(&document, PUBLISH_ACTION, arn, bucket) {
        return Some(DestinationRejection::Unpermitted);
    }
    let answer = clients
        .sns(callback, region)
        .publish()
        .topic_arn(arn)
        .message(notification)
        .send()
        .await;
    match answer {
        Ok(_) => None,
        Err(error) => {
            if refused_by_its_service(&error) {
                return Some(DestinationRejection::Absent);
            }
            unverifiable("an Amazon SNS topic", arn, &error);
            None
        }
    }
}

/// The topic attribute carrying its access control policy, as `GetTopicAttributes` names
/// it.
const TOPIC_POLICY_ATTRIBUTE: &str = "Policy";

/// Why the function `arn` names is not a destination this configuration may be recorded
/// with, or `None` when it is one.
///
/// Both halves are what the real service checks: "in the case of Lambda destinations,
/// Amazon S3 verifies that the Lambda function permissions grant Amazon S3 permission to
/// invoke the function from the Amazon S3 bucket"
/// (`PutBucketNotificationConfiguration`'s own documentation). The permission is a grant on
/// the FUNCTION, so it is read where it lives — the function's own resource-based policy,
/// which Lambda reports through `GetPolicy` — and evaluated by the one IAM evaluator every
/// service of this emulator decides an access request with ([`destination_granted`]). A
/// function carrying no policy at all answers `GetPolicy` with its own refusal, which grants
/// nothing.
///
/// Nothing is written by either call.
async fn function_rejection(
    clients: &mut Clients,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    arn: &str,
    bucket: &str,
) -> Option<DestinationRejection> {
    let answer = clients
        .lambda(callback, region)
        .get_function_configuration()
        .function_name(arn)
        .send()
        .await;
    // The ARN the grant is evaluated on is the one LAMBDA resolved the destination to, so a
    // destination written in any of the forms `FunctionName` admits is judged against the
    // ARN its statements name.
    let resolved = match answer {
        Ok(configuration) => configuration
            .function_arn
            .unwrap_or_else(|| arn.to_string()),
        Err(error) => {
            if refused_by_its_service(&error) {
                return Some(DestinationRejection::Absent);
            }
            unverifiable("a Lambda function", arn, &error);
            return None;
        }
    };
    let answer = clients
        .lambda(callback, region)
        .get_policy()
        .function_name(&resolved)
        .send()
        .await;
    let document = match answer {
        Ok(policy) => policy.policy.unwrap_or_default(),
        Err(error) => {
            if refused_by_its_service(&error) {
                // A function with no resource-based policy is reported as having none, which
                // grants nothing.
                return Some(DestinationRejection::Unpermitted);
            }
            unverifiable("a Lambda function", arn, &error);
            return None;
        }
    };
    if destination_granted(&document, INVOKE_FUNCTION_ACTION, &resolved, bucket) {
        return None;
    }
    Some(DestinationRejection::Unpermitted)
}

/// The IAM action S3 needs on a function destination to deliver an event to it.
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The service principal Amazon S3 publishes notifications under — "for Amazon Web Services
/// services, the principal is a domain-style identifier that the service defines, such as
/// s3.amazonaws.com" (`AddPermission$Principal`).
const S3_SERVICE_PRINCIPAL: &str = "s3.amazonaws.com";

/// The condition key a grant narrowed to ONE invoking resource names the source by.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The condition key a grant narrowed to the account that OWNS the invoking resource names
/// it by.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// Whether `document`, the resource policy of one destination, allows the Amazon S3 service
/// principal to perform `action` on `resource` for a notification published by `bucket`.
///
/// The ONE place a destination's permission is decided, for all three destination kinds: S3
/// asks the same question of each — may Amazon S3 publish this bucket's notifications to
/// it? — and only the action and the resource differ ([`SEND_MESSAGE_ACTION`],
/// [`PUBLISH_ACTION`], [`INVOKE_FUNCTION_ACTION`]). The decision is
/// [`::service_common::iam_authorization`]'s, so a document this reads and the same document
/// read by the service that owns it cannot disagree; the two condition keys such a grant is
/// narrowed by travel as the request's context, and are declared as keys this request MODELS
/// because it carries a value for each.
///
/// `aws:SourceAccount` is the account that OWNS the publishing bucket, which here is the one
/// account this emulator serves ([`::engine::emulator_account_id`]) — the same account
/// every bucket's owner is reported as. A deployment where bucket ownership varies reads
/// the owner off the bucket's own record instead, and this is the seam that changes.
fn destination_granted(document: &str, action: &str, resource: &str, bucket: &str) -> bool {
    let principals = [S3_SERVICE_PRINCIPAL.to_string()];
    let context = [
        (SOURCE_ARN_KEY.to_string(), bucket_arn(bucket)),
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
    ];
    let request =
        ::service_common::iam_authorization::Request::new(action, resource, &principals)
            .with_context(&context)
            .with_published_keys(&[SOURCE_ARN_KEY, SOURCE_ACCOUNT_KEY]);
    ::service_common::iam_authorization::resource_decision(document, &request)
        == ::service_common::iam_authorization::Decision::Allowed
}

/// The error codes that are the EMULATOR's own refusal of the call rather than an answer
/// from the service that owns the destination: no service of this emulator serves the
/// endpoint that was called, more than one of them claims it, or the in-process callback
/// chain was cut short. The destination's own service was never reached in any of them.
const TRANSPORT_REFUSALS: &[&str] = &[
    "UnknownService",
    "AmbiguousService",
    "CallbackRecursionLimitExceeded",
];

/// Whether `error` is the service that OWNS the destination refusing the call — the one
/// outcome that fails a destination's validation.
///
/// A failure with no error document behind it (a transport that could not carry the call)
/// and the emulator's own refusal of it ([`TRANSPORT_REFUSALS`]) are not that: nothing
/// judged the destination, which is a different thing from its own service rejecting it.
fn refused_by_its_service<E, R>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<E, R>,
) -> bool
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let Some(code) = error.as_service_error().and_then(ProvideErrorMetadata::code) else {
        return false;
    };
    // The code may carry a shape namespace; the code itself is its last segment.
    let code = code.rsplit_once('#').map_or(code, |(_, name)| name);
    !TRANSPORT_REFUSALS.contains(&code)
}

/// Log one destination the service that owns it gave no verdict on, which is why the
/// configuration naming it is recorded as given.
fn unverifiable<E>(destination: &str, named: &str, error: &E)
where
    E: ::std::error::Error + Send + Sync + 'static,
{
    report(DESTINATION_NOT_RESOLVED, destination, named, error);
}

/// The clients one operation's deliveries are made through, built on first use and then
/// reused: a fan-out to several destinations of one service must not construct a client
/// per destination.
///
/// Each slot remembers the Region its client was built for, because a destination is
/// reached in the Region ITS OWN ARN names: a configuration spanning two Regions of one
/// service rebuilds the client rather than sending the second delivery to the first
/// Region's endpoint.
#[derive(Default)]
struct Clients {
    event_bridge: Option<(String, ::aws_sdk_eventbridge::Client)>,
    sqs: Option<(String, ::aws_sdk_sqs::Client)>,
    sns: Option<(String, ::aws_sdk_sns::Client)>,
    lambda: Option<(String, ::aws_sdk_lambda::Client)>,
}

/// Build each client the same way: over the in-process callback into this same emulator,
/// against the destination service's endpoint in `region`.
///
/// The transport is in-process, so each client disables what a network client needs and
/// this one cannot have: retries, timeouts, stalled-stream protection and identity
/// caching each need a sleep implementation the callback transport is not given, and
/// there is nothing to retry behind it anyway.
macro_rules! callback_client {
    ($field:ident, $sdk:ident, $endpoint:literal) => {
        fn $field(
            &mut self,
            callback: &::smithy_server_runtime::EmbeddedEmulator,
            region: &str,
        ) -> &::$sdk::Client {
            if self.$field.as_ref().is_none_or(|(held, _)| held != region) {
                let config = ::$sdk::Config::builder()
                    .behavior_version(::$sdk::config::BehaviorVersion::latest())
                    .http_client(callback.http_client())
                    .credentials_provider(callback.aws_credentials_provider())
                    .region(::$sdk::config::Region::new(region.to_string()))
                    .endpoint_url(format!("https://{}.{region}.amazonaws.com", $endpoint))
                    .retry_config(::$sdk::config::retry::RetryConfig::disabled())
                    .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
                    .identity_cache(::$sdk::config::IdentityCache::no_cache())
                    .stalled_stream_protection(
                        ::$sdk::config::StalledStreamProtectionConfig::disabled(),
                    )
                    .build();
                self.$field = Some((region.to_string(), ::$sdk::Client::from_conf(config)));
            }
            &self
                .$field
                .as_ref()
                .expect("the client for this region was just built")
                .1
        }
    };
}

impl Clients {
    // EventBridge publishes its endpoint and signs under `events`.
    callback_client!(event_bridge, aws_sdk_eventbridge, "events");
    callback_client!(sqs, aws_sdk_sqs, "sqs");
    callback_client!(sns, aws_sdk_sns, "sns");
    callback_client!(lambda, aws_sdk_lambda, "lambda");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every event value this emitter publishes is one of the model's own `Event`
    /// values, so a configuration can actually name it.
    #[test]
    fn every_published_event_is_a_modelled_event_value() {
        let kinds = [
            ObjectEventKind::CreatedByPut,
            ObjectEventKind::CreatedByCopy,
            ObjectEventKind::CreatedByMultipartUpload,
            ObjectEventKind::Removed,
            ObjectEventKind::DeleteMarkerCreated,
            ObjectEventKind::TaggingPut,
            ObjectEventKind::TaggingDelete,
            ObjectEventKind::AclPut,
            ObjectEventKind::RetentionPut,
            ObjectEventKind::AnnotationPut,
            ObjectEventKind::AnnotationDelete,
            ObjectEventKind::RestoreInitiated,
        ];
        for kind in kinds {
            let value = kind.event_value();
            assert!(
                super::super::EVENT_VALUES.contains(&value),
                "case {kind:?}: {value:?} is not one of the model's Event values"
            );
            assert!(
                value.starts_with("s3:") && kind.record_event_name() == &value["s3:".len()..],
                "case {kind:?}: expected the record name to be {value:?} without its \
                 prefix, got {:?}",
                kind.record_event_name()
            );
        }
    }

    /// The wildcard form spans every event of ONE type and nothing else.
    #[test]
    fn a_configured_event_matches_its_own_type_only() {
        // (case, configured, actual, matches)
        let rows: &[(&str, &str, &str, bool)] = &[
            ("the same value", "s3:ObjectCreated:Put", "s3:ObjectCreated:Put", true),
            ("another detail of the type", "s3:ObjectCreated:Put", "s3:ObjectCreated:Copy", false),
            ("the type wildcard", "s3:ObjectCreated:*", "s3:ObjectCreated:Copy", true),
            (
                "the type wildcard over a long detail",
                "s3:ObjectCreated:*",
                "s3:ObjectCreated:CompleteMultipartUpload",
                true,
            ),
            ("the wildcard of another type", "s3:ObjectRemoved:*", "s3:ObjectCreated:Put", false),
            ("the removal wildcard", "s3:ObjectRemoved:*", "s3:ObjectRemoved:DeleteMarkerCreated", true),
            ("the tagging wildcard", "s3:ObjectTagging:*", "s3:ObjectTagging:Delete", true),
            (
                "a detail-less value against the wildcard of a type it is not",
                "s3:ObjectCreated:*",
                "s3:ReducedRedundancyLostObject",
                false,
            ),
            (
                "a value whose type is a prefix of another type",
                "s3:ObjectRestore:*",
                "s3:ObjectRestoreX:Post",
                false,
            ),
        ];
        for (case, configured, actual, expected) in rows {
            let matched = event_matches(configured, actual);
            assert_eq!(
                matched, *expected,
                "case {case:?}: expected {configured:?} against {actual:?} to be \
                 {expected}, got {matched}"
            );
        }
    }

    /// Every filter rule a configuration carries has to hold, and a configuration with
    /// no filter restricts nothing.
    #[test]
    fn a_key_passes_a_filter_only_when_every_rule_holds() {
        fn rule(name: &str, value: &str) -> super::super::NotificationFilterRule {
            super::super::NotificationFilterRule {
                name: Some(name.to_string()),
                value: Some(value.to_string()),
            }
        }
        // (case, rules, key, passes)
        let rows: Vec<(&str, Option<Vec<super::super::NotificationFilterRule>>, &str, bool)> = vec![
            ("no filter at all", None, "anything/at/all.txt", true),
            ("an empty rule list", Some(Vec::new()), "anything.txt", true),
            ("a prefix that holds", Some(vec![rule("prefix", "input/")]), "input/a.json", true),
            ("a prefix that does not", Some(vec![rule("prefix", "input/")]), "output/a.json", false),
            ("a suffix that holds", Some(vec![rule("suffix", ".json")]), "input/a.json", true),
            ("a suffix that does not", Some(vec![rule("suffix", ".json")]), "input/a.csv", false),
            (
                "both rules holding",
                Some(vec![rule("prefix", "input/"), rule("suffix", ".json")]),
                "input/a.json",
                true,
            ),
            (
                "one of two rules failing",
                Some(vec![rule("prefix", "input/"), rule("suffix", ".json")]),
                "input/a.csv",
                false,
            ),
            (
                "a rule naming no value",
                Some(vec![super::super::NotificationFilterRule {
                    name: Some("prefix".to_string()),
                    value: None,
                }]),
                "anything.txt",
                true,
            ),
        ];
        for (case, rules, key, expected) in rows {
            let passed = key_matches(rules.as_ref(), key);
            assert_eq!(
                passed, expected,
                "case {case:?}: expected {key:?} to pass {rules:?} = {expected}, got \
                 {passed}"
            );
        }
    }

    /// A published key is percent-encoded, with a space as a plus and the path
    /// separator left alone.
    #[test]
    fn a_published_key_is_url_encoded() {
        // (case, key, published)
        let rows: &[(&str, &str, &str)] = &[
            ("a plain key", "report.json", "report.json"),
            ("a prefixed key", "input/2026/report.json", "input/2026/report.json"),
            ("a key with a space", "red flower.jpg", "red+flower.jpg"),
            ("a key with parentheses", "test_file(3).png", "test_file%283%29.png"),
            ("a non-ASCII key", "café.txt", "caf%C3%A9.txt"),
        ];
        for (case, key, expected) in rows {
            let published = record_key(key);
            assert_eq!(
                published, *expected,
                "case {case:?}: expected {key:?} to publish as {expected:?}, got \
                 {published:?}"
            );
        }
    }

    /// A sequencer orders two changes of one key the way their instants order.
    #[test]
    fn a_sequencer_increases_with_the_instant() {
        let earlier = ObjectEvent {
            at: SystemTime::UNIX_EPOCH + ::std::time::Duration::from_nanos(1),
            ..ObjectEvent::new(ObjectEventKind::CreatedByPut, "k")
        };
        let later = ObjectEvent {
            at: SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1),
            ..ObjectEvent::new(ObjectEventKind::CreatedByPut, "k")
        };
        let (first, second) = (earlier.sequencer(), later.sequencer());
        assert!(
            first < second,
            "case \"ordering\": expected {first:?} to sort before {second:?}"
        );
        assert_eq!(
            first.len(),
            second.len(),
            "case \"width\": expected one width, got {first:?} and {second:?}"
        );
    }

    /// A queue ARN resolves to the url SQS addresses it by; anything that is not a queue
    /// ARN resolves to nothing.
    #[test]
    fn a_queue_arn_resolves_to_the_url_sqs_addresses_it_by() {
        // (case, arn, url)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a queue of the account",
                "arn:aws:sqs:us-east-1:000000000000:object-events",
                Some("https://sqs.us-east-1.amazonaws.com/000000000000/object-events"),
            ),
            (
                "a queue in another Region",
                "arn:aws:sqs:eu-west-1:000000000000:object-events",
                Some("https://sqs.eu-west-1.amazonaws.com/000000000000/object-events"),
            ),
            (
                "a resource field that names no queue",
                "arn:aws:sqs:us-east-1:000000000000:queue/object-events",
                None,
            ),
            ("not an ARN at all", "object-events", None),
        ];
        for (case, arn, expected) in rows {
            let url = queue_url(arn);
            assert_eq!(
                url.as_deref(),
                *expected,
                "case {case:?}: expected {arn:?} to resolve to {expected:?}, got {url:?}"
            );
        }
    }

    /// Both documents name the CALL the event came of, by the pair that call's own
    /// response carried — and report an ETag WITHOUT the quotes the store holds it in.
    #[test]
    fn both_documents_name_the_call_the_event_came_of() {
        let correlation = RequestCorrelation {
            request_id: "0123456789ABCDEF".to_string(),
            extended_request_id: "a".repeat(64),
        };
        let event = ObjectEvent {
            size: Some(11),
            etag: Some("\"d41d8cd98f00b204e9800998ecf8427e\"".to_string()),
            ..ObjectEvent::new(ObjectEventKind::CreatedByPut, "input/report.json")
        };
        let target = NotificationTarget {
            id: Some("queue-configuration".to_string()),
            arn: "arn:aws:sqs:us-east-1:000000000000:object-events".to_string(),
            events: vec!["s3:ObjectCreated:*".to_string()],
            filter_rules: None,
        };

        let envelope =
            records_envelope("bucket", "us-east-1", &target, &event, &correlation, "198.51.100.7");
        let record = &envelope["Records"][0];
        assert_eq!(
            record["responseElements"]["x-amz-request-id"],
            ::serde_json::json!(correlation.request_id),
            "case \"the records envelope\": expected the call's own request id, got {:?}",
            record["responseElements"]["x-amz-request-id"]
        );
        assert_eq!(
            record["responseElements"]["x-amz-id-2"],
            ::serde_json::json!(correlation.extended_request_id),
            "case \"the records envelope\": expected the call's own extended id, got {:?}",
            record["responseElements"]["x-amz-id-2"]
        );
        assert_eq!(
            record["s3"]["object"]["eTag"],
            ::serde_json::json!("d41d8cd98f00b204e9800998ecf8427e"),
            "case \"the records envelope\": expected an unquoted eTag, got {:?}",
            record["s3"]["object"]["eTag"]
        );

        assert_eq!(
            record["requestParameters"]["sourceIPAddress"],
            ::serde_json::json!("198.51.100.7"),
            "case \"the records envelope\": expected the address the call was made from, got \
             {:?}",
            record["requestParameters"]["sourceIPAddress"]
        );

        let detail = event_bridge_detail("bucket", &event, &correlation, "198.51.100.7");
        assert_eq!(
            detail["source-ip-address"],
            ::serde_json::json!("198.51.100.7"),
            "case \"the EventBridge detail\": expected the address the call was made from, got \
             {:?}",
            detail["source-ip-address"]
        );
        assert_eq!(
            detail["request-id"],
            ::serde_json::json!(correlation.request_id),
            "case \"the EventBridge detail\": expected the call's own request id, got {:?}",
            detail["request-id"]
        );
        assert_eq!(
            detail["object"]["etag"],
            ::serde_json::json!("d41d8cd98f00b204e9800998ecf8427e"),
            "case \"the EventBridge detail\": expected an unquoted etag, got {:?}",
            detail["object"]["etag"]
        );
    }
}
