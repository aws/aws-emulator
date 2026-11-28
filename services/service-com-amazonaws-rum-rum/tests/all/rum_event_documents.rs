// SPDX-License-Identifier: CC0-1.0

//! The RUM EVENT DOCUMENT a read reports for an event the data plane accepted.
//!
//! The rule under test: an event `PutRumEvents` accepts is reported by
//! `GetAppMonitorData` as the service's own RUM event document — the fields RUM publishes
//! for a RUM event, each computed from the delivered batch, from the app monitor's stored
//! state and from what the service itself knows about the delivery, with the event's
//! instant as `event_timestamp` in MILLISECONDS since the epoch, which is the unit
//! `TimeRange`'s bounds are expressed in. The client's own
//! `{id, timestamp, type, metadata, details}` structure is not what a reader gets back.
//!
//! The fields are the first-level keys the model enumerates for a RUM event
//! (`smithy_query(query="doc", shape="MetricDefinitionRequest")`: "Valid key values for
//! first-level keys in the `ValueKey`, `EventPattern`, and `DimensionKeys` fields" —
//! `account_id`, `application_Id`, `application_version`, `application_name`, `batch_id`,
//! `event_details`, `event_id`, `event_interaction`, `event_timestamp`, `event_type`,
//! `event_version`, `log_stream`, `metadata`, `sessionId`, `user_details`, `userId`), with
//! the application's id spelled `application_id` as the wire document spells it, and the
//! `metadata_keys` / `metadata_values` projections of `metadata` beside it.
//!
//! Four of those fields are the SERVICE's own derivations rather than anything echoed, and
//! each is pinned here and by a test of its own below:
//!
//! * `log_stream` carries the event's INSTANT as `{"seconds": …, "nanos": …}`, not the name
//!   of the CloudWatch Logs stream a batch's copies accumulate in;
//! * `metadata` is the client's document ENRICHED with the members RUM itself SETS at
//!   ingestion, which are reported AFTER every member the client serialized, in RUM's own
//!   order: the `domain` the telemetry is attributed to, and then where the caller is —
//!   `countryCode`, `subdivisionCode` and `localityName`, the members no client serializes
//!   and the first of which the model names as the extended-metric dimension
//!   `"metadata.countryCode": "CountryCode"` (`smithy_query(query="doc",
//!   shape="MetricDefinitionRequest", member="DimensionKeys")`) and as the attribute
//!   `GetAppMonitorData`'s `Country` filter searches;
//! * `metadata_values` carries one `<key>=<value>` text per first-level member of
//!   `metadata`, positionally aligned with `metadata_keys`;
//! * `event_interaction` is the IDENTIFIER of the interaction the event belongs to,
//!   percent-encoded, and not the interaction counter a client's payload carries beside it.
//!
//! The rows span what the rule generalises over: whether the batch carries `metadata` at
//! all, whether that metadata is a JSON object, the TYPES its values have, whether the
//! client stamped a schema version or an interaction on the event (and in which of the two
//! documents, and with or without a parent interaction), whether the batch names a user
//! session, the shape of `details` (an object, a nested one, an array, text that is no JSON
//! at all), and the value of the instant (an ordinary one, one with a subsecond part, one
//! before the epoch).
//!
//! Each row also asserts the unit: the window that covers the event in MILLISECONDS does,
//! and the same instant read as SECONDS covers nothing.

use serde_json::{json, Map, Value};
use service_com_amazonaws_rum_rum::state::rum_event::MAX_SHARED_ATTRIBUTE_BYTES_PER_BATCH;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, create, emulator, ACCOUNT};

/// How many milliseconds one second is: `RumEvent$timestamp` travels as the epoch seconds
/// restJson1 carries a `Timestamp` as, while the document and the query window are in epoch
/// milliseconds.
const MILLIS_PER_SECOND: i64 = 1_000;

/// How many nanoseconds one millisecond is: the unit `log_stream`'s subsecond member is in.
const NANOS_PER_MILLI: i64 = 1_000_000;

/// The members RUM adds to an event's `metadata` for where the caller is, in the order the
/// recorded document carries them.
///
/// Source: the recorded enriched document, whose `metadata` carries these three beside the
/// members the client serialized; the model names the first of them as an extended-metric
/// dimension (`"metadata.countryCode": "CountryCode"`, `smithy_query(query="doc",
/// shape="MetricDefinitionRequest", member="DimensionKeys")`). Their VALUES are the
/// environment's, so no value is stated here: what is asserted is that the service adds
/// them and what KIND of text each is.
const CALLER_LOCATION_MEMBERS: [&str; 3] = ["countryCode", "subdivisionCode", "localityName"];

/// The metadata member carrying the internet domain the telemetry is attributed to: the
/// other member RUM sets at ingestion, reported with the location members after everything
/// the client serialized.
///
/// Source: the recorded enriched document, whose `metadata` carries it after the members the
/// client serialized and ahead of the location members, even for a batch whose client
/// serialized a member of that name itself.
const DOMAIN_MEMBER: &str = "domain";

/// The domain authority every monitor this suite creates is given: the value the service
/// then reports as the `domain` of telemetry whose client reported none itself.
const MONITOR_DOMAIN: &str = "documents.example.com";

/// One delivered event, and the document the read then owes for it.
struct Row {
    case: &'static str,
    /// `PutRumEventsRequest$BatchId`.
    batch_id: &'static str,
    /// `UserDetails$userId` / `UserDetails$sessionId`, for the batches that name them.
    user_id: Option<&'static str>,
    session_id: Option<&'static str>,
    /// `AppMonitorDetails$version`: the client's own view of the application's version.
    version: Option<&'static str>,
    /// `RumEvent$id`.
    event_id: &'static str,
    /// `RumEvent$timestamp` as the wire carries it, and the milliseconds the document must
    /// then report.
    timestamp: Value,
    millis: i64,
    /// `RumEvent$type`.
    event_type: &'static str,
    /// `RumEvent$metadata`, an `@mediaType("application/json")` string, for the events that
    /// carry one.
    metadata: Option<&'static str>,
    /// `RumEvent$details`, the one such document every event carries.
    details: &'static str,
    /// The `event_version` the document must report: the version the client stamped its
    /// payload with, where it stamped one.
    event_version: Option<&'static str>,
    /// The `event_interaction` the document must report: the percent-encoded identifier of
    /// the interaction the event belongs to, the separator, and the identifier of the
    /// interaction it descends from — and no identifier at all for an event that names
    /// none, whatever interaction COUNTER its payload carries.
    interaction: &'static str,
}

impl Row {
    /// The monitor this row's batch is delivered to: one per row, so the window a row asks
    /// for selects its own event alone.
    fn monitor(&self, index: usize) -> String {
        format!("documented-{index}")
    }

    /// This batch as `PutRumEvents` carries it.
    fn batch(&self, monitor: &str, monitor_id: &str) -> Value {
        let mut user_details = json!({});
        if let Some(user_id) = self.user_id {
            user_details["userId"] = json!(user_id);
        }
        if let Some(session_id) = self.session_id {
            user_details["sessionId"] = json!(session_id);
        }
        let mut app_monitor_details = json!({"name": monitor, "id": monitor_id});
        if let Some(version) = self.version {
            app_monitor_details["version"] = json!(version);
        }
        let mut event = json!({
            "id": self.event_id,
            "timestamp": self.timestamp,
            "type": self.event_type,
            "details": self.details,
        });
        if let Some(metadata) = self.metadata {
            event["metadata"] = json!(metadata);
        }
        json!({
            "BatchId": self.batch_id,
            "AppMonitorDetails": app_monitor_details,
            "UserDetails": user_details,
            "RumEvents": [event],
        })
    }

    /// `metadata` as the document reports it: the document the client serialized with the
    /// members RUM sets at ingestion (`service_set`) reported after every member of it
    /// ([`with_service_set_members`], the one copy of that rule in this suite), the
    /// service-set members alone for an event that carried no metadata, and metadata the
    /// client did not serialize as an object exactly as it arrived, since such a document
    /// has no members to report any beside.
    fn reported_metadata(&self, service_set: &Map<String, Value>) -> Value {
        let carried = match self.metadata {
            Some(metadata) => as_json(metadata),
            None => json!({}),
        };
        with_service_set_members(&carried, service_set)
    }

