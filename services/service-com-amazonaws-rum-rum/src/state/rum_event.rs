// SPDX-License-Identifier: CC0-1.0

//! The RUM event document: the ONE form a delivered `RumEvent` is held in, reported as and
//! copied as.
//!
//! `PutRumEvents` accepts the CLIENT's structure — a `RumEvent` plus the batch's
//! `BatchId`, `AppMonitorDetails` and `UserDetails` — while what a reader gets back is the
//! SERVICE's own document: that event ENRICHED, at ingestion, with the batch's context and
//! with what only the service knows (the app monitor the batch was delivered to, the
//! account that owns it, where the caller is), under the field names
//! RUM publishes for a RUM event.
//!
//! Those names are the model's own. `MetricDefinitionRequest` enumerates the first-level
//! keys a custom metric's `ValueKey`, `EventPattern` and `DimensionKeys` may name in a RUM
//! event ([`super::RUM_EVENT_FIELDS`]) — the account, the application, the batch, the
//! event's own id, instant, type, version, interaction and details, the log stream, the
//! metadata and the user session — and an `EventPattern` is matched against the very
//! document a read reports, so the client's `{id, timestamp, type, metadata, details}` is
//! NOT the form anything downstream sees. The enrichment therefore happens ONCE, here, and
//! the monitor's buffer, the query that reads it back
//! ([`crate::ops::get_app_monitor_data`]) and the copy stored in CloudWatch Logs
//! ([`super::cw_log_delivery`]) all carry the same bytes. The one field whose spelling the
//! wire and that list disagree on is the application's id: the document carries it as
//! `application_id`, which is what the recorded wire document spells, while the list keeps
//! the `application_Id` the documentation of those metric members spells — one is the
//! event's field name, the other is what a metric definition is judged against.
//!
//! Recorded behaviour pins the shape of the fields the model leaves open:
//!
//! * `event_timestamp` is an epoch instant in MILLISECONDS, which is the unit
//!   `TimeRange`'s bounds are expressed in as well, so a query and an event are placed on
//!   ONE scale ([`event_timestamp_millis`], [`event_instant_millis`]);
//! * `log_stream` carries an INSTANT rather than a name — the `{seconds, nanos}` structure
//!   [`instant_document`] writes, holding the instant of the event itself — so the name of
//!   the CloudWatch Logs stream a batch's copies accumulate in
//!   ([`super::cw_log_delivery::delivery_stream_name`]) is no part of the document;
//! * `metadata` is the document the client serialized it as, MERGED with the attributes the
//!   batch declared for every one of its events ([`shared_attributes`]) and
//!   ENRICHED with where the
//!   caller is ([`super::caller_location`]), with the `metadata_keys` / `metadata_values`
//!   projections of it beside it: the keys are its first-level member names, and each value
//!   entry is that member as the `<key>=<value>` text [`metadata_projections`] writes, so a
//!   consumer reading the two lists positionally recovers the pairs;
//! * `event_interaction` is the IDENTIFIER of the interaction the event belongs to
//!   ([`event_interaction`]), not the interaction counter a client's payload carries beside
//!   it;
//! * the document's `userId` and `sessionId` are reported BOTH at the top level and inside
//!   `user_details`, which is why the model's field list carries all three.

use ::serde_json::{Map, Value};

use crate::ops::types::RumEvent;

/// The field a RUM event document carries the instant the event occurred under, as an
/// epoch instant in milliseconds — one spelling for the data-plane write, the query that
/// selects on it and the copy stored in CloudWatch Logs.
pub const EVENT_TIMESTAMP_FIELD: &str = "event_timestamp";

/// The field carrying the client's own metadata document, which is where a RUM web client
/// reports the identity of the user session (`RumEvent$metadata`: "a JSON serialization of
/// the identity of the user for this session").
pub const METADATA_FIELD: &str = "metadata";

/// The field carrying the first-level KEYS of [`METADATA_FIELD`], in the order the
/// document holds them.
pub const METADATA_KEYS_FIELD: &str = "metadata_keys";

/// The field carrying the first-level VALUES of [`METADATA_FIELD`], positionally aligned
/// with [`METADATA_KEYS_FIELD`].
pub const METADATA_VALUES_FIELD: &str = "metadata_values";

/// The field carrying the instant of the event, as the `{seconds, nanos}` structure
/// [`instant_document`] writes.
pub const LOG_STREAM_FIELD: &str = "log_stream";

