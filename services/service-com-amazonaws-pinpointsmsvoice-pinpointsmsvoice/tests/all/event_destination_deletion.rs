// SPDX-License-Identifier: CC0-1.0

//! Which stored record `DeleteConfigurationSetEventDestination` removes, read back through
//! `GetConfigurationSetEventDestinations` and `ListConfigurationSets`.
//!
//! RULES, each asserted over a table rather than one call:
//!
//! * an event destination is scoped to the configuration set that owns it, so a delete
//!   removes the record addressed by BOTH path labels together and leaves the same name
//!   stored under every other set — and leaves the owning set itself in place, destinations
//!   or none;
//! * the name a delete frees is free: creating it again stores the new definition, so the
//!   removal left no trace of the old record behind.
//!
//! `event_destination_updates.rs` owns what a delete does to the SIBLINGS of the removed
//! destination inside one set, and the modelled `NotFoundException` an absent set or
//! destination gets; `legacy_surface_authorization.rs` owns the 403 an account without the
//! entitlement is answered before any of this is resolved.
//!
//! The suite drives the real `restJson1` router, so the requests here are the bytes an AWS
//! SDK `pinpoint-sms-voice` client sends.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The emulator under test, over the one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status and the parsed JSON body (`Value::Null` when empty).
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The `EventDestinations` member of a `GetConfigurationSetEventDestinations` answer.
    fn destinations(&self) -> Vec<Value> {
        self.body
            .get("EventDestinations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `Name` of every destination the answer reports, in the order reported.
    fn destination_names(&self) -> Vec<String> {
        self.destinations()
            .iter()
            .map(|destination| {
                destination
                    .get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `ConfigurationSets` member of a `ListConfigurationSets` answer.
    fn configuration_sets(&self) -> Vec<String> {
        self.body
            .get("ConfigurationSets")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .map(|name| name.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite drives unsigned requests; signature verification is the framework's
        // own concern and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // The account holds the entitlement this API's resource-scoped actions need, so the
        // rows below exercise what the delete DOES; the entitlement itself is pinned in
        // `legacy_surface_authorization.rs`.
        engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .legacy_surface_entitled = true;
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(value.to_string())
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://sms-voice.pinpoint.us-east-1.amazonaws.com{path}"
            ))
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/sms-voice/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("the test request must build");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Answer { status, body }
    }

    async fn create_set(&self, name: &str) -> Answer {
        self.call(
            "POST",
            "/v1/sms-voice/configuration-sets",
            Some(json!({ "ConfigurationSetName": name })),
        )
        .await
    }

    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Answer {
        self.call(
            "POST",
            &format!("/v1/sms-voice/configuration-sets/{set}/event-destinations"),
            Some(json!({ "EventDestinationName": name, "EventDestination": definition })),
        )
        .await
    }

    async fn delete_destination(&self, set: &str, name: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/v1/sms-voice/configuration-sets/{set}/event-destinations/{name}"),
            None,
        )
        .await
    }

    async fn get_destinations(&self, set: &str) -> Answer {
        self.call(
            "GET",
            &format!("/v1/sms-voice/configuration-sets/{set}/event-destinations"),
            None,
        )
        .await
    }

    async fn list_sets(&self) -> Answer {
        self.call("GET", "/v1/sms-voice/configuration-sets", None).await
    }
}

/// One row of the scoping table below.
struct Scoping {
    /// What this row is, named in every assertion it fails.
    case: &'static str,
    /// The configuration sets the row creates.
    sets: &'static [&'static str],
    /// The set the delete names.
    target_set: &'static str,
    /// The destination names stored under EVERY one of `sets`.
    destinations: &'static [&'static str],
    /// The destination the delete names.
    doomed: &'static str,
}

