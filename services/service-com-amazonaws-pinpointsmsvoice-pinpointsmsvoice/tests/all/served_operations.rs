// SPDX-License-Identifier: CC0-1.0

//! That every operation this service binds is SERVED, and that the write
//! `UpdateConfigurationSetEventDestination` performs is real state scoped to the
//! configuration set the request names.
//!
//! RULES, each asserted over a table rather than one call:
//!
//! * every operation the service binds answers its own result — a modelled success or a
//!   modelled refusal — so none of them answers the scaffold channel's HTTP 501
//!   `NotImplemented`, whatever the account's entitlement and whatever the request names;
//! * an update writes state that outlives the call and is scoped to the configuration set
//!   that owns the destination, so two sets holding a destination of the SAME name are
//!   independent and repeating one update changes nothing further.
//!
//! The routes below are the ones the model binds (`smithy_query(query="digest",
//! model_path="services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/\
//! smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json")`, also rendered in
//! the crate's `OPERATIONS.md`), and the suite drives the real `restJson1` router, so the
//! requests here are the bytes an AWS SDK `pinpoint-sms-voice` client sends.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The HTTP status the scaffold channel answers an unimplemented operation with, and the
/// error code it carries (`crate::scaffolded`): what no operation of this service may
/// answer any more.
const SCAFFOLD_STATUS: u16 = 501;
/// The error code that same channel carries.
const SCAFFOLD_CODE: &str = "NotImplemented";

/// A configuration set the surface rows address, created through the API.
const SURFACE_SET: &str = "served-set";
/// An event destination of [`SURFACE_SET`], created through the API.
const SURFACE_DESTINATION: &str = "served-dest";

