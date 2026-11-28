// SPDX-License-Identifier: CC0-1.0

//! The COPIES of delivered telemetry an app monitor with `CwLogEnabled` stores in
//! CloudWatch Logs.
//!
//! The rule under test: while a monitor's `CwLogEnabled` is on, every event of every batch
//! `PutRumEvents` accepts for it is copied into the log group the monitor advertises as
//! `DataStorage.CwLog.CwLogGroup` — one log event per delivered RUM event, carrying exactly
//! the RUM event document the data plane enriched that event into, in the stream of the
//! user session the batch came from — and while it is off, nothing is copied anywhere. The
//! copies are stored BESIDE the monitor's own buffer: `GetAppMonitorData` still reports
//! every delivered event, as the same document.
//!
//! The rows span what the rule generalises over: how many events one batch carries, how
//! many batches a session sends, how many sessions a monitor collects from, whether the
//! batch names a session at all, whether it names an `Alias` (a request-level condition
//! value, which is no part of a RUM event), whether an event carries `metadata`, and the
//! shape of an event's `details`. The second suite spans the other dimension — when
//! delivery is on: off at the create, turned on by an update, turned off again.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::authorization::{Emulator, ACCOUNT_KEY};

/// The prefix a RUM-managed log group's name carries: RUM is a vended-logs service.
const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// One RUM event a batch carries.
struct Event {
    /// `RumEvent$id`: 36 characters matching the member's UUID pattern.
    id: &'static str,
    /// `RumEvent$type`.
    event_type: &'static str,
    /// `RumEvent$timestamp` as seconds since the epoch — the unit the wire carries it in —
    /// and the milliseconds the copied document carries it as.
    timestamp: i64,
    instant_millis: i64,
    /// `RumEvent$metadata`, an `@mediaType("application/json")` string, for the events that
    /// carry one.
    metadata: Option<&'static str>,
    /// `RumEvent$details`, the one such document every event carries.
    details: &'static str,
}

impl Event {
    /// This event as `PutRumEvents` carries it on the wire.
    fn wire(&self) -> Value {
        let mut event = json!({
            "id": self.id,
            "timestamp": self.timestamp,
            "type": self.event_type,
            "details": self.details,
        });
        if let Some(metadata) = self.metadata {
            event["metadata"] = json!(metadata);
        }
        event
    }

    /// The `event_interaction` the copy carries for this event: the percent-encoded
    /// identifier of the interaction the event belongs to and the separator, and no
    /// identifier at all for an event that names none — which is every event of this suite
    /// but the one row whose details carry a `pageInteractionId`.
    fn interaction(&self) -> Value {
        match parsed(self.details).get("pageInteractionId").cloned() {
            Some(Value::String(identifier)) => {
                json!(format!("{} &", identifier.replace('/', "%2F")))
            }
            _ => json!(""),
        }
    }

    /// The instant this event's document carries as `log_stream`: the whole seconds since
    /// the epoch and the nanoseconds within that second, which is zero for every fixture
    /// here, since the wire carries each of these instants as whole seconds.
    fn instant(&self) -> Value {
        json!({"seconds": self.timestamp, "nanos": 0})
    }
}

/// One `PutRumEvents` call a row makes.
struct Delivery {
    case: &'static str,
    /// `PutRumEventsRequest$BatchId`.
    batch_id: &'static str,
    /// `PutRumEventsRequest$Alias`, for the batches that name one.
    alias: Option<&'static str>,
    /// `UserDetails$userId` / `UserDetails$sessionId`.
    user_id: Option<&'static str>,
    session_id: Option<&'static str>,
    /// `AppMonitorDetails$version`, the client's own view of the app monitor.
    version: Option<&'static str>,
    events: Vec<Event>,
}

impl Delivery {
    /// This batch as `PutRumEvents` carries it, for the monitor `monitor`/`monitor_id`.
    fn wire(&self, monitor: &str, monitor_id: &str) -> Value {
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
        let mut body = json!({
            "BatchId": self.batch_id,
            "AppMonitorDetails": app_monitor_details,
            "UserDetails": user_details,
            "RumEvents": self.events.iter().map(Event::wire).collect::<Vec<_>>(),
        });
        if let Some(alias) = self.alias {
            body["Alias"] = json!(alias);
        }
        body
    }