/// The field carrying the identifier of the interaction the event belongs to
/// ([`event_interaction`]).
pub const EVENT_INTERACTION_FIELD: &str = "event_interaction";

/// The member an instant carries its whole seconds since the epoch under
/// ([`instant_document`]).
const INSTANT_SECONDS_MEMBER: &str = "seconds";

/// The member an instant carries the nanoseconds WITHIN that second under.
const INSTANT_NANOS_MEMBER: &str = "nanos";

/// What one [`METADATA_VALUES_FIELD`] entry joins its member's name to its value with.
const METADATA_PAIR_SEPARATOR: &str = "=";

/// How many milliseconds one second is, for the instant codec below.
const MILLIS_PER_SECOND: i64 = 1_000;

/// How many nanoseconds one millisecond is: what the subsecond part of an
/// [`::aws_smithy_types::DateTime`] has to be divided by to reach the same unit.
const NANOS_PER_MILLI: u32 = 1_000_000;

/// The member a client's metadata (or event details) reports the schema version of the
/// event under, which the document promotes to its own `event_version`: the RUM web client
/// stamps every event's payload with the version of the schema it serialized it under.
const CLIENT_VERSION_MEMBER: &str = "version";

/// The member a client's event details (or its session metadata) reports the IDENTIFIER of
/// the interaction the event belongs to under, which the document promotes to its own
/// `event_interaction` ([`event_interaction`]).
///
/// It is the identifier and not the `interaction` counter a RUM web client stamps its
/// payload with beside it: the recorded document reports the interaction as a text naming
/// it, while the counter is a number saying how many interactions of the session precede
/// it.
const CLIENT_INTERACTION_ID_MEMBER: &str = "pageInteractionId";

/// The member reporting the identifier of the interaction THAT interaction descends from,
/// which a RUM web client reports beside it for an interaction reached from another one.
const CLIENT_PARENT_INTERACTION_ID_MEMBER: &str = "parentPageInteractionId";

/// What [`event_interaction`] joins an event's interaction identifier to the identifier of
/// the interaction it descends from with: the recorded value carries the separator even for
/// an event that descends from no interaction, where nothing follows it.
const INTERACTION_SEPARATOR: &str = " &";

/// The bytes of an interaction identifier [`event_interaction`] leaves as they are: RFC
/// 3986's unreserved set (`ALPHA` / `DIGIT` / `-` / `.` / `_` / `~`). Every other byte is
/// percent-encoded, which is what the recorded value does to the `/` a page's interaction
/// identifier starts with.
const INTERACTION_UNRESERVED: &::percent_encoding::AsciiSet =
    &::percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');

/// `RumEvent$timestamp` as a RUM event document carries it: the instant in MILLISECONDS
/// since the epoch, which is the unit `TimeRange$After`/`$Before` bound an event at.
///
/// An instant whose millisecond count no `i64` can hold saturates at that type's bound
/// rather than wrapping onto an instant on the other side of the epoch, so the ordering of
/// two delivered instants is never inverted by the codec.
pub fn event_timestamp_millis(instant: &::aws_smithy_types::DateTime) -> i64 {
    let within_the_second = i64::from(instant.subsec_nanos() / NANOS_PER_MILLI);
    instant
        .secs()
        .saturating_mul(MILLIS_PER_SECOND)
        .saturating_add(within_the_second)
}

/// The instant a RUM event `document` carries, in milliseconds since the epoch, or `None`
/// for a document whose [`EVENT_TIMESTAMP_FIELD`] is missing or is no instant at all.
///
/// The one reading of what [`event_timestamp_millis`] writes, so a query cannot place an
/// event at an instant other than the one it was delivered at. A fractional count of
/// milliseconds reads as the whole millisecond it falls in — the second BELOW the value,
/// for an instant before the epoch as well as after it — because that is the instant such a
/// document names.
pub fn event_instant_millis(document: &Value) -> Option<i64> {
    let carried = document.get(EVENT_TIMESTAMP_FIELD)?;
    carried
        .as_i64()
        .or_else(|| carried.as_f64().map(whole_millis))
}

/// A fractional count of milliseconds since the epoch as the whole millisecond it falls
/// in. A float-to-integer cast saturates at the integer's own bounds, so a value no `i64`
/// can hold lands on the nearest instant one can.
fn whole_millis(millis: f64) -> i64 {
    millis.floor() as i64
}