/// The emulator under test, over the one shared engine.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer: the status and the parsed JSON body (`Value::Null` when empty).
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The `__type` discriminator of a `restJson1` error envelope, or `""` on a success.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The `EventDestinations` member of a `GetConfigurationSetEventDestinations` answer.
    fn destinations(&self) -> Vec<Value> {
        self.body
            .get("EventDestinations")
            .and_then(Value::as_array)
            .cloned()
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
        Self { router, engine }
    }

    /// Grant the account the entitlement this API's resource-scoped actions need. No
    /// operation in this model grants it, so a suite that needs those actions to run
    /// states it in the store, exactly as the handlers read it; the entitlement rule
    /// itself is pinned in `legacy_surface_authorization.rs`.
    fn entitle(&self) {
        self.engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .legacy_surface_entitled = true;
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

    async fn update_destination(&self, set: &str, name: &str, definition: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/v1/sms-voice/configuration-sets/{set}/event-destinations/{name}"),
            Some(json!({ "EventDestination": definition })),
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

    /// The set and destination the surface rows address, created through the API so a row
    /// that names them names something that really exists.
    async fn seed_surface(&self) {
        assert_eq!(
            self.create_set(SURFACE_SET).await.status,
            200,
            "the surface set must be created"
        );
        assert_eq!(
            self.create_destination(SURFACE_SET, SURFACE_DESTINATION, json!({ "Enabled": true }))
                .await
                .status,
            200,
            "the surface destination must be created"
        );
    }
}

/// One operation of this service, as the request that reaches it.
struct Bound {
    /// The operation name, for the assertion messages.
    name: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The status an ENTITLED account is answered: every one of these operations is
    /// implemented and the request below is well formed and names something that exists,
    /// so each answers its modelled success.
    entitled_status: u16,
}

/// Every operation the model binds, each as a well-formed request naming the seeded
/// resources — eight rows for the eight bound operations.
fn bound_operations() -> Vec<Bound> {
    let destination_path = format!(
        "/v1/sms-voice/configuration-sets/{SURFACE_SET}/event-destinations/{SURFACE_DESTINATION}"
    );
    vec![
        Bound {
            name: "CreateConfigurationSet",
            method: "POST",
            path: "/v1/sms-voice/configuration-sets".to_string(),
            body: Some(json!({ "ConfigurationSetName": "served-second-set" })),
            entitled_status: 200,
        },
        Bound {
            name: "CreateConfigurationSetEventDestination",
            method: "POST",
            path: format!("/v1/sms-voice/configuration-sets/{SURFACE_SET}/event-destinations"),
            body: Some(json!({
                "EventDestinationName": "served-second-dest",
                "EventDestination": { "Enabled": false },
            })),
            entitled_status: 200,
        },
        Bound {
            name: "GetConfigurationSetEventDestinations",
            method: "GET",
            path: format!("/v1/sms-voice/configuration-sets/{SURFACE_SET}/event-destinations"),
            body: None,
            entitled_status: 200,
        },
        Bound {
            name: "ListConfigurationSets",
            method: "GET",
            path: "/v1/sms-voice/configuration-sets".to_string(),
            body: None,
            entitled_status: 200,
        },
        Bound {
            name: "SendVoiceMessage",
            method: "POST",
            path: "/v1/sms-voice/voice/message".to_string(),
            body: Some(json!({
                "DestinationPhoneNumber": "+14255550199",
                "ConfigurationSetName": SURFACE_SET,
                "Content": { "PlainTextMessage": { "Text": "served" } },
            })),
            entitled_status: 200,
        },
        Bound {
            name: "UpdateConfigurationSetEventDestination",
            method: "PUT",
            path: destination_path.clone(),
            body: Some(json!({ "EventDestination": { "Enabled": false } })),
            entitled_status: 200,
        },
        Bound {
            name: "DeleteConfigurationSetEventDestination",
            method: "DELETE",
            path: destination_path,
            body: None,
            entitled_status: 200,
        },
        Bound {
            name: "DeleteConfigurationSet",
            method: "DELETE",
            path: format!("/v1/sms-voice/configuration-sets/{SURFACE_SET}"),
            body: None,
            entitled_status: 200,
        },
    ]
}

/// RULE: every operation the service binds answers its own modelled result, so none of
/// them answers the scaffold channel's HTTP 501 `NotImplemented`.
///
/// Each row runs on its own harness, so no row depends on what another one did, and each
/// is driven for an ENTITLED account (where the operation answers its modelled success)
/// and for an UNENTITLED one (where a resource-scoped action is refused by the
/// entitlement rule `legacy_surface_authorization.rs` owns) — the refusal is a modelled
/// decision either way, never the scaffold's.
#[tokio::test]
async fn every_bound_operation_answers_its_own_result_and_never_the_scaffold_501() {
    for bound in bound_operations() {
        for entitled in [true, false] {
            let case = format!("{} (entitled: {entitled})", bound.name);
            let harness = Harness::new();
            harness.entitle();
            harness.seed_surface().await;
            if !entitled {
                harness
                    .engine
                    .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
                    .lock()
                    .legacy_surface_entitled = false;
            }

            let answer = harness
                .call(bound.method, &bound.path, bound.body.clone())
                .await;
            assert_ne!(
                answer.status, SCAFFOLD_STATUS,
                "case {case:?}: expected a status other than {SCAFFOLD_STATUS}, got {} {}",
                answer.status, answer.body
            );
            assert_ne!(
                answer.error_code(),
                SCAFFOLD_CODE,
                "case {case:?}: expected an answer other than {SCAFFOLD_CODE}, got {}",
                answer.body
            );
            if entitled {
                assert_eq!(
                    answer.status, bound.entitled_status,
                    "case {case:?}: expected {}, got {} {}",
                    bound.entitled_status, answer.status, answer.body
                );
            }
        }
    }
}

/// The definition every set's destination is created with, so a row can show which set's
/// copy its update reached.
fn created_definition() -> Value {
    json!({ "Enabled": true, "MatchingEventTypes": ["RINGING"] })
}

/// What the update carries, and so what the set it names must read back as.
fn update_definition() -> Value {
    json!({ "Enabled": false, "MatchingEventTypes": ["BUSY", "FAILED"] })
}

/// RULE: an update writes state that outlives the call and is scoped to the configuration
/// set that owns the destination, so a destination of the same name in another set is
/// untouched and repeating the update changes nothing further.
///
/// The rows differ in how many sets hold a destination of that one name and in which of
/// them the update names, so the rule is not pinned on a store holding a single set.
#[tokio::test]
async fn an_update_writes_state_scoped_to_the_set_it_names() {
    /// The destination name every set in a row holds, so a set can only be told apart by
    /// what its own copy reads back as.
    const SHARED: &str = "shared-dest";
    // (case, the sets to create, the one whose destination is updated)
    let rows: &[(&str, &[&str], &str)] = &[
        ("one set", &["only"], "only"),
        ("the first of three sets", &["a", "b", "c"], "a"),
        ("the middle of three sets", &["a", "b", "c"], "b"),
        ("the last of three sets", &["a", "b", "c"], "c"),
    ];

    for (case, sets, updated_set) in rows {
        let harness = Harness::new();
        harness.entitle();
        for set in *sets {
            assert_eq!(
                harness.create_set(set).await.status,
                200,
                "case {case:?}: creating set {set}"
            );
            assert_eq!(
                harness
                    .create_destination(set, SHARED, created_definition())
                    .await
                    .status,
                200,
                "case {case:?}: creating the destination of set {set}"
            );
        }

        // The same update three times over: the first one writes, and a repeat of an
        // update that states the same members states nothing new.
        for attempt in 1..=3 {
            let updated = harness
                .update_destination(updated_set, SHARED, update_definition())
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: update attempt {attempt} must answer 200, got {} {}",
                updated.status, updated.body
            );

            for set in *sets {
                // The set the update named reads back the update's members; every other
                // set reads back exactly what it was created with.
                let mut expected = if set == updated_set {
                    update_definition()
                } else {
                    created_definition()
                };
                expected
                    .as_object_mut()
                    .expect("a definition is a JSON object")
                    .insert("Name".to_string(), json!(SHARED));

                let read = harness.get_destinations(set).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: reading set {set} back after attempt {attempt} must \
                     answer 200, got {} {}",
                    read.status, read.body
                );
                assert_eq!(
                    read.destinations(),
                    vec![expected.clone()],
                    "case {case:?}: after attempt {attempt}, set {set} expected \
                     [{expected}], got {}",
                    read.body
                );
            }
        }
    }
}