    /// The copy this batch owes the log group for `event`: the RUM event document the data
    /// plane enriched that event into, under the field names RUM publishes for a RUM event.
    ///
    /// `service_set` is what the service set on the metadata at ingestion — the domain the
    /// telemetry is attributed to and where it was reported to come from — read off the
    /// document the monitor's own buffer holds for an event whose client serialized none of
    /// them: those members and their position after the client's own are the requirement,
    /// the place itself is the environment's, and what this asserts is that the COPY carries
    /// the same bytes.
    fn expected_copy(
        &self,
        event: &Event,
        monitor: &str,
        monitor_id: &str,
        service_set: &Map<String, Value>,
    ) -> Value {
        let carried = event.metadata.map(parsed).unwrap_or_else(|| json!({}));
        let metadata = match carried {
            Value::Object(client) => {
                // The client's members in the order it serialized them, then every member
                // the service set \u2014 each with the client's value where it serialized one,
                // since what RUM sets for such a member is its place.
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
            other => other,
        };
        let (keys, values) = projections(&metadata);
        let mut copy = json!({
            "account_id": engine::emulator_account_id(),
            "application_id": monitor_id,
            "application_name": monitor,
            "batch_id": self.batch_id,
            "event_id": event.id,
            "event_interaction": event.interaction(),
            "event_timestamp": event.instant_millis,
            "event_type": event.event_type,
            "event_details": parsed(event.details),
            "log_stream": event.instant(),
            "metadata": metadata,
            "metadata_keys": keys,
            "metadata_values": values,
            "user_details": {},
        });
        if let Some(version) = self.version {
            copy["application_version"] = json!(version);
        }
        if let Some(user_id) = self.user_id {
            copy["userId"] = json!(user_id);
            copy["user_details"]["userId"] = json!(user_id);
        }
        if let Some(session_id) = self.session_id {
            copy["sessionId"] = json!(session_id);
            copy["user_details"]["sessionId"] = json!(session_id);
        }
        copy
    }

    /// The log stream this batch's copies belong in: the session it came from, and the
    /// monitor's own id for a batch that named none.
    fn stream<'a>(&'a self, monitor_id: &'a str) -> &'a str {
        self.session_id.unwrap_or(monitor_id)
    }
}

/// `text` as the JSON document it carries.
fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("the fixture {text:?} must be a JSON document ({error})");
    })
}

/// The `metadata_keys` / `metadata_values` projections a document carries beside its
/// `metadata`: the metadata's first-level keys, and at the same positions the
/// `<key>=<value>` text of each member.
fn projections(metadata: &Value) -> (Vec<Value>, Vec<Value>) {
    metadata
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| {
                    (
                        json!(key),
                        json!(format!(
                            "{key}={}",
                            match value {
                                Value::String(text) => text.clone(),
                                other => other.to_string(),
                            }
                        )),
                    )
                })
                .unzip()
        })
        .unwrap_or_default()
}

/// The members RUM sets on an event's `metadata` at ingestion, in the order it reports
/// them: the domain the telemetry is attributed to, and then where the caller is.
///
/// Source: the recorded enriched document, which carries them after the members the client
/// serialized; the model names one of them as an extended-metric dimension
/// (`"metadata.countryCode": "CountryCode"`, `smithy_query(query="doc",
/// shape="MetricDefinitionRequest", member="DimensionKeys")`). Their values are the
/// environment's, so none is stated here.
const SERVICE_SET_MEMBERS: [&str; 4] = [
    "domain",
    "countryCode",
    "subdivisionCode",
    "localityName",
];