    /// The whole document the read owes for this row's event, for the monitor
    /// `monitor`/`monitor_id` it was delivered to and the members `service_set` the service
    /// set on its metadata at ingestion.
    fn expected_document(
        &self,
        monitor: &str,
        monitor_id: &str,
        service_set: &Map<String, Value>,
    ) -> Value {
        let metadata = self.reported_metadata(service_set);
        let (keys, values) = projections(&metadata);
        let mut document = json!({
            "application_id": monitor_id,
            "application_name": monitor,
            "event_id": self.event_id,
            "event_timestamp": self.millis,
            "event_type": self.event_type,
            "log_stream": instant_of(self.millis),
            "batch_id": self.batch_id,
            "metadata": metadata,
            "metadata_keys": keys,
            "metadata_values": values,
            "event_details": as_json(self.details),
            "user_details": {},
            "account_id": ACCOUNT,
            "event_interaction": self.interaction,
        });
        if let Some(version) = self.version {
            document["application_version"] = json!(version);
        }
        if let Some(version) = self.event_version {
            document["event_version"] = json!(version);
        }
        if let Some(user_id) = self.user_id {
            document["userId"] = json!(user_id);
            document["user_details"]["userId"] = json!(user_id);
        }
        if let Some(session_id) = self.session_id {
            document["sessionId"] = json!(session_id);
            document["user_details"]["sessionId"] = json!(session_id);
        }
        document
    }
}

/// `text` as the JSON document it carries, or as the text itself when it is no JSON at all
/// — which is how a `JsonValue` member the model constrains in no way is reported.
fn as_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

/// The instant `millis` since the epoch as `log_stream` carries it: the whole second it
/// falls in and the nanoseconds within that second, so an instant with a subsecond part
/// keeps it and one before the epoch names the second BELOW it.
fn instant_of(millis: i64) -> Value {
    json!({
        "seconds": millis.div_euclid(MILLIS_PER_SECOND),
        "nanos": millis.rem_euclid(MILLIS_PER_SECOND) * NANOS_PER_MILLI,
    })
}

/// The `metadata_keys` / `metadata_values` projections of a metadata document: its
/// first-level keys in the order it holds them, and the `<key>=<value>` text of the member
/// at each of those positions. Metadata that is no JSON object has no first-level keys, so
/// it projects nothing.
fn projections(metadata: &Value) -> (Vec<Value>, Vec<Value>) {
    metadata
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| (json!(key), json!(format!("{key}={}", value_text(value)))))
                .unzip()
        })
        .unwrap_or_default()
}

/// One metadata value as the text a projection carries it as: a string as itself, every
/// other JSON value as the text it is written as.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The caller location `document`'s metadata reports, asserting the service added all three
/// members and that each is the kind of text it names: an ISO 3166-1 alpha-2 country code
/// (two uppercase letters), a non-empty subdivision code and a non-empty locality name.
fn caller_location(case: &str, document: &Value) -> Map<String, Value> {
    let metadata = document["metadata"].as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected the document's metadata to be an object, got {document}")
    });
    let mut location = Map::new();
    for member in CALLER_LOCATION_MEMBERS {
        let value = metadata.get(member).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected RUM to add {member:?} to the event's metadata, got \
                 {document}"
            )
        });
        let text = value.as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected {member:?} to be a text, got {value}")
        });
        assert!(
            !text.is_empty(),
            "case {case:?}: expected {member:?} to name somewhere, got {text:?}"
        );
        location.insert(member.to_string(), json!(text));
    }
    let country = location["countryCode"].as_str().unwrap_or_default();
    assert!(
        country.len() == 2 && country.chars().all(|letter| letter.is_ascii_uppercase()),
        "case {case:?}: expected countryCode to be an ISO 3166-1 alpha-2 code, got {country:?}"
    );
    location
}

/// The members the service SET on `document`'s metadata at ingestion, in the order a RUM
/// event document reports them: the `domain` the telemetry is attributed to, and then where
/// the caller is ([`caller_location`]).
///
/// `domain` is asserted to be `domain` — the one the monitor the batch was delivered to has
/// authority over, for a batch whose client serialized none itself — while the location's
/// values are the environment's, so for those only the members and the kind of text each is
/// are asserted.
fn service_set_members(case: &str, document: &Value, domain: Option<&str>) -> Map<String, Value> {
    let mut members = Map::new();
    if let Some(domain) = domain {
        assert_eq!(
            document["metadata"][DOMAIN_MEMBER].as_str(),
            Some(domain),
            "case {case:?}: expected the domain the telemetry is attributed to, got {document}"
        );
        members.insert(DOMAIN_MEMBER.to_string(), json!(domain));
    }
    for (member, value) in caller_location(case, document) {
        members.insert(member, value);
    }
    members
}

/// The rows every case below delivers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a page view whose metadata and details are both stamped",
            batch_id: "11111111-2222-4333-8444-555555555555",
            user_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "11111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_067_200_i64),
            millis: 1_704_067_200_000,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some(
                "{\"version\":\"1.0.0\",\"pageId\":\"/home\",\"browserName\":\"Chrome\",\
                 \"interaction\":3}",
            ),
            details: "{\"version\":\"1.0.0\",\"pageId\":\"/home\",\
                      \"pageInteractionId\":\"/home-0\",\"interaction\":0.0}",
            event_version: Some("1.0.0"),
            // The identifier names the interaction; the counters both documents carry are
            // not it.
            interaction: "%2Fhome-0 &",
        },
        Row {
            case: "an event carrying no metadata at all",
            batch_id: "22222222-2222-4333-8444-555555555555",
            user_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("2.3.1"),
            event_id: "22111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_067_260_i64),
            millis: 1_704_067_260_000,
            event_type: "com.amazon.rum.js_error_event",
            metadata: None,
            details: "{\"version\":\"0.9.0\",\"message\":\"boom\"}",
            // The details are the event's own, so a version stamped there is the event's
            // version even when the session's metadata is absent.
            event_version: Some("0.9.0"),
            interaction: "",
        },
        Row {
            case: "metadata whose values are not all strings",
            batch_id: "33333333-2222-4333-8444-555555555555",
            user_id: Some("bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("eeeeeeee-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "33111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_070_800_i64),
            millis: 1_704_070_800_000,
            event_type: "com.amazon.rum.http_event",
            metadata: Some(
                "{\"pageId\":42,\"cached\":true,\"sampleRate\":0.25,\"interaction\":7}",
            ),
            details: "{\"request\":{\"method\":\"GET\",\"status\":200}}",
            event_version: None,
            // An interaction COUNTER names no interaction, so this event belongs to none.
            interaction: "",
        },
        Row {
            case: "a batch that names no session, no user and no version",
            batch_id: "44444444-2222-4333-8444-555555555555",
            user_id: None,
            session_id: None,
            version: None,
            event_id: "44111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_074_400_i64),
            millis: 1_704_074_400_000,
            event_type: "com.amazon.rum.session_start_event",
            metadata: Some("{\"browserName\":\"Edge\"}"),
            details: "{}",
            event_version: None,
            interaction: "",
        },
        Row {
            case: "details that are a JSON array",
            batch_id: "55555555-2222-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "55111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_078_000_i64),
            millis: 1_704_078_000_000,
            event_type: "com.amazon.rum.performance_resource_event",
            metadata: Some("{\"version\":\"1.2.3\"}"),
            details: "[1,2,3]",
            event_version: Some("1.2.3"),
            interaction: "",
        },
        Row {
            case: "metadata and details that are no JSON at all",
            batch_id: "66666666-2222-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "66111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_081_600_i64),
            millis: 1_704_081_600_000,
            event_type: "com.amazon.rum.custom_event",
            metadata: Some("not a document"),
            details: "neither is this",
            event_version: None,
            interaction: "",
        },
        Row {
            case: "an instant carrying a subsecond part, and an interaction with a parent",
            batch_id: "77777777-2222-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "77111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_085_200.75_f64),
            millis: 1_704_085_200_750,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some("{\"pageId\":\"/pricing\"}"),
            details: "{\"pageTitle\":\"Pricing\",\"pageInteractionId\":\"/pricing-2\",\
                      \"parentPageInteractionId\":\"/home-0\"}",
            event_version: None,
            interaction: "%2Fpricing-2 &%2Fhome-0",
        },
        Row {
            case: "an instant before the epoch",
            batch_id: "88888888-2222-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "88111111-1234-4234-8234-123456789012",
            timestamp: json!(-1.5_f64),
            millis: -1_500,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some("{\"pageId\":\"/past\"}"),
            details: "{\"pageTitle\":\"Past\"}",
            event_version: None,
            interaction: "",
        },
        Row {
            case: "an interaction identifier whose text has to be encoded",
            batch_id: "99999999-3333-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "99111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_088_800_i64),
            millis: 1_704_088_800_000,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some("{\"pageId\":\"/search\"}"),
            details: "{\"pageInteractionId\":\"/search?q=a b&c-1\"}",
            event_version: None,
            interaction: "%2Fsearch%3Fq%3Da%20b%26c-1 &",
        },
        Row {
            case: "an interaction only the session's metadata names",
            batch_id: "aaaaaaaa-3333-4333-8444-555555555555",
            user_id: Some("cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            event_id: "aa111111-1234-4234-8234-123456789012",
            timestamp: json!(1_704_092_400_i64),
            millis: 1_704_092_400_000,
            event_type: "com.amazon.rum.page_view_event",
            metadata: Some("{\"pageInteractionId\":\"/cart-3\"}"),
            details: "{\"pageTitle\":\"Cart\"}",
            event_version: None,
            interaction: "%2Fcart-3 &",
        },
    ]
}