/// What ONE `PutRumEvents` call delivered, beside the events themselves: the context every
/// event of that batch is enriched with.
///
/// The app monitor is named from the STORED record the batch's `Id` resolved to — the
/// monitor that actually accepted the batch — rather than from the client's
/// `AppMonitorDetails`, which is the sending web client's own view and names whatever it
/// was configured with. The one member of that structure no server-side value can stand
/// for is the application's own version, which only the client knows.
#[derive(Debug, Clone, Copy)]
pub struct Delivery<'a> {
    /// The `Id` of the app monitor the batch was delivered to, as the stored record holds
    /// it: the document's `application_id`.
    pub app_monitor_id: &'a str,
    /// The `Name` of that same monitor: the document's `application_name`.
    pub app_monitor_name: &'a str,
    /// `AppMonitorDetails$version`, when the client reported one: the document's
    /// `application_version`.
    pub app_monitor_version: Option<&'a str>,
    /// `PutRumEventsRequest$BatchId` of the call that delivered the event.
    pub batch_id: &'a str,
    /// `UserDetails$userId` of the session the batch was collected from.
    pub user_id: Option<&'a str>,
    /// `UserDetails$sessionId` of that session.
    pub session_id: Option<&'a str>,
    /// The attributes `PutRumEventsRequest$SessionMetadata` declared for EVERY event of the
    /// batch, sent once for the batch rather than once per event, which each event's own
    /// `metadata` is merged with ([`reported_metadata`]).
    ///
    /// Resolved ONCE per call, by [`shared_attributes`], rather than per event: the
    /// document is the same for every event of the batch, and the bound that caps what it
    /// costs is applied there.
    pub shared_attributes: Option<&'a Map<String, Value>>,
}

/// The most one batch's declared attributes may add to the documents of the call that
/// declared them.
///
/// Those attributes are COPIED into each of the batch's events' stored documents
/// ([`reported_metadata`]), so what one accepted call costs is the declared size TIMES the
/// number of events it carries — and a caller picks both numbers, so what is bounded here is
/// their PRODUCT rather than either factor: a call whose copies stay inside this ceiling has
/// its attributes merged, and one past it has them merged into nothing, so one accepted call
/// adds at most this many bytes however large a document it declares and however many events
/// it carries. The attributes RUM describes a batch as declaring (the browser, the device and
/// the operating system of the end user) are a few hundred bytes, so an ordinary batch of any
/// size is well inside it; the number is this emulator's own bound and no modelled
/// constraint, so a call past it is still accepted and every event keeps its own metadata,
/// which is the same direction as an oversized event copy ([`super::cw_log_delivery`]).
pub const MAX_SHARED_ATTRIBUTE_BYTES_PER_BATCH: usize = 4 * 1024 * 1024;

/// The attributes one batch declared for every event it carries
/// (`PutRumEventsRequest$SessionMetadata`), as the members to merge into each of those
/// events' own `metadata` — resolved once for the call, before any of its documents are
/// built, from the declared document and the number of events `events` it would be copied
/// into.
///
/// `None` for a batch that declared none, one whose document is no JSON OBJECT (such a
/// document declares no first-level attributes), and one whose copies would pass
/// [`MAX_SHARED_ATTRIBUTE_BYTES_PER_BATCH`]. That bound is checked on the TEXT and on the
/// event count, BEFORE the document is parsed and before any copy is made, so neither the
/// parse nor the copies are work a caller can choose the size of.
pub fn shared_attributes(declared: Option<&str>, events: usize) -> Option<Map<String, Value>> {
    let declared = declared?;
    let copied = declared.len().saturating_mul(events);
    if copied > MAX_SHARED_ATTRIBUTE_BYTES_PER_BATCH {
        ::tracing::warn!(
            bytes = declared.len(),
            events,
            copied,
            limit = MAX_SHARED_ATTRIBUTE_BYTES_PER_BATCH,
            "the attributes declared for this batch would be copied into its events past \
             what this emulator merges, so they are not merged"
        );
        return None;
    }
    match json_member(declared) {
        Value::Object(members) => Some(members),
        _ => None,
    }
}