/// What `document`'s metadata says the service set on it, asserting all four members are
/// there as texts.
fn service_set_members(case: &str, document: &Value) -> Map<String, Value> {
    let metadata = document["metadata"].as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected the document's metadata to be an object, got {document}")
    });
    SERVICE_SET_MEMBERS
        .into_iter()
        .map(|member| {
            let value = metadata.get(member).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected RUM to set {member:?} on the event's metadata, \
                     got {document}"
                )
            });
            assert!(
                value.as_str().is_some_and(|text| !text.is_empty()),
                "case {case:?}: expected {member:?} to name something, got {value}"
            );
            (member.to_string(), value.clone())
        })
        .collect()
}

/// Now, in milliseconds since the epoch — the unit a log event's timestamp is in.
fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_millis(),
    )
    .expect("the clock fits in a log event timestamp")
}

/// Create one app monitor and answer its minted `Id`.
async fn create_monitor(emulator: &Emulator, case: &str, name: &str, cw_log_enabled: bool) -> String {
    let created = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            "/appmonitor",
            Some(json!({
                "Name": name,
                "Domain": "example.com",
                "CwLogEnabled": cw_log_enabled,
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created.json(case)["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAppMonitor must answer an Id"))
        .to_string()
}

/// The `DataStorage.CwLog.CwLogGroup` the monitor `name` advertises.
async fn advertised_group(emulator: &Emulator, case: &str, name: &str) -> Option<String> {
    let reply = emulator
        .rum(ACCOUNT_KEY, "GET", &format!("/appmonitor/{name}"), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["AppMonitor"]["DataStorage"]["CwLog"]["CwLogGroup"]
        .as_str()
        .map(str::to_string)
}

/// Deliver one batch to the monitor whose id is `monitor_id`.
async fn deliver(emulator: &Emulator, case: &str, monitor_id: &str, body: Value) {
    let reply = emulator
        .rum(
            ACCOUNT_KEY,
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

/// The `logStreamName`s `logs:DescribeLogStreams` reports for a log group, sorted.
async fn streams_of(emulator: &Emulator, case: &str, log_group: &str) -> Vec<String> {
    let (status, body) = emulator
        .logs("DescribeLogStreams", &json!({"logGroupName": log_group}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeLogStreams to answer 200, got {status} {body}"
    );
    let mut names: Vec<String> = body["logStreams"]
        .as_array()
        .map(|streams| {
            streams
                .iter()
                .filter_map(|stream| stream["logStreamName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The log events `logs:GetLogEvents` reports for one stream, oldest first.
async fn log_events_in(
    emulator: &Emulator,
    case: &str,
    log_group: &str,
    log_stream: &str,
) -> Vec<Value> {
    let (status, body) = emulator
        .logs(
            "GetLogEvents",
            &json!({
                "logGroupName": log_group,
                "logStreamName": log_stream,
                "startFromHead": true,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetLogEvents to answer 200, got {status} {body}"
    );
    body["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `message` of each log event, parsed as the JSON document the copy is.
fn messages(case: &str, events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .map(|event| {
            let message = event["message"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a log event must carry a message, got {event}")
            });
            serde_json::from_str(message).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: a copy must be a JSON document, got {message:?} ({error})"
                )
            })
        })
        .collect()
}

/// The `Events` `GetAppMonitorData` reports for `monitor` over the whole epoch.
async fn collected_event_ids(emulator: &Emulator, case: &str, monitor: &str) -> Vec<String> {
    let reply = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            &format!("/appmonitor/{monitor}/data"),
            Some(json!({"TimeRange": {"After": 0}})),
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
        .filter_map(|entry| {
            let document: Value = serde_json::from_str(entry.as_str().unwrap_or_default()).ok()?;
            document["event_id"].as_str().map(str::to_string)
        })
        .collect()
}

/// The batches every row of the first suite delivers, in order.
fn deliveries() -> Vec<Delivery> {
    vec![
        Delivery {
            case: "one event from a session",
            batch_id: "11111111-2222-4333-8444-555555555555",
            alias: None,
            user_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            events: vec![Event {
                id: "11111111-1234-4234-8234-123456789012",
                event_type: "com.amazon.rum.page_view_event",
                timestamp: 1_704_067_200,
                instant_millis: 1_704_067_200_000,
                metadata: Some("{\"browserName\":\"Chrome\",\"pageId\":\"/home\"}"),
                details: "{\"pageTitle\":\"Home\"}",
            }],
        },
        Delivery {
            case: "three events from the SAME session, with an alias",
            batch_id: "22222222-2222-4333-8444-555555555555",
            alias: Some("live"),
            user_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("1.0.0"),
            events: vec![
                Event {
                    id: "21111111-1234-4234-8234-123456789012",
                    event_type: "com.amazon.rum.js_error_event",
                    timestamp: 1_704_067_260,
                    instant_millis: 1_704_067_260_000,
                    metadata: None,
                    details: "{\"message\":\"boom\"}",
                },
                Event {
                    id: "22111111-1234-4234-8234-123456789012",
                    event_type: "com.amazon.rum.http_event",
                    timestamp: 1_704_067_320,
                    instant_millis: 1_704_067_320_000,
                    metadata: Some("{\"deviceType\":\"mobile\"}"),
                    // A nested document, since `details` is any JSON value.
                    details: "{\"request\":{\"method\":\"GET\",\"status\":200}}",
                },
                Event {
                    id: "23111111-1234-4234-8234-123456789012",
                    event_type: "com.amazon.rum.performance_navigation_event",
                    timestamp: 1_704_067_380,
                    instant_millis: 1_704_067_380_000,
                    metadata: None,
                    details: "{\"duration\":12.5}",
                },
            ],
        },
        Delivery {
            case: "a batch from a SECOND session",
            batch_id: "33333333-2222-4333-8444-555555555555",
            alias: None,
            user_id: Some("bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            session_id: Some("eeeeeeee-eeee-4ddd-8ccc-bbbbbbbbbbbb"),
            version: Some("2.3.1"),
            events: vec![
                Event {
                    id: "31111111-1234-4234-8234-123456789012",
                    event_type: "com.amazon.rum.session_start_event",
                    timestamp: 1_704_070_800,
                    instant_millis: 1_704_070_800_000,
                    metadata: None,
                    details: "{\"sessionStart\":true}",
                },
                Event {
                    id: "32111111-1234-4234-8234-123456789012",
                    event_type: "com.amazon.rum.page_view_event",
                    timestamp: 1_704_070_860,
                    instant_millis: 1_704_070_860_000,
                    metadata: Some("{\"countryCode\":\"JP\"}"),
                    details: "{\"pageTitle\":\"Checkout\"}",
                },
            ],
        },
        Delivery {
            case: "a batch that names NO session and no version",
            batch_id: "44444444-2222-4333-8444-555555555555",
            alias: None,
            user_id: None,
            session_id: None,
            version: None,
            events: vec![Event {
                id: "41111111-1234-4234-8234-123456789012",
                event_type: "com.amazon.rum.js_error_event",
                timestamp: 1_704_074_400,
                instant_millis: 1_704_074_400_000,
                metadata: None,
                // An empty document, which is what the minimal client sends.
                details: "{}",
            }],
        },
    ]
}

#[tokio::test]
async fn every_delivered_event_is_copied_into_the_vended_log_group() {
    let emulator = Emulator::new();
    let monitor = "copies";
    let monitor_id = create_monitor(&emulator, "setup", monitor, true).await;
    let log_group = advertised_group(&emulator, "setup", monitor)
        .await
        .unwrap_or_else(|| panic!("a monitor with delivery on must advertise a log group"));
    let named = format!("{CW_LOG_GROUP_PREFIX}{monitor}");
    assert!(
        log_group.starts_with(&named) && log_group.len() > named.len(),
        "case \"setup\": expected the vended group of the monitor to extend {named:?} with the \
         suffix the service chose for it, got {log_group:?}"
    );

    // What each stream owes, in the order the batches were delivered.
    let before = now_ms();
    for delivery in deliveries() {
        deliver(
            &emulator,
            delivery.case,
            &monitor_id,
            delivery.wire(monitor, &monitor_id),
        )
        .await;
    }
    let after = now_ms();

    // What the service set on the metadata at ingestion, read off the copy of an event whose
    // client sent NO metadata of its own, so all four members are the service's own: they
    // are the requirement, the values are the environment's, and every document below is
    // then owed them wherever its client named none.
    let probe = "the members the service sets on an event's metadata";
    let mut service_set = Map::new();
    for delivery in deliveries() {
        if service_set.is_empty() {
            let stream = delivery.stream(&monitor_id).to_string();
            let copies = messages(
                probe,
                &log_events_in(&emulator, probe, &log_group, &stream).await,
            );
            for event in &delivery.events {
                if event.metadata.is_some() {
                    continue;
                }
                if let Some(copy) = copies
                    .iter()
                    .find(|copy| copy["event_id"].as_str() == Some(event.id))
                {
                    service_set = service_set_members(probe, copy);
                    break;
                }
            }
        }
    }
    assert!(
        !service_set.is_empty(),
        "case {probe:?}: the batches must deliver one event whose client sent no metadata, \
         so the members the service sets are observable"
    );

    let mut expected: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    for delivery in deliveries() {
        let stream = delivery.stream(&monitor_id).to_string();
        for event in &delivery.events {
            expected
                .entry(stream.clone())
                .or_default()
                .push(delivery.expected_copy(event, monitor, &monitor_id, &service_set));
        }
    }

    // One stream per session the monitor collected from, plus the monitor's own for the
    // batch that named no session — and nothing else.
    assert_eq!(
        streams_of(&emulator, "every stream", &log_group).await,
        expected.keys().cloned().collect::<Vec<_>>(),
        "case \"every stream\": expected one stream per session the batches named, got the \
         streams listed above"
    );

    for (stream, owed) in &expected {
        let case = format!("stream {stream}");
        let events = log_events_in(&emulator, &case, &log_group, stream).await;
        assert_eq!(
            messages(&case, &events),
            *owed,
            "case {case:?}: expected one copy per delivered event, in delivery order, got \
             {events:?}"
        );
        for event in &events {
            let timestamp = event["timestamp"].as_i64().unwrap_or_default();
            assert!(
                (before..=after).contains(&timestamp),
                "case {case:?}: a copy is stamped with the instant RUM delivered it, so the \
                 timestamp must fall in {before}..={after}, got {timestamp} ({event})"
            );
        }
    }

    // The copies are stored BESIDE the monitor's own buffer: the data plane's reader still
    // reports every delivered event.
    let delivered_ids: Vec<String> = deliveries()
        .iter()
        .flat_map(|delivery| {
            delivery
                .events
                .iter()
                .map(|event| event.id.to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        collected_event_ids(&emulator, "the monitor's own buffer", monitor).await,
        delivered_ids,
        "case \"the monitor's own buffer\": copying to CloudWatch Logs must not change what \
         GetAppMonitorData reports"
    );
}

#[tokio::test]
async fn copies_are_stored_exactly_while_delivery_is_on() {
    let emulator = Emulator::new();
    let monitor = "copies-toggled";
    let monitor_id = create_monitor(&emulator, "setup", monitor, false).await;

    /// One batch per row, each carrying a single event whose `id` the row is identified by.
    fn batch(batch_id: &str, event_id: &str, timestamp: i64, monitor_id: &str) -> Value {
        json!({
            "BatchId": batch_id,
            "AppMonitorDetails": {"name": "copies-toggled", "id": monitor_id},
            "UserDetails": {"sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb"},
            "RumEvents": [{
                "id": event_id,
                "timestamp": timestamp,
                "type": "com.amazon.rum.page_view_event",
                "details": "{\"pageId\":\"/home\"}",
            }],
        })
    }

    /// Turn delivery on or off for the monitor.
    async fn set_delivery(emulator: &Emulator, case: &str, monitor: &str, enabled: bool) {
        let reply = emulator
            .rum(
                ACCOUNT_KEY,
                "PATCH",
                &format!("/appmonitor/{monitor}"),
                Some(json!({"CwLogEnabled": enabled})),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected UpdateAppMonitor to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    let session = "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb";
    let delivery_off_event = "51111111-1234-4234-8234-123456789012";
    let delivery_on_event = "52111111-1234-4234-8234-123456789012";
    let delivery_off_again_event = "53111111-1234-4234-8234-123456789012";

    // 1. Delivery was never turned on: the batch is accepted and copied nowhere — the
    //    monitor advertises no group, so there is no group to hold a copy.
    deliver(
        &emulator,
        "delivery off",
        &monitor_id,
        batch(
            "11111111-2222-4333-8444-555555555555",
            delivery_off_event,
            1_704_067_200,
            &monitor_id,
        ),
    )
    .await;
    assert!(
        advertised_group(&emulator, "delivery off", monitor)
            .await
            .is_none(),
        "case \"delivery off\": a monitor that stores no copies advertises no group"
    );
    let (status, body) = emulator
        .logs(
            "DescribeLogGroups",
            &json!({"logGroupNamePrefix": CW_LOG_GROUP_PREFIX}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"delivery off\": expected DescribeLogGroups to answer 200, got {status} {body}"
    );
    assert_eq!(
        body["logGroups"].as_array().map(Vec::len).unwrap_or_default(),
        0,
        "case \"delivery off\": a batch delivered to a monitor that stores no copies \
         provisions no group, got {body}"
    );

    // 2. An update turns delivery on: the batches from then on are copied, and the ones
    //    delivered before are not retro-copied — they were delivered while the monitor
    //    stored no copies.
    set_delivery(&emulator, "delivery on", monitor, true).await;
    // The group RUM vended for this monitor, read off the monitor now that it advertises
    // one: part of the name is the service's own choice, so it cannot be rebuilt from the
    // monitor's name.
    let log_group = advertised_group(&emulator, "delivery on", monitor)
        .await
        .unwrap_or_else(|| {
            panic!("case \"delivery on\": a monitor with delivery on must advertise a log group")
        });
    assert_eq!(
        streams_of(&emulator, "delivery on", &log_group).await,
        Vec::<String>::new(),
        "case \"delivery on\": turning delivery on copies nothing by itself"
    );
    deliver(
        &emulator,
        "delivery on",
        &monitor_id,
        batch(
            "22222222-2222-4333-8444-555555555555",
            delivery_on_event,
            1_704_067_260,
            &monitor_id,
        ),
    )
    .await;
    let copied = messages(
        "delivery on",
        &log_events_in(&emulator, "delivery on", &log_group, session).await,
    );
    assert_eq!(
        copied
            .iter()
            .map(|copy| copy["event_id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec![delivery_on_event],
        "case \"delivery on\": only the batch delivered while delivery was on is copied, \
         got {copied:?}"
    );

    // 3. And an update that turns it off again stops the copies, leaving the ones already
    //    stored where they are.
    set_delivery(&emulator, "delivery off again", monitor, false).await;
    deliver(
        &emulator,
        "delivery off again",
        &monitor_id,
        batch(
            "33333333-2222-4333-8444-555555555555",
            delivery_off_again_event,
            1_704_067_320,
            &monitor_id,
        ),
    )
    .await;
    let copied = messages(
        "delivery off again",
        &log_events_in(&emulator, "delivery off again", &log_group, session).await,
    );
    assert_eq!(
        copied
            .iter()
            .map(|copy| copy["event_id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec![delivery_on_event],
        "case \"delivery off again\": a batch delivered while delivery is off is copied \
         nowhere, got {copied:?}"
    );

    // Every batch is in the monitor's own buffer whatever delivery was doing.
    assert_eq!(
        collected_event_ids(&emulator, "the monitor's own buffer", monitor).await,
        vec![
            delivery_off_event.to_string(),
            delivery_on_event.to_string(),
            delivery_off_again_event.to_string(),
        ],
        "case \"the monitor's own buffer\": every delivered batch is stored on the monitor"
    );
}