/// Deliver one batch, asserting the data plane accepted it.
async fn deliver(router: &MultiServiceRouter, case: &str, monitor_id: &str, body: Value) {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitors/{monitor_id}/"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// The documents `GetAppMonitorData` reports for `monitor` over the window `[after,
/// before]`, both in milliseconds since the epoch, narrowed by `filters` when a case names
/// any.
async fn selected(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    after: i64,
    before: i64,
    filters: Option<Value>,
) -> Vec<Value> {
    let mut body = json!({"TimeRange": {"After": after, "Before": before}});
    if let Some(filters) = filters {
        body["Filters"] = filters;
    }
    let reply = call(
        router,
        "POST",
        &format!("/appmonitor/{monitor}/data"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitorData to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["Events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let text = entry.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a reported event must be a JSON string, got {entry}")
            });
            serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: a reported event must be a document, got {text:?} ({error})")
            })
        })
        .collect()
}

/// The same read with no filters at all.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    after: i64,
    before: i64,
) -> Vec<Value> {
    selected(router, case, monitor, after, before, None).await
}

/// The field names a document carries, sorted — what a missing or an extra field shows up
/// in.
fn fields(document: &Value) -> Vec<String> {
    let mut names: Vec<String> = document
        .as_object()
        .unwrap_or(&Map::new())
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn every_accepted_event_is_reported_as_the_rum_event_document() {
    for (index, row) in rows().iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = row.monitor(index);
        let monitor_id = create(
            &router,
            row.case,
            json!({"Name": &monitor, "Domain": MONITOR_DOMAIN}),
        )
        .await;
        deliver(
            &router,
            row.case,
            &monitor_id,
            row.batch(&monitor, &monitor_id),
        )
        .await;

        // The window that covers the event is the one in MILLISECONDS: the single
        // millisecond the instant falls in, since both bounds are inclusive.
        let documents = reported(&router, row.case, &monitor, row.millis, row.millis).await;
        assert_eq!(
            documents.len(),
            1,
            "case {:?}: the window covering the delivered instant must report the one \
             accepted event, got {documents:?}",
            row.case
        );
        // The members the service set at ingestion: the domain the batch is attributed to,
        // and where the caller is — those members and the kind of text each is are the
        // requirement, while the place itself is the environment's.
        let service_set = if row.metadata == Some("not a document") {
            // Metadata the client did not serialize as an object has no members for the
            // service to report any beside, so this row's document carries none of them.
            Map::new()
        } else {
            service_set_members(row.case, &documents[0], Some(MONITOR_DOMAIN))
        };
        let expected = row.expected_document(&monitor, &monitor_id, &service_set);
        assert_eq!(
            fields(&documents[0]),
            fields(&expected),
            "case {:?}: expected exactly the fields RUM publishes for a RUM event, got {}",
            row.case,
            documents[0]
        );
        assert_eq!(
            documents[0], expected,
            "case {:?}: expected the enriched document {expected}, got {}",
            row.case, documents[0]
        );

        // And the unit is the millisecond: the same instant read as SECONDS names a window
        // a thousandfold away from the event, which covers nothing. Only the rows at
        // positive instants can ask that, since a `Before` of zero is the member's modelled
        // default and stands for now.
        if row.millis > 0 {
            let seconds = row.millis / MILLIS_PER_SECOND;
            let outside = reported(
                &router,
                row.case,
                &monitor,
                seconds - MILLIS_PER_SECOND,
                seconds + MILLIS_PER_SECOND,
            )
            .await;
            assert!(
                outside.is_empty(),
                "case {:?}: a window naming the instant in seconds covers no event, since \
                 the bounds are milliseconds, got {outside:?}",
                row.case
            );
        }

        // Reading reports the same document again: the read changed nothing.
        let again = reported(&router, row.case, &monitor, row.millis, row.millis).await;
        assert_eq!(
            again, documents,
            "case {:?}: expected a second read to report the same document, got {again:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn log_stream_carries_the_events_own_instant() {
    // Each row: the case, `RumEvent$timestamp` as the wire carries it, and the instant in
    // milliseconds the document must then place the event at. The rows span the instant's
    // surface — a whole second, a subsecond part, the epoch itself, an instant before it,
    // and one far in the future — because the instant is what this field carries.
    let rows: [(&str, Value, i64); 5] = [
        ("a whole second", json!(1_704_067_200_i64), 1_704_067_200_000),
        (
            "a subsecond part",
            json!(1_704_067_200.125_f64),
            1_704_067_200_125,
        ),
        ("the epoch itself", json!(0_i64), 0),
        ("an instant before the epoch", json!(-2.25_f64), -2_250),
        (
            "an instant far in the future",
            json!(4_102_444_800_i64),
            4_102_444_800_000,
        ),
    ];

    // The session is named on every batch, so the field cannot be the stream name without
    // the assertion below noticing.
    let session = "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb";
    for (index, (case, timestamp, millis)) in rows.into_iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = format!("instant-{index}");
        let monitor_id = create(
            &router,
            case,
            json!({"Name": &monitor, "Domain": MONITOR_DOMAIN}),
        )
        .await;
        deliver(
            &router,
            case,
            &monitor_id,
            json!({
                "BatchId": "11111111-2222-4333-8444-5555555555ff",
                "AppMonitorDetails": {"name": monitor, "id": monitor_id},
                "UserDetails": {"sessionId": session},
                "RumEvents": [{
                    "id": "aaaa1111-1234-4234-8234-123456789012",
                    "timestamp": timestamp,
                    "type": "com.amazon.rum.page_view_event",
                    "details": "{}",
                }],
            }),
        )
        .await;

        let documents = reported(&router, case, &monitor, millis, millis).await;
        assert_eq!(
            documents.len(),
            1,
            "case {case:?}: expected the one delivered event, got {documents:?}"
        );
        let reported_stream = &documents[0]["log_stream"];
        assert_eq!(
            *reported_stream,
            instant_of(millis),
            "case {case:?}: expected log_stream to carry the event's instant {}, got \
             {reported_stream}",
            instant_of(millis)
        );
        assert_ne!(
            reported_stream.as_str(),
            Some(session),
            "case {case:?}: log_stream is an instant, not the name of the stream the copies \
             accumulate in, got {reported_stream}"
        );
    }
}

#[tokio::test]
async fn every_metadata_member_is_projected_as_a_key_and_a_key_equals_value_text() {
    // Each row: the case and `RumEvent$metadata` as the client serialized it. The rows span
    // the shapes a metadata document takes — several members, values of each JSON type, a
    // value carrying the separator itself, a nested document, an empty document and one
    // that is no object at all — because the projection is over whatever the client sent.
    let rows: [(&str, Option<&str>); 7] = [
        (
            "texts",
            Some("{\"version\":\"1.0.0\",\"pageId\":\"/checkout\",\"title\":\"Checkout\"}"),
        ),
        (
            "values of every JSON type",
            Some(
                "{\"pageId\":42,\"cached\":true,\"sampleRate\":0.25,\"absent\":null,\
                 \"tags\":[\"a\",\"b\"]}",
            ),
        ),
        (
            "a value carrying the separator",
            Some("{\"query\":\"a=b\",\"pageId\":\"/x=y\"}"),
        ),
        (
            "a nested document",
            Some("{\"page\":{\"id\":\"/home\",\"depth\":2}}"),
        ),
        ("an empty document", Some("{}")),
        ("no metadata at all", None),
        ("metadata that is no document", Some("not a document")),
    ];

    for (index, (case, metadata)) in rows.into_iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = format!("projected-{index}");
        let monitor_id = create(
            &router,
            case,
            json!({"Name": &monitor, "Domain": MONITOR_DOMAIN}),
        )
        .await;
        let instant = 1_704_067_200_i64;
        let mut event = json!({
            "id": "bbbb1111-1234-4234-8234-123456789012",
            "timestamp": instant,
            "type": "com.amazon.rum.page_view_event",
            "details": "{}",
        });
        if let Some(metadata) = metadata {
            event["metadata"] = json!(metadata);
        }
        deliver(
            &router,
            case,
            &monitor_id,
            json!({
                "BatchId": "11111111-2222-4333-8444-5555555555ee",
                "AppMonitorDetails": {"name": monitor, "id": monitor_id},
                "UserDetails": {},
                "RumEvents": [event],
            }),
        )
        .await;

        let millis = instant * MILLIS_PER_SECOND;
        let documents = reported(&router, case, &monitor, millis, millis).await;
        assert_eq!(
            documents.len(),
            1,
            "case {case:?}: expected the one delivered event, got {documents:?}"
        );
        let document = &documents[0];
        let keys = document["metadata_keys"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: metadata_keys must be a list, got {document}"))
            .clone();
        let values = document["metadata_values"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: metadata_values must be a list, got {document}")
            })
            .clone();
        assert_eq!(
            keys.len(),
            values.len(),
            "case {case:?}: the two projections are positionally aligned, so they have one \
             length, got {keys:?} and {values:?}"
        );

        match document["metadata"].as_object() {
            None => {
                // Metadata that is no object has no first-level members to project.
                assert!(
                    keys.is_empty() && values.is_empty(),
                    "case {case:?}: metadata that is no document projects nothing, got \
                     {keys:?} and {values:?}"
                );
            }
            Some(members) => {
                let expected_keys: Vec<Value> = members.keys().map(|key| json!(key)).collect();
                assert_eq!(
                    keys, expected_keys,
                    "case {case:?}: expected one key per first-level member of the reported \
                     metadata, got {keys:?}"
                );
                for (position, (key, value)) in members.iter().enumerate() {
                    let expected = format!("{key}={}", value_text(value));
                    assert_eq!(
                        values[position].as_str(),
                        Some(expected.as_str()),
                        "case {case:?}: expected the member at position {position} to be \
                         projected as {expected:?}, got {}",
                        values[position]
                    );
                }
                // The caller's location is a first-level member like any other, so it is
                // projected like any other: the three members RUM adds are in both lists.
                for member in CALLER_LOCATION_MEMBERS {
                    let value = members.get(member).unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: expected RUM to add {member:?} to the event's \
                             metadata, got {document}"
                        )
                    });
                    let pair = format!("{member}={}", value_text(value));
                    assert!(
                        keys.contains(&json!(member)) && values.contains(&json!(pair)),
                        "case {case:?}: expected the location member {member:?} to be \
                         projected as {pair:?}, got {keys:?} and {values:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn the_caller_location_is_added_to_every_event_and_can_be_filtered_on() {
    // One monitor, two events: one whose client sent metadata and one whose client sent
    // none — the service's reading of where the telemetry came from is on both.
    let (router, _engine) = emulator();
    let case = "the caller's location on every event";
    let monitor = "located";
    let monitor_id = create(
        &router,
        case,
        json!({"Name": monitor, "Domain": MONITOR_DOMAIN}),
    )
    .await;
    let instants: [i64; 2] = [1_704_067_200, 1_704_067_260];
    let metadata: [Option<&str>; 2] = [Some("{\"pageId\":\"/home\"}"), None];
    let events: Vec<Value> = instants
        .iter()
        .zip(metadata)
        .enumerate()
        .map(|(step, (instant, metadata))| {
            let mut event = json!({
                "id": format!("cccc111{step}-1234-4234-8234-123456789012"),
                "timestamp": instant,
                "type": "com.amazon.rum.page_view_event",
                "details": "{}",
            });
            if let Some(metadata) = metadata {
                event["metadata"] = json!(metadata);
            }
            event
        })
        .collect();
    deliver(
        &router,
        case,
        &monitor_id,
        json!({
            "BatchId": "11111111-2222-4333-8444-5555555555dd",
            "AppMonitorDetails": {"name": monitor, "id": monitor_id},
            "UserDetails": {},
            "RumEvents": events,
        }),
    )
    .await;

    let after = instants[0] * MILLIS_PER_SECOND;
    let before = instants[1] * MILLIS_PER_SECOND;
    let documents = reported(&router, case, monitor, after, before).await;
    assert_eq!(
        documents.len(),
        instants.len(),
        "case {case:?}: expected one document per delivered event, got {documents:?}"
    );
    let location = caller_location(case, &documents[0]);
    for (step, document) in documents.iter().enumerate() {
        let step_case = format!("{case}, event {step}");
        assert_eq!(
            caller_location(&step_case, document),
            location,
            "case {step_case:?}: every event of one delivery is reported from one place, got \
             {document}"
        );
    }

    // And the location is a member of the event, so the modelled `Country` filter — "Valid
    // values for `Name` are `Browser` | `Device` | `Country` | `Page` | `OS` | `EventType`
    // | `Invert`" (`smithy_query(query="doc", shape="QueryFilter", member="Name")`) —
    // selects on it: the country the events came from reports all of them, and any other
    // country reports none.
    let country = location["countryCode"].as_str().unwrap_or_default();
    let selected_here = selected(
        &router,
        case,
        monitor,
        after,
        before,
        Some(json!([{"Name": "Country", "Values": [country]}])),
    )
    .await;
    assert_eq!(
        selected_here.len(),
        instants.len(),
        "case {case:?}: a Country filter naming {country:?} must report every event, got \
         {selected_here:?}"
    );
    let elsewhere = if country == "JP" { "DE" } else { "JP" };
    let selected_elsewhere = selected(
        &router,
        case,
        monitor,
        after,
        before,
        Some(json!([{"Name": "Country", "Values": [elsewhere]}])),
    )
    .await;
    assert!(
        selected_elsewhere.is_empty(),
        "case {case:?}: a Country filter naming {elsewhere:?} matches no event delivered \
         from {country:?}, got {selected_elsewhere:?}"
    );
}

#[tokio::test]
async fn a_location_member_the_client_reported_is_reported_as_it_arrived() {
    // One batch, two events: the first names a country and a locality of its own, the
    // second names no metadata at all. `metadata` is the client's own document, so the
    // first event keeps what it sent and is only ADDED the member it left out, while the
    // second carries the service's reading of all three.
    let (router, _engine) = emulator();
    let case = "a client that reported a location of its own";
    let monitor = "client-located";
    let monitor_id = create(
        &router,
        case,
        json!({"Name": monitor, "Domain": MONITOR_DOMAIN}),
    )
    .await;
    let claimed_country = "ZW";
    let claimed_locality = "Nowhere";
    let instants: [i64; 2] = [1_704_067_200, 1_704_067_260];
    deliver(
        &router,
        case,
        &monitor_id,
        json!({
            "BatchId": "11111111-2222-4333-8444-5555555555cc",
            "AppMonitorDetails": {"name": monitor, "id": monitor_id},
            "UserDetails": {},
            "RumEvents": [
                {
                    "id": "dddd1110-1234-4234-8234-123456789012",
                    "timestamp": instants[0],
                    "type": "com.amazon.rum.page_view_event",
                    "metadata": format!(
                        "{{\"countryCode\":\"{claimed_country}\",\
                          \"localityName\":\"{claimed_locality}\"}}"
                    ),
                    "details": "{}",
                },
                {
                    "id": "dddd1111-1234-4234-8234-123456789012",
                    "timestamp": instants[1],
                    "type": "com.amazon.rum.page_view_event",
                    "details": "{}",
                },
            ],
        }),
    )
    .await;

    let documents = reported(
        &router,
        case,
        monitor,
        instants[0] * MILLIS_PER_SECOND,
        instants[1] * MILLIS_PER_SECOND,
    )
    .await;
    assert_eq!(
        documents.len(),
        2,
        "case {case:?}: expected one document per delivered event, got {documents:?}"
    );
    // What the service reports where the client reported nothing.
    let observed = caller_location(case, &documents[1]);
    // The event that reported two of the members: those two are its own, the third is the
    // service's, and all three are projected like any other member.
    let claimed = caller_location(case, &documents[0]);
    assert_eq!(
        claimed["countryCode"].as_str(),
        Some(claimed_country),
        "case {case:?}: expected the country the client reported, got {}",
        documents[0]
    );
    assert_eq!(
        claimed["localityName"].as_str(),
        Some(claimed_locality),
        "case {case:?}: expected the locality the client reported, got {}",
        documents[0]
    );
    assert_eq!(
        claimed["subdivisionCode"], observed["subdivisionCode"],
        "case {case:?}: the member the client left out is the service's, got {}",
        documents[0]
    );
    for (member, value) in &claimed {
        let pair = json!(format!("{member}={}", value_text(value)));
        assert!(
            documents[0]["metadata_values"]
                .as_array()
                .is_some_and(|values| values.contains(&pair)),
            "case {case:?}: expected {pair} among the projected members, got {}",
            documents[0]
        );
    }
}

#[tokio::test]
async fn one_document_is_reported_for_every_event_of_a_batch() {
    // The projection is per EVENT, not per batch: a batch carrying several events is
    // reported as several documents, each carrying that event's own id, instant, type and
    // details beside the batch's shared context.
    let (router, _engine) = emulator();
    let monitor = "documented-batch";
    let monitor_id = create(
        &router,
        "a batch of three",
        json!({"Name": monitor, "Domain": MONITOR_DOMAIN}),
    )
    .await;
    let session = "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb";
    let batch_id = "99999999-2222-4333-8444-555555555555";
    let instants: [i64; 3] = [1_704_067_200_000, 1_704_067_260_000, 1_704_067_320_000];
    let events: Vec<Value> = instants
        .iter()
        .enumerate()
        .map(|(step, millis)| {
            json!({
                "id": format!("9911111{step}-1234-4234-8234-123456789012"),
                "timestamp": millis / MILLIS_PER_SECOND,
                "type": "com.amazon.rum.page_view_event",
                "metadata": format!("{{\"pageId\":\"/page-{step}\"}}"),
                "details": format!(
                    "{{\"pageTitle\":\"Page {step}\",\"pageInteractionId\":\"/page-{step}-0\"}}"
                ),
            })
        })
        .collect();
    deliver(
        &router,
        "a batch of three",
        &monitor_id,
        json!({
            "BatchId": batch_id,
            "AppMonitorDetails": {"name": monitor, "id": monitor_id, "version": "1.0.0"},
            "UserDetails": {"userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "sessionId": session},
            "RumEvents": events,
        }),
    )
    .await;

    let documents = reported(
        &router,
        "a batch of three",
        monitor,
        instants[0],
        instants[2],
    )
    .await;
    assert_eq!(
        documents.len(),
        instants.len(),
        "case \"a batch of three\": expected one document per delivered event, got \
         {documents:?}"
    );
    for (step, document) in documents.iter().enumerate() {
        let case = format!("event {step} of the batch");
        assert_eq!(
            document["event_id"].as_str(),
            Some(format!("9911111{step}-1234-4234-8234-123456789012").as_str()),
            "case {case:?}: expected the events in delivery order, got {document}"
        );
        assert_eq!(
            document["event_timestamp"].as_i64(),
            Some(instants[step]),
            "case {case:?}: expected each event's own instant in milliseconds, got {document}"
        );
        assert_eq!(
            document["metadata"]["pageId"].as_str(),
            Some(format!("/page-{step}").as_str()),
            "case {case:?}: expected each event's own metadata, got {document}"
        );
        assert_eq!(
            document["batch_id"].as_str(),
            Some(batch_id),
            "case {case:?}: expected every document to carry the batch's id, got {document}"
        );
        assert_eq!(
            document["log_stream"],
            instant_of(instants[step]),
            "case {case:?}: expected every document to carry its own event's instant, got \
             {document}"
        );
        assert_eq!(
            document["event_interaction"].as_str(),
            Some(format!("%2Fpage-{step}-0 &").as_str()),
            "case {case:?}: expected each event's own interaction identifier, got {document}"
        );
        assert_eq!(
            document["application_id"].as_str(),
            Some(monitor_id.as_str()),
            "case {case:?}: expected every document to name the monitor the batch reached, \
             got {document}"
        );
    }
}

/// One event of a batch that declares attributes, and the `metadata` its document owes.
struct Shared {
    /// `RumEvent$metadata`, for the events that carry one.
    metadata: Option<&'static str>,
    /// The `metadata` the document must report for this event, the caller location aside:
    /// written out per row rather than recomputed, so each row is a GOLDEN of the merge.
    merged: &'static str,
}

/// One batch: the attributes it declares for every event it carries, and those events.
struct SharedBatch {
    case: &'static str,
    /// `PutRumEventsRequest$SessionMetadata`, for the batches that declare attributes.
    session_metadata: Option<&'static str>,
    events: Vec<Shared>,
}

/// `RumEvent$id` of the event at `position` of a batch: the 36-character UUID the member's
/// modelled pattern admits
/// (`^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`).
fn shared_event_id(position: usize) -> String {
    format!("cc11111{position}-1234-4234-8234-123456789012")
}

/// The instant, in milliseconds, the event at `position` of a shared-attribute batch is
/// delivered at: one whole second apart, so a window covers the whole batch and each event
/// is placed at its own instant.
fn shared_instant(position: usize) -> i64 {
    1_704_096_000_000 + (position as i64) * MILLIS_PER_SECOND
}

/// The batches below, one per row of the rule.
///
/// `PutRumEventsRequest$SessionMetadata` is "a JSON serialization of attributes that apply to
/// every event in this batch … CloudWatch RUM merges these attributes into each event in the
/// batch, so you send them once for the batch instead of once for each event. If an attribute
/// appears both here and in an individual event's `metadata`, CloudWatch RUM uses the value in
/// the event's `metadata`" (`smithy_query(query="doc", shape="PutRumEventsRequest",
/// member="SessionMetadata")`).
///
/// The rows span what that rule generalises over: whether the batch declares attributes at
/// all, whether an event carries metadata of its own, whether an attribute is spelled in both
/// places, the TYPES the declared values have, how many events of one batch share them, and
/// documents on either side that are no JSON object (so declare no attributes and have no
/// members to merge into).
fn shared_attribute_batches() -> Vec<SharedBatch> {
    vec![
        SharedBatch {
            case: "attributes the events of the batch do not carry themselves",
            session_metadata: Some(
                "{\"browserName\":\"Chrome\",\"deviceType\":\"mobile\",\"osName\":\"iOS\"}",
            ),
            events: vec![
                Shared {
                    metadata: Some("{\"pageId\":\"/home\"}"),
                    merged: "{\"pageId\":\"/home\",\"browserName\":\"Chrome\",\
                             \"deviceType\":\"mobile\",\"osName\":\"iOS\"}",
                },
                // Sent once for the batch rather than once per event: the event that
                // serialized no metadata of its own still carries the batch's attributes.
                Shared {
                    metadata: None,
                    merged: "{\"browserName\":\"Chrome\",\"deviceType\":\"mobile\",\
                             \"osName\":\"iOS\"}",
                },
            ],
        },
        SharedBatch {
            case: "an attribute the batch and the event both spell",
            session_metadata: Some("{\"browserName\":\"Chrome\",\"osName\":\"iOS\"}"),
            events: vec![
                Shared {
                    metadata: Some("{\"browserName\":\"Firefox\"}"),
                    merged: "{\"browserName\":\"Firefox\",\"osName\":\"iOS\"}",
                },
                Shared {
                    metadata: Some("{\"osName\":\"Android\",\"pageId\":\"/cart\"}"),
                    merged: "{\"osName\":\"Android\",\"pageId\":\"/cart\",\
                             \"browserName\":\"Chrome\"}",
                },
            ],
        },
        SharedBatch {
            case: "declared attributes whose values are not all strings",
            session_metadata: Some(
                "{\"deviceMemory\":8,\"touch\":true,\"sampleRate\":0.25,\
                 \"screen\":{\"width\":1280,\"height\":720},\"features\":[\"a\",\"b\"],\
                 \"referrer\":null}",
            ),
            events: vec![Shared {
                metadata: Some("{\"pageId\":\"/specs\"}"),
                merged: "{\"pageId\":\"/specs\",\"deviceMemory\":8,\"touch\":true,\
                         \"sampleRate\":0.25,\"screen\":{\"width\":1280,\"height\":720},\
                         \"features\":[\"a\",\"b\"],\"referrer\":null}",
            }],
        },
        SharedBatch {
            // The half of the rule that must not move: a batch that declares nothing leaves
            // every event's metadata the document the client serialized.
            case: "a batch that declares no attributes at all",
            session_metadata: None,
            events: vec![Shared {
                metadata: Some("{\"pageId\":\"/home\"}"),
                merged: "{\"pageId\":\"/home\"}",
            }],
        },
        SharedBatch {
            case: "a batch whose declared attributes are the empty document",
            session_metadata: Some("{}"),
            events: vec![Shared {
                metadata: Some("{\"pageId\":\"/home\"}"),
                merged: "{\"pageId\":\"/home\"}",
            }],
        },
        SharedBatch {
            // A `JsonValue` the model constrains in no way: text that is no JSON object
            // declares no first-level attributes, so there is nothing to merge.
            case: "declared attributes that are no JSON document at all",
            session_metadata: Some("not a document"),
            events: vec![Shared {
                metadata: Some("{\"pageId\":\"/home\"}"),
                merged: "{\"pageId\":\"/home\"}",
            }],
        },
        SharedBatch {
            case: "an event whose own metadata is no JSON document at all",
            session_metadata: Some("{\"browserName\":\"Chrome\"}"),
            events: vec![Shared {
                metadata: Some("not a document"),
                // No members to merge any attribute into, so it is reported as it arrived.
                merged: "\"not a document\"",
            }],
        },
        SharedBatch {
            // An attribute of a name RUM adds itself is still the client's: the service
            // fills in what the delivery did not report rather than writing over it.
            case: "an attribute of the name the service adds for the caller's location",
            session_metadata: Some("{\"countryCode\":\"DE\"}"),
            events: vec![Shared {
                metadata: Some("{\"pageId\":\"/home\"}"),
                merged: "{\"pageId\":\"/home\",\"countryCode\":\"DE\"}",
            }],
        },
    ]
}

/// `expected` with the members RUM sets at ingestion reported AFTER everything the client
/// serialized, which is the document RUM reports: `service_set` holds them in RUM's own
/// order (the domain the telemetry is attributed to, then where the caller is).
///
/// The ONE copy of that rule in this suite — both tables build their expected `metadata`
/// through it. A member the delivery reported itself keeps the VALUE it arrived with,
/// whatever its name, which is the precedence the service applies; what RUM sets for such a
/// member is its PLACE. A document with no members at all — one the client did not
/// serialize as a JSON object — has none to report any beside.
fn with_service_set_members(expected: &Value, service_set: &Map<String, Value>) -> Value {
    let Some(client) = expected.as_object() else {
        return expected.clone();
    };
    let mut reported: Map<String, Value> = client
        .iter()
        .filter(|(member, _)| !service_set.contains_key(*member))
        .map(|(member, value)| (member.clone(), value.clone()))
        .collect();
    for (member, observed) in service_set {
        let value = client.get(member).unwrap_or(observed).clone();
        reported.insert(member.clone(), value);
    }
    Value::Object(reported)
}

#[tokio::test]
async fn declared_batch_attributes_merge_into_every_events_metadata() {
    for (index, batch) in shared_attribute_batches().iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = format!("shared-attributes-{index}");
        let monitor_id = create(
            &router,
            batch.case,
            json!({"Name": &monitor, "Domain": MONITOR_DOMAIN}),
        )
        .await;

        let events: Vec<Value> = batch
            .events
            .iter()
            .enumerate()
            .map(|(position, event)| {
                let mut carried = json!({
                    "id": shared_event_id(position),
                    "timestamp": shared_instant(position) / MILLIS_PER_SECOND,
                    "type": "com.amazon.rum.page_view_event",
                    "details": "{}",
                });
                if let Some(metadata) = event.metadata {
                    carried["metadata"] = json!(metadata);
                }
                carried
            })
            .collect();
        let mut body = json!({
            "BatchId": "cccccccc-3333-4333-8444-555555555555",
            "AppMonitorDetails": {"name": &monitor, "id": &monitor_id, "version": "1.0.0"},
            "UserDetails": {"sessionId": "dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"},
            "RumEvents": Value::Array(events),
        });
        if let Some(declared) = batch.session_metadata {
            body["SessionMetadata"] = json!(declared);
        }
        deliver(&router, batch.case, &monitor_id, body).await;

        let last = batch.events.len().saturating_sub(1);
        let documents = reported(
            &router,
            batch.case,
            &monitor,
            shared_instant(0),
            shared_instant(last),
        )
        .await;
        assert_eq!(
            documents.len(),
            batch.events.len(),
            "case {:?}: expected one document per delivered event, got {documents:?}",
            batch.case
        );

        for (position, event) in batch.events.iter().enumerate() {
            let case = format!("{} (event {position})", batch.case);
            let document = &documents[position];
            assert_eq!(
                document["event_id"].as_str(),
                Some(shared_event_id(position).as_str()),
                "case {case:?}: expected the events in delivery order, got {document}"
            );
            // The members the service set on THIS event's metadata: a document with no
            // members at all has none to report any beside.
            let service_set = if as_json(event.merged).is_object() {
                service_set_members(&case, document, Some(MONITOR_DOMAIN))
            } else {
                Map::new()
            };
            let expected = with_service_set_members(&as_json(event.merged), &service_set);
            assert_eq!(
                document["metadata"], expected,
                "case {case:?}: expected the metadata {expected}, got {document}"
            );
            // The projections are of the MERGED document, so a consumer reading them
            // positionally recovers the batch's attributes too.
            let (keys, values) = projections(&expected);
            assert_eq!(
                document["metadata_keys"],
                Value::Array(keys),
                "case {case:?}: expected the keys of the merged metadata, got {document}"
            );
            assert_eq!(
                document["metadata_values"],
                Value::Array(values),
                "case {case:?}: expected one <key>=<value> text per merged member, got \
                 {document}"
            );
        }

        // Reading reports the same documents again: the read changed nothing.
        let again = reported(
            &router,
            batch.case,
            &monitor,
            shared_instant(0),
            shared_instant(last),
        )
        .await;
        assert_eq!(
            again, documents,
            "case {:?}: expected a second read to report the same documents, got {again:?}",
            batch.case
        );
    }
}

#[tokio::test]
async fn an_event_is_selected_by_the_attributes_its_batch_declared() {
    // The merge is part of the stored document, so an attribute declared once for the batch
    // is an attribute of every event of it for everything that reads them — including the
    // dimensions `GetAppMonitorData` searches, which are the attributes of the event's
    // `metadata` (`smithy_query(query="doc", shape="QueryFilter", member="Name")`: "Valid
    // values for `Name` are `Browser` | `Device` | `Country` | `Page` | `OS` | `EventType` |
    // `Invert`"). An event spelling one itself is still selected by ITS value, not the
    // batch's.
    let (router, _engine) = emulator();
    let monitor = "shared-attribute-dimensions";
    let case = "a batch declaring the browser, the device and the operating system";
    let monitor_id = create(
        &router,
        case,
        json!({"Name": monitor, "Domain": MONITOR_DOMAIN}),
    )
    .await;
    let shared = shared_event_id(0);
    let own = shared_event_id(1);
    deliver(
        &router,
        case,
        &monitor_id,
        json!({
            "BatchId": "cccccccc-4444-4333-8444-555555555555",
            "AppMonitorDetails": {"name": monitor, "id": monitor_id},
            "UserDetails": {"sessionId": "dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"},
            "SessionMetadata":
                "{\"browserName\":\"Chrome\",\"deviceType\":\"mobile\",\"osName\":\"iOS\"}",
            "RumEvents": [
                {
                    "id": &shared,
                    "timestamp": shared_instant(0) / MILLIS_PER_SECOND,
                    "type": "com.amazon.rum.page_view_event",
                    "metadata": "{\"pageId\":\"/home\"}",
                    "details": "{}",
                },
                {
                    "id": &own,
                    "timestamp": shared_instant(1) / MILLIS_PER_SECOND,
                    "type": "com.amazon.rum.page_view_event",
                    "metadata": "{\"pageId\":\"/cart\",\"browserName\":\"Firefox\"}",
                    "details": "{}",
                },
            ],
        }),
    )
    .await;

    // Each row: the filter, and the ids of the events it must select.
    let rows: [(&str, Value, Vec<&String>); 5] = [
        (
            "the browser the batch declared",
            json!([{"Name": "Browser", "Values": ["Chrome"]}]),
            vec![&shared],
        ),
        (
            "the browser the event declared itself",
            json!([{"Name": "Browser", "Values": ["Firefox"]}]),
            vec![&own],
        ),
        (
            "the device the batch declared, for every event of it",
            json!([{"Name": "Device", "Values": ["mobile"]}]),
            vec![&shared, &own],
        ),
        (
            "the operating system the batch declared, for every event of it",
            json!([{"Name": "OS", "Values": ["iOS"]}]),
            vec![&shared, &own],
        ),
        (
            "a device no delivery declared",
            json!([{"Name": "Device", "Values": ["desktop"]}]),
            vec![],
        ),
    ];
    for (row, filters, expected) in rows {
        let documents = selected(
            &router,
            row,
            monitor,
            shared_instant(0),
            shared_instant(1),
            Some(filters),
        )
        .await;
        let selected_ids: Vec<&str> = documents
            .iter()
            .filter_map(|document| document["event_id"].as_str())
            .collect();
        let wanted: Vec<&str> = expected.iter().map(|id| id.as_str()).collect();
        assert_eq!(
            selected_ids, wanted,
            "case {row:?}: expected the events {wanted:?}, got {selected_ids:?}"
        );
    }
}

#[tokio::test]
async fn the_attributes_one_batch_may_declare_are_bounded() {
    // The declared attributes are copied into EVERY event of the batch, so what one call
    // may add to the stored documents is the declared size times the event count, and it is
    // that product this emulator bounds
    // (`crate::state::rum_event::MAX_SHARED_ATTRIBUTE_BYTES_PER_BATCH`): a call whose copies
    // stay inside the budget has its attributes merged, one past it has them merged into
    // nothing, and either way the call is accepted and every event keeps its own metadata.
    // The rows move along BOTH factors and sit on the budget and on each side of it, since
    // that is where a bound can be wrong.
    let budget = MAX_SHARED_ATTRIBUTE_BYTES_PER_BATCH;
    // (case, the bytes the batch declares, the events it carries)
    let rows: [(&str, usize, usize); 5] = [
        ("a small document and a small batch", 512, 4),
        ("one event declaring the whole budget", budget, 1),
        ("one event declaring one byte past the budget", budget + 1, 1),
        ("a batch whose copies land exactly on the budget", 1024, budget / 1024),
        ("one event more than that budget admits", 1024, budget / 1024 + 1),
    ];
    for (index, (case, declared_bytes, events)) in rows.into_iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = format!("bounded-attributes-{index}");
        let monitor_id = create(
            &router,
            case,
            json!({"Name": &monitor, "Domain": MONITOR_DOMAIN}),
        )
        .await;
        // One attribute padded to the size the row asks for, so the declared TEXT is
        // exactly `declared_bytes` long: `{"padding":"<pad>"}` costs the envelope around it.
        let envelope = json!({"padding": ""}).to_string().len();
        let padding = "p".repeat(declared_bytes.saturating_sub(envelope));
        let declared = json!({"padding": &padding}).to_string();
        assert_eq!(
            declared.len(),
            declared_bytes,
            "case {case:?}: the row must declare exactly {declared_bytes} bytes, got {}",
            declared.len()
        );
        // The event the assertions below read is the batch's FIRST, placed at its own
        // instant so the window selects it alone however many events follow it.
        let carried: Vec<Value> = (0..events)
            .map(|position| {
                json!({
                    "id": shared_event_id(position % 10),
                    "timestamp": shared_instant(position.min(1)) / MILLIS_PER_SECOND,
                    "type": "com.amazon.rum.page_view_event",
                    "metadata": "{\"pageId\":\"/home\"}",
                    "details": "{}",
                })
            })
            .collect();

        deliver(
            &router,
            case,
            &monitor_id,
            json!({
                "BatchId": "cccccccc-5555-4333-8444-555555555555",
                "AppMonitorDetails": {"name": &monitor, "id": &monitor_id},
                "UserDetails": {"sessionId": "dddddddd-eeee-4ddd-8ccc-bbbbbbbbbbbb"},
                "SessionMetadata": declared,
                "RumEvents": Value::Array(carried),
            }),
        )
        .await;

        let documents = reported(
            &router,
            case,
            &monitor,
            shared_instant(0),
            shared_instant(0),
        )
        .await;
        assert_eq!(
            documents.len(),
            1,
            "case {case:?}: expected the batch's first event at its own instant, got \
             {} document(s)",
            documents.len()
        );
        let metadata = &documents[0]["metadata"];
        assert_eq!(
            metadata["pageId"].as_str(),
            Some("/home"),
            "case {case:?}: the event keeps its own attributes whatever the batch declared, \
             got {}",
            documents[0]["event_id"]
        );
        let merged = metadata["padding"].as_str();
        if declared_bytes.saturating_mul(events) <= budget {
            assert_eq!(
                merged.map(str::len),
                Some(padding.len()),
                "case {case:?}: expected the declared attribute to be merged, got {:?}",
                merged.map(str::len)
            );
            assert_eq!(
                merged,
                Some(padding.as_str()),
                "case {case:?}: expected the declared attribute to be merged as it arrived"
            );
        } else {
            assert_eq!(
                merged, None,
                "case {case:?}: expected a call past the budget to merge nothing, got {:?}",
                merged.map(str::len)
            );
        }
    }
}

/// One delivered batch and the metadata members its document then owes, in order.
struct Placement {
    case: &'static str,
    /// The members the monitor is created with beside its `Name`: the form its domain
    /// authority is held in, or the platform of an application that has none.
    created_with: fn() -> Value,
    /// `RumEvent$metadata` as the client serialized it, for the batches that carry one.
    metadata: Option<&'static str>,
    /// The first-level members the reported `metadata` must carry, IN ORDER — which is the
    /// order `metadata_keys` and `metadata_values` are then aligned on.
    members: &'static [&'static str],
    /// The value `domain` must be reported with, or `None` for a batch neither side has a
    /// domain for.
    domain: Option<&'static str>,
    /// The members whose value the client reported and the service therefore keeps.
    kept: &'static [(&'static str, &'static str)],
}