/// One delivered `RumEvent` as the RUM event document the service holds, reports and
/// copies it as.
///
/// Every field the service can fill from the delivery and its own state is always carried;
/// a field that can only come from an OPTIONAL member of the batch
/// (`application_version`, `userId`, `sessionId`) is carried when the batch carried it,
/// since nothing is reported that the client did not send. `metadata` and `details` are
/// `@mediaType("application/json")` strings, so each is re-parsed into the document rather
/// than nested as escaped text; a value that is not valid JSON is kept verbatim as a
/// string, since the model constrains neither.
pub fn event_document(delivery: &Delivery<'_>, event: &RumEvent) -> String {
    let metadata = reported_metadata(event.metadata.as_deref(), delivery.shared_attributes);
    let details = json_member(&event.details);

    // The fields go in in the order RUM reports them in, which is the order a recorded
    // document carries them in: the session, then the application, then the event, then
    // the documents the client sent, then the account and the interaction.
    let mut document = Map::new();
    let mut user = Map::new();
    for (field, carried) in [("userId", delivery.user_id), ("sessionId", delivery.session_id)] {
        if let Some(carried) = carried {
            document.insert(field.to_string(), Value::String(carried.to_string()));
            user.insert(field.to_string(), Value::String(carried.to_string()));
        }
    }
    document.insert(
        "application_id".to_string(),
        Value::String(delivery.app_monitor_id.to_string()),
    );
    if let Some(version) = delivery.app_monitor_version {
        document.insert(
            "application_version".to_string(),
            Value::String(version.to_string()),
        );
    }
    document.insert(
        "application_name".to_string(),
        Value::String(delivery.app_monitor_name.to_string()),
    );
    document.insert("event_id".to_string(), Value::String(event.id.clone()));
    document.insert(
        EVENT_TIMESTAMP_FIELD.to_string(),
        Value::from(event_timestamp_millis(&event.timestamp)),
    );
    document.insert(
        super::EVENT_TYPE_FIELD.to_string(),
        Value::String(event.r#type.clone()),
    );
    // The schema version is the event's own, as the client stamped its payload with it:
    // the details first, since they describe THIS event, and the session's metadata after
    // it.
    if let Some(version) = client_member(&details, &metadata, CLIENT_VERSION_MEMBER) {
        document.insert("event_version".to_string(), version);
    }
    document.insert(
        LOG_STREAM_FIELD.to_string(),
        instant_document(&event.timestamp),
    );
    document.insert(
        "batch_id".to_string(),
        Value::String(delivery.batch_id.to_string()),
    );
    let (keys, values) = metadata_projections(&metadata);
    document.insert(METADATA_FIELD.to_string(), metadata.clone());
    document.insert(METADATA_KEYS_FIELD.to_string(), Value::Array(keys));
    document.insert(METADATA_VALUES_FIELD.to_string(), Value::Array(values));
    document.insert(super::EVENT_DETAILS_FIELD.to_string(), details.clone());
    document.insert("user_details".to_string(), Value::Object(user));
    document.insert(
        "account_id".to_string(),
        Value::String(::engine::emulator_account_id().to_string()),
    );
    document.insert(
        EVENT_INTERACTION_FIELD.to_string(),
        Value::String(event_interaction(&details, &metadata)),
    );
    Value::Object(document).to_string()
}

/// One instant as a RUM event document carries it: the whole `seconds` since the epoch and
/// the `nanos` within that second.
///
/// The two members are the one instant split, never rounded: an instant with a subsecond
/// part reports it in `nanos`, and an instant before the epoch reports the second it falls
/// IN (the one below it) with the nanoseconds from that second's start, which is the
/// invariant an [`::aws_smithy_types::DateTime`] holds its own halves under.
fn instant_document(instant: &::aws_smithy_types::DateTime) -> Value {
    let mut carried = Map::new();
    carried.insert(
        INSTANT_SECONDS_MEMBER.to_string(),
        Value::from(instant.secs()),
    );
    carried.insert(
        INSTANT_NANOS_MEMBER.to_string(),
        Value::from(instant.subsec_nanos()),
    );
    Value::Object(carried)
}

/// The event's `metadata` as the SERVICE reports it: the document the client serialized for
/// THIS event, merged with the attributes the batch declared for every one of its events
/// (`PutRumEventsRequest$SessionMetadata`) and with where the caller is
/// ([`super::caller_location`]).
///
/// One rule settles all three sources, member by member: an attribute the event's own
/// document carries is reported exactly as it arrived, and every other source only FILLS IN
/// a member the event did not report. So a batch's attributes reach every event of that
/// batch — they are sent once for the batch instead of once per event — while an attribute
/// spelled in both places is the event's, and the geolocation (RUM's own addition, which no
/// client serializes) is added to the metadata of EVERY event, including one whose batch and
/// event carried no metadata at all, whose document is then the location alone. `metadata`
/// stays the document the client sent ("a JSON serialization of the identity of the user for
/// this session"): the service fills in what the event did not report rather than re-writing
/// what it did. A document that is not a JSON OBJECT declares no attributes — so metadata
/// the client did not serialize as one has no members to add any to and is reported exactly
/// as it arrived, and a batch whose declared document is not one ([`shared_attributes`])
/// has no attributes to merge into anything.
fn reported_metadata(serialized: Option<&str>, shared: Option<&Map<String, Value>>) -> Value {
    let mut carried = serialized
        .map(json_member)
        .unwrap_or_else(|| Value::Object(Map::new()));
    if let Some(members) = carried.as_object_mut() {
        if let Some(declared) = shared {
            for (member, value) in declared {
                members.entry(member.clone()).or_insert_with(|| value.clone());
            }
        }
        for (member, value) in super::caller_location::OBSERVED_CALLER_LOCATION.members() {
            members
                .entry(member.to_string())
                .or_insert_with(|| Value::String(value.to_string()));
        }
    }
    carried
}

/// The `event_interaction` of one event: the identifier of the interaction it belongs to,
/// percent-encoded, the separator, and the identifier of the interaction that one descends
/// from — which an event reached from no other interaction does not name, so nothing
/// follows the separator for it.
///
/// An event that names no interaction at all belongs to none, so it carries no identifier.
/// Both identifiers are read from the event's own `details` first and from the session's
/// `metadata` after it, the two documents a client stamps its payload in, and each is
/// percent-encoded ([`INTERACTION_UNRESERVED`]) because an interaction is identified by the
/// page it happened on, whose identifier is a path.
fn event_interaction(details: &Value, metadata: &Value) -> String {
    let interaction = client_text(details, metadata, CLIENT_INTERACTION_ID_MEMBER);
    let parent = client_text(details, metadata, CLIENT_PARENT_INTERACTION_ID_MEMBER);
    if interaction.is_none() && parent.is_none() {
        return String::new();
    }
    format!(
        "{}{INTERACTION_SEPARATOR}{}",
        percent_encoded(&interaction.unwrap_or_default()),
        percent_encoded(&parent.unwrap_or_default())
    )
}

/// `text` with every byte outside [`INTERACTION_UNRESERVED`] percent-encoded.
fn percent_encoded(text: &str) -> String {
    ::percent_encoding::utf8_percent_encode(text, INTERACTION_UNRESERVED).to_string()
}

/// The TEXT the client reported for `member` ([`client_member`]), as the document writes a
/// value of any other type.
fn client_text(details: &Value, metadata: &Value, member: &str) -> Option<String> {
    client_member(details, metadata, member).map(|value| projected_text(&value))
}

/// The value the client reported for `member` in this event's `details`, or failing that in
/// its session `metadata` — the two documents a RUM web client stamps its payload in.
fn client_member(details: &Value, metadata: &Value, member: &str) -> Option<Value> {
    [details, metadata]
        .into_iter()
        .filter_map(|document| document.get(member))
        .find(|value| !value.is_null())
        .cloned()
}

/// The `metadata_keys` / `metadata_values` projections of a metadata document: its
/// first-level keys, and the `<key>=<value>` text of the member at each of those positions.
///
/// They are a FLAT pair of lists over one document, so each value is projected as the text
/// it is written as, carrying the member's own name ahead of it so a consumer that reads
/// the two lists positionally recovers the pairs; metadata the client did not serialize as
/// a JSON object projects nothing, since it has no first-level keys to flatten.
fn metadata_projections(metadata: &Value) -> (Vec<Value>, Vec<Value>) {
    let Some(members) = metadata.as_object() else {
        return (Vec::new(), Vec::new());
    };
    members
        .iter()
        .map(|(key, value)| {
            (
                Value::String(key.clone()),
                Value::String(format!(
                    "{key}{METADATA_PAIR_SEPARATOR}{}",
                    projected_text(value)
                )),
            )
        })
        .unzip()
}

/// One metadata value as the text the projection carries it as: a string as itself, and
/// every other JSON value as the text it is written as.
fn projected_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A `JsonValue` member as the document it carries, or as a plain string when it does not
/// parse as JSON.
fn json_member(text: &str) -> Value {
    ::serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}