/// RULE: a destination is addressed by the configuration set that owns it TOGETHER with its
/// own name, so a delete removes one record and the same destination name stored under
/// another set is untouched; the set the record was removed from stays in place, with no
/// destinations left or some.
///
/// Every row stores the SAME destination names under every set, so a handler that keyed
/// destinations by name alone — or that removed the set along with its last destination —
/// fails here. The rows differ along what the rule generalises over: how many sets are
/// stored, which of them the delete names (first, middle, last), and how many destinations
/// the sets carry.
#[tokio::test]
async fn a_delete_removes_the_destination_only_from_the_set_that_owns_it() {
    let rows: &[Scoping] = &[
        Scoping {
            case: "the only destination of the first of two sets",
            sets: &["set-a", "set-b"],
            target_set: "set-a",
            destinations: &["shared"],
            doomed: "shared",
        },
        Scoping {
            case: "the only destination of the last of two sets",
            sets: &["set-a", "set-b"],
            target_set: "set-b",
            destinations: &["shared"],
            doomed: "shared",
        },
        Scoping {
            case: "one of two destinations of the middle of three sets",
            sets: &["set-a", "set-b", "set-c"],
            target_set: "set-b",
            destinations: &["shared", "kept"],
            doomed: "shared",
        },
        Scoping {
            case: "one of three destinations of the last of three sets",
            sets: &["set-a", "set-b", "set-c"],
            target_set: "set-c",
            destinations: &["alpha", "beta", "gamma"],
            doomed: "beta",
        },
    ];

    for Scoping {
        case,
        sets,
        target_set,
        destinations,
        doomed,
    } in rows
    {
        let harness = Harness::new();
        for set in *sets {
            assert_eq!(
                harness.create_set(set).await.status,
                200,
                "case {case:?}: the set {set} must be created"
            );
            for name in *destinations {
                let created = harness
                    .create_destination(set, name, json!({ "Enabled": true }))
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: creating {name} under {set} must answer 200, got {} {}",
                    created.status, created.body
                );
            }
        }

        let deleted = harness.delete_destination(target_set, doomed).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );

        for set in *sets {
            // Only the set the delete named loses the record; the stored order is the
            // destinations' own, which the create suite pins.
            let expected: Vec<String> = destinations
                .iter()
                .filter(|name| set != target_set || *name != doomed)
                .map(|name| (*name).to_string())
                .collect();
            let read = harness.get_destinations(set).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: reading {set} back must answer 200, got {} {}",
                read.status, read.body
            );
            let mut actual = read.destination_names();
            actual.sort();
            let mut expected_sorted = expected.clone();
            expected_sorted.sort();
            assert_eq!(
                actual, expected_sorted,
                "case {case:?}: expected {expected_sorted:?} under {set}, got {actual:?}"
            );
        }

        // The sets themselves survive, including one left with no destinations at all.
        let listed = harness.list_sets().await;
        let mut listed_names = listed.configuration_sets();
        listed_names.sort();
        let mut expected_sets: Vec<String> = sets.iter().map(|set| (*set).to_string()).collect();
        expected_sets.sort();
        assert_eq!(
            listed_names, expected_sets,
            "case {case:?}: a delete must not remove a configuration set, expected \
             {expected_sets:?}, got {listed_names:?}"
        );
    }
}

/// RULE: the delete frees the name, so creating it again stores the new definition — the
/// removal leaves no part of the old record behind.
///
/// The rows differ in the shape of the definition on each side of the delete (a boolean
/// alone, each of the three destination targets, every member at once), so a handler that
/// merged the new definition into a record it never removed fails here. The expectation is
/// the re-created definition plus the `Name` the record is filed under, which is what
/// `GetConfigurationSetEventDestinations` reports.
#[tokio::test]
async fn a_deleted_name_is_free_and_carries_only_the_definition_it_is_created_with_again() {
    const SET: &str = "reuse-set";
    const NAME: &str = "recycled";

    let sns = json!({ "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:voice" } });
    let cloud_watch = json!({
        "Enabled": false,
        "CloudWatchLogsDestination": {
            "IamRoleArn": "arn:aws:iam::000000000000:role/voice-logs",
            "LogGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/voice:*",
        },
    });
    let firehose = json!({
        "MatchingEventTypes": ["BUSY", "FAILED"],
        "KinesisFirehoseDestination": {
            "DeliveryStreamArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/voice",
            "IamRoleArn": "arn:aws:iam::000000000000:role/voice-firehose",
        },
    });
    let everything = json!({
        "Enabled": true,
        "MatchingEventTypes": ["ANSWERED", "RINGING"],
        "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:other" },
        "CloudWatchLogsDestination": {
            "IamRoleArn": "arn:aws:iam::000000000000:role/other-logs",
            "LogGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:/other:*",
        },
        "KinesisFirehoseDestination": {
            "DeliveryStreamArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/other",
            "IamRoleArn": "arn:aws:iam::000000000000:role/other-firehose",
        },
    });

    // (case, the definition created first, the definition created again after the delete)
    let rows: &[(&str, Value, Value)] = &[
        (
            "every member, then the boolean alone",
            everything.clone(),
            json!({ "Enabled": true }),
        ),
        (
            "the SNS target, then the CloudWatch Logs target",
            sns.clone(),
            cloud_watch.clone(),
        ),
        (
            "the CloudWatch Logs target, then the Firehose target",
            cloud_watch.clone(),
            firehose.clone(),
        ),
        (
            "the Firehose target, then every member",
            firehose.clone(),
            everything.clone(),
        ),
        (
            "no members at all, then the SNS target",
            json!({}),
            sns.clone(),
        ),
        (
            "the SNS target, then no members at all",
            sns.clone(),
            json!({}),
        ),
    ];

    for (case, first, second) in rows {
        let harness = Harness::new();
        assert_eq!(
            harness.create_set(SET).await.status,
            200,
            "case {case:?}: the set must be created"
        );
        assert_eq!(
            harness
                .create_destination(SET, NAME, first.clone())
                .await
                .status,
            200,
            "case {case:?}: the first create must answer 200"
        );
        assert_eq!(
            harness.delete_destination(SET, NAME).await.status,
            200,
            "case {case:?}: the delete must answer 200"
        );

        let recreated = harness.create_destination(SET, NAME, second.clone()).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: creating the freed name again must answer 200, got {} {}",
            recreated.status, recreated.body
        );

        let mut expected = second.clone();
        expected
            .as_object_mut()
            .expect("a definition row is a JSON object")
            .insert("Name".to_string(), json!(NAME));

        let read = harness.get_destinations(SET).await;
        assert_eq!(
            read.destinations(),
            vec![expected.clone()],
            "case {case:?}: expected {expected} alone, got {}",
            read.body
        );
    }
}