/// Every member RUM sets on an event's metadata at ingestion is reported AFTER the members
/// the client serialized, in RUM's own order — the `domain` the telemetry is attributed to,
/// then the caller's location — whatever position the sending client put a member of one of
/// those names at, and whether it sent one at all.
///
/// The rows differ along what the rule generalises over: where in its document the client
/// serialized `domain` (first, in the middle, last, nowhere), whether it serialized a
/// location member too, whether it serialized any metadata at all, and the form the
/// monitor's domain authority is held in (`Domain`, `DomainList`, or no authority at all —
/// a platform whose application is served from no internet domain).
#[tokio::test]
async fn the_members_rum_sets_at_ingestion_are_reported_after_the_clients_own() {
    // The three members RUM sets for where the caller is always close the document, so
    // every row's expected member list ends with them.
    let rows: [Placement; 10] = [
        Placement {
            case: "a client that serialized its domain before its other members",
            created_with: || json!({"Domain": MONITOR_DOMAIN}),
            metadata: Some(
                "{\"domain\":\"shop.example.com\",\"version\":\"1.0.0\",\"pageId\":\"/checkout\",\
                  \"title\":\"Checkout\"}",
            ),
            members: &[
                "version",
                "pageId",
                "title",
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            domain: Some("shop.example.com"),
            kept: &[("version", "1.0.0"), ("pageId", "/checkout")],
        },
        Placement {
            case: "a client that serialized its domain among its other members",
            created_with: || json!({"Domain": MONITOR_DOMAIN}),
            metadata: Some(
                "{\"version\":\"1.0.0\",\"domain\":\"shop.example.com\",\"pageId\":\"/checkout\",\
                  \"title\":\"Checkout\"}",
            ),
            members: &[
                "version",
                "pageId",
                "title",
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            domain: Some("shop.example.com"),
            kept: &[("title", "Checkout")],
        },
        Placement {
            case: "a client that serialized its domain last",
            created_with: || json!({"Domain": MONITOR_DOMAIN}),
            metadata: Some(
                "{\"version\":\"1.0.0\",\"pageId\":\"/checkout\",\"domain\":\"shop.example.com\"}",
            ),
            members: &[
                "version",
                "pageId",
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            domain: Some("shop.example.com"),
            kept: &[("pageId", "/checkout")],
        },
        Placement {
            case: "a client that serialized no domain, for a monitor holding one",
            created_with: || json!({"Domain": MONITOR_DOMAIN}),
            metadata: Some("{\"version\":\"1.0.0\",\"pageId\":\"/checkout\"}"),
            members: &[
                "version",
                "pageId",
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            // Neither the client nor anything else reported a domain, so the one the
            // monitor has authority over is what the telemetry is attributed to.
            domain: Some(MONITOR_DOMAIN),
            kept: &[("version", "1.0.0")],
        },
        Placement {
            case: "a client that serialized no domain, for a monitor holding a domain list",
            created_with: || {
                json!({"DomainList": ["first.example.com", "second.example.com"]})
            },
            metadata: Some("{\"pageId\":\"/cart\"}"),
            members: &[
                "pageId",
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            domain: Some("first.example.com"),
            kept: &[("pageId", "/cart")],
        },
        Placement {
            case: "a client that serialized a location member of its own beside its domain",
            created_with: || json!({"Domain": MONITOR_DOMAIN}),
            metadata: Some(
                "{\"countryCode\":\"ZW\",\"domain\":\"shop.example.com\",\"pageId\":\"/home\"}",
            ),
            members: &[
                "pageId",
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            domain: Some("shop.example.com"),
            // `metadata` is the client's own document, so a member it reported keeps the
            // value it arrived with even where the service has a reading of its own.
            kept: &[("countryCode", "ZW"), ("pageId", "/home")],
        },
        Placement {
            case: "a batch carrying no metadata at all",
            created_with: || json!({"Domain": MONITOR_DOMAIN}),
            metadata: None,
            members: &[
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            domain: Some(MONITOR_DOMAIN),
            kept: &[],
        },
        Placement {
            case: "a client that serialized an empty metadata document",
            created_with: || json!({"Domain": MONITOR_DOMAIN}),
            metadata: Some("{}"),
            members: &[
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            domain: Some(MONITOR_DOMAIN),
            kept: &[],
        },
        Placement {
            case: "a monitor whose application has no internet domain",
            // `Platform` admits Web, Android and iOS, and only a web application has a
            // "top-level internet domain name for which your application has administrative
            // authority" — so a monitor of this platform holds no domain authority at all.
            created_with: || json!({"Platform": "iOS"}),
            metadata: Some("{\"pageId\":\"/home\"}"),
            // Neither side has a domain to report, so the member is absent rather than
            // empty.
            members: &["pageId", "countryCode", "subdivisionCode", "localityName"],
            domain: None,
            kept: &[("pageId", "/home")],
        },
        Placement {
            case: "a client that serialized a domain for a monitor holding none",
            created_with: || json!({"Platform": "Android"}),
            metadata: Some("{\"domain\":\"app.example.com\",\"pageId\":\"/home\"}"),
            members: &[
                "pageId",
                "domain",
                "countryCode",
                "subdivisionCode",
                "localityName",
            ],
            domain: Some("app.example.com"),
            kept: &[("pageId", "/home")],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = format!("placed-{index}");
        let mut created = (row.created_with)();
        created["Name"] = json!(&monitor);
        let monitor_id = create(&router, row.case, created).await;
        let instant = 1_704_067_200_i64 + index as i64;
        let mut event = json!({
            "id": format!("cccc111{index}-1234-4234-8234-123456789012"),
            "timestamp": instant,
            "type": "com.amazon.rum.page_view_event",
            "details": "{}",
        });
        if let Some(metadata) = row.metadata {
            event["metadata"] = json!(metadata);
        }
        deliver(
            &router,
            row.case,
            &monitor_id,
            json!({
                "BatchId": "11111111-2222-4333-8444-5555555555dd",
                "AppMonitorDetails": {"name": &monitor, "id": &monitor_id},
                "UserDetails": {},
                "RumEvents": [event],
            }),
        )
        .await;

        let millis = instant * MILLIS_PER_SECOND;
        let documents = reported(&router, row.case, &monitor, millis, millis).await;
        assert_eq!(
            documents.len(),
            1,
            "case {:?}: expected the one delivered event, got {documents:?}",
            row.case
        );
        let document = &documents[0];
        let reported_members = document["metadata"].as_object().unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the document's metadata to be an object, got {document}",
                row.case
            )
        });

        // The ORDER is the requirement: the client's members as it serialized them, then
        // the members RUM sets at ingestion, in RUM's order.
        let order: Vec<&str> = reported_members.keys().map(String::as_str).collect();
        assert_eq!(
            order, row.members,
            "case {:?}: expected the client's members first and the ones RUM sets at \
             ingestion after them, got {document}",
            row.case
        );
        // And the two flat projections are that same order, pairing each key with its own
        // value at the same position.
        let keys: Vec<&str> = document["metadata_keys"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: metadata_keys must be a list, got {document}",
                    row.case
                )
            })
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            keys, row.members,
            "case {:?}: expected metadata_keys to carry the members in that same order, got \
             {document}",
            row.case
        );
        let values = document["metadata_values"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (position, (key, value)) in reported_members.iter().enumerate() {
            let pair = format!("{key}={}", value_text(value));
            assert_eq!(
                values.get(position).and_then(Value::as_str),
                Some(pair.as_str()),
                "case {:?}: expected the member at position {position} to be projected as \
                 {pair:?}, got {document}",
                row.case
            );
        }

        // The VALUE of the domain: the one the client reported for the page, or the
        // monitor's own authority for a batch whose client reported none.
        assert_eq!(
            reported_members.get(DOMAIN_MEMBER).and_then(Value::as_str),
            row.domain,
            "case {:?}: expected the domain the telemetry is attributed to, got {document}",
            row.case
        );
        for (member, value) in row.kept {
            assert_eq!(
                reported_members.get(*member).and_then(Value::as_str),
                Some(*value),
                "case {:?}: expected the member {member:?} the client reported to keep its \
                 value, got {document}",
                row.case
            );
        }
    }
}
