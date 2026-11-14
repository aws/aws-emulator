// SPDX-License-Identifier: CC0-1.0

//! What callers that RACE at this API's resource-scoped actions are answered.
//!
//! RULE: whether the account this emulator serves may perform a resource-scoped action of
//! this API is decided from that account's entitlement for this legacy surface alone —
//! never from the store, and never from the order in which callers arrive — so callers
//! racing at one action are each answered the same refusal (HTTP 403, error code
//! `AccessDeniedException`, message `Access Denied`) and none of them reaches the state
//! layer: there is no winner, nothing is created, changed, removed or sent. The
//! ACCOUNT-SCOPED listing names no resource, so it keeps answering what is stored under the
//! same contention. Where the account IS entitled the same contention is resolved by the
//! state layer instead, so exactly one racer creates a name and every other is the modelled
//! conflict — which is what shows the uniform refusal above comes from the entitlement
//! decision and not from a write path that refuses everyone.
//!
//! The suites drive the real `restJson1` router, so the requests here are the bytes an AWS
//! SDK `pinpoint-sms-voice` client sends, dispatched from one task per racer on a
//! multi-threaded runtime. The rows differ along what the rule generalises over: how many
//! callers race, which operation and method they race at, whether they all name ONE value
//! or one value each, whether the resource they name exists, and which payload they carry.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::state::{
    ConfigurationSet, EventDestinationRecord,
};
use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The HTTP status an authorization refusal carries.
const DENIED_STATUS: u16 = 403;
/// The error code an authorization refusal carries, which a client branches on.
const DENIED_CODE: &str = "AccessDeniedException";
/// The whole message an authorization refusal carries.
const DENIED_MESSAGE: &str = "Access Denied";

/// A configuration set the rows put in the store directly, so a racer that is refused
/// cannot be refused merely because the resource is absent: it IS there, and the request is
/// still refused.
const EXISTING_SET: &str = "race-existing-set";
/// An event destination of [`EXISTING_SET`], there for the same reason.
const EXISTING_DESTINATION: &str = "race-existing-dest";
/// A path naming a configuration set no request creates, so the action addresses nothing;
/// a row that uses it is refused for the same reason as one naming [`EXISTING_SET`].
const ABSENT_SET_PATH: &str = "/v1/sms-voice/configuration-sets/race-absent-set";

/// The emulator under test: the service mounted on the production router, over the one
/// shared engine the test also reads and seeds state through.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer: the status, the `x-amzn-errortype` header and the parsed JSON body
/// (`Value::Null` when empty).
struct Answer {
    status: u16,
    error_type_header: String,
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

    /// The detail text under the member this service reports it in.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// What the store holds that a resource-scoped action could have changed: the stored sets
/// with their destinations, and how many voice messages were accepted.
type Snapshot = (Vec<(String, Vec<String>)>, usize);

impl Harness {
    /// A harness whose account does NOT hold the entitlement — the default state, and what
    /// the racing suites drive.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suites drive unsigned requests; signature verification is the framework's own
        // concern and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Grant the account the entitlement this API's resource-scoped actions need. There is
    /// no operation in this model that grants it, so a suite that needs the actions to run
    /// states it in the store, exactly as the handlers read it.
    fn entitle(&self) {
        self.engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .legacy_surface_entitled = true;
    }

    /// Put a configuration set carrying one event destination in the store WITHOUT going
    /// through the API, so a racing request can name a resource that really exists.
    fn seed_set_with_destination(&self) {
        let mut set = ConfigurationSet::new(EXISTING_SET);
        set.event_destinations.insert(
            EXISTING_DESTINATION.to_string(),
            EventDestinationRecord {
                name: EXISTING_DESTINATION.to_string(),
                enabled: Some(true),
                ..EventDestinationRecord::default()
            },
        );
        self.engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .configuration_sets
            .insert(EXISTING_SET.to_string(), set);
    }

    /// Dispatch `racers` requests of `row` at once — one task per racer on the runtime's
    /// worker threads, all started before any answer is read — and report every answer in
    /// racer order.
    async fn race(&self, row: &Race) -> Vec<Answer> {
        let mut racing = Vec::with_capacity(row.racers);
        for racer in 0..row.racers {
            // The router is the production one and clones into each task; every clone
            // serves the SAME registered service over the one shared engine.
            let router = self.router.clone();
            let request = request(row.method, row.path, row.body.map(|build| build(racer)));
            racing.push(tokio::spawn(
                async move { answer(router.dispatch(request).await) },
            ));
        }

        let mut answers = Vec::with_capacity(row.racers);
        for (racer, task) in racing.into_iter().enumerate() {
            answers.push(
                task.await
                    .unwrap_or_else(|error| panic!("racer {racer} must answer, got {error:?}")),
            );
        }
        answers
    }

    /// One request, for the controls that need no contention.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        answer(self.router.dispatch(request(method, path, body)).await)
    }

    /// Everything a resource-scoped action could have changed, as one comparable value.
    fn snapshot(&self) -> Snapshot {
        let state = self
            .engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>();
        let store = state.lock();
        let sets = store
            .configuration_sets
            .iter()
            .map(|(name, set)| (name.clone(), set.event_destinations.keys().cloned().collect()))
            .collect();
        (sets, store.sent_messages.len())
    }
}

/// The bytes an SDK client sends for one call.
fn request(method: &str, path: &str, body: Option<Value>) -> http::Request<bytes::Bytes> {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(value.to_string())
    });
    http::Request::builder()
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
        .expect("the test request must build")
}

/// The parts of a response these suites assert on.
fn answer(response: http::Response<bytes::Bytes>) -> Answer {
    let status = response.status().as_u16();
    let error_type_header = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type_header,
        body,
    }
}

/// One action, raced at by `racers` callers at once.
struct Race {
    /// What makes this row different from the others.
    name: &'static str,
    /// How many callers arrive at once.
    racers: usize,
    method: &'static str,
    path: &'static str,
    /// The body, built per racer: a row either hands every racer the SAME value (the
    /// builder ignores its argument) or one value each.
    body: Option<fn(usize) -> Value>,
}

fn one_name_for_every_racer(_racer: usize) -> Value {
    json!({ "ConfigurationSetName": "race-contended-set" })
}

fn a_name_per_racer(racer: usize) -> Value {
    json!({ "ConfigurationSetName": format!("race-own-set-{racer}") })
}

fn the_existing_name(_racer: usize) -> Value {
    json!({ "ConfigurationSetName": EXISTING_SET })
}

fn one_destination_for_every_racer(_racer: usize) -> Value {
    json!({
        "EventDestinationName": "race-dest",
        "EventDestination": {
            "Enabled": true,
            "MatchingEventTypes": ["ANSWERED"],
            "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:voice-events" },
        },
    })
}

fn a_destination_per_racer(racer: usize) -> Value {
    json!({
        "EventDestinationName": format!("race-dest-{racer}"),
        "EventDestination": {
            "MatchingEventTypes": ["BUSY"],
            "KinesisFirehoseDestination": {
                "DeliveryStreamArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/voice",
                "IamRoleArn": "arn:aws:iam::000000000000:role/voice-firehose",
            },
        },
    })
}

fn a_definition_per_racer(racer: usize) -> Value {
    json!({ "EventDestination": { "Enabled": racer.is_multiple_of(2) } })
}

fn one_voice_message_for_every_racer(_racer: usize) -> Value {
    json!({
        "DestinationPhoneNumber": "+14255550182",
        "OriginationPhoneNumber": "+12065550100",
        "Content": { "PlainTextMessage": { "Text": "a call", "VoiceId": "Joanna" } },
    })
}

fn a_voice_message_per_racer(racer: usize) -> Value {
    json!({
        "ConfigurationSetName": EXISTING_SET,
        "DestinationPhoneNumber": format!("+1425555{:04}", 1000 + racer),
        "OriginationPhoneNumber": "+12065550100",
        "Content": { "CallInstructionsMessage": { "Text": "press 1" } },
    })
}

/// Every resource-scoped action of this API under contention: raced at by two, four and
/// eight callers, at one shared value and at a value per racer, on a resource that exists
/// and on one that never did.
const RACES: &[Race] = &[
    Race {
        name: "eight CreateConfigurationSet racers at one identical name",
        racers: 8,
        method: "POST",
        path: "/v1/sms-voice/configuration-sets",
        body: Some(one_name_for_every_racer),
    },
    Race {
        name: "four CreateConfigurationSet racers, each at a name of its own",
        racers: 4,
        method: "POST",
        path: "/v1/sms-voice/configuration-sets",
        body: Some(a_name_per_racer),
    },
    Race {
        name: "two CreateConfigurationSet racers at a name the store already holds",
        racers: 2,
        method: "POST",
        path: "/v1/sms-voice/configuration-sets",
        body: Some(the_existing_name),
    },
    Race {
        name: "four CreateConfigurationSetEventDestination racers at one destination name",
        racers: 4,
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/race-existing-set/event-destinations",
        body: Some(one_destination_for_every_racer),
    },
    Race {
        name: "four CreateConfigurationSetEventDestination racers under a set that does not exist",
        racers: 4,
        method: "POST",
        path: "/v1/sms-voice/configuration-sets/race-absent-set/event-destinations",
        body: Some(a_destination_per_racer),
    },
    Race {
        name: "four UpdateConfigurationSetEventDestination racers at one destination",
        racers: 4,
        method: "PUT",
        path: "/v1/sms-voice/configuration-sets/race-existing-set/event-destinations/race-existing-dest",
        body: Some(a_definition_per_racer),
    },
    Race {
        name: "eight DeleteConfigurationSetEventDestination racers at one destination",
        racers: 8,
        method: "DELETE",
        path: "/v1/sms-voice/configuration-sets/race-existing-set/event-destinations/race-existing-dest",
        body: None,
    },
    Race {
        name: "two GetConfigurationSetEventDestinations racers on a set that exists",
        racers: 2,
        method: "GET",
        path: "/v1/sms-voice/configuration-sets/race-existing-set/event-destinations",
        body: None,
    },
    Race {
        name: "four GetConfigurationSetEventDestinations racers on a set that does not exist",
        racers: 4,
        method: "GET",
        path: "/v1/sms-voice/configuration-sets/race-absent-set/event-destinations",
        body: None,
    },
    Race {
        name: "eight DeleteConfigurationSet racers at the set that exists",
        racers: 8,
        method: "DELETE",
        path: "/v1/sms-voice/configuration-sets/race-existing-set",
        body: None,
    },
    Race {
        name: "two DeleteConfigurationSet racers at a set that does not exist",
        racers: 2,
        method: "DELETE",
        path: "/v1/sms-voice/configuration-sets/race-absent-set",
        body: None,
    },
    Race {
        name: "eight SendVoiceMessage racers placing one identical call",
        racers: 8,
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(one_voice_message_for_every_racer),
    },
    Race {
        name: "four SendVoiceMessage racers, each placing a call of its own",
        racers: 4,
        method: "POST",
        path: "/v1/sms-voice/voice/message",
        body: Some(a_voice_message_per_racer),
    },
];

/// RULE: for an account without the entitlement, callers racing at ONE resource-scoped
/// action are each answered the same 403 `AccessDeniedException` / `Access Denied`, however
/// many they are and whatever they name, and the store is left exactly as it was — no racer
/// reaches the state layer, so none of them is a winner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_racer_at_a_resource_scoped_action_is_answered_the_same_refusal() {
    let harness = Harness::new();
    harness.seed_set_with_destination();
    let before = harness.snapshot();

    for row in RACES {
        let answers = harness.race(row).await;
        assert_eq!(
            answers.len(),
            row.racers,
            "case {:?}: expected one answer per racer",
            row.name
        );

        for (racer, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer.status, DENIED_STATUS,
                "case {:?} racer {racer}: expected HTTP {DENIED_STATUS}, got {} {:?}",
                row.name, answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                DENIED_CODE,
                "case {:?} racer {racer}: expected the code {DENIED_CODE}, got {:?}",
                row.name,
                answer.body
            );
            assert_eq!(
                answer.error_type_header, DENIED_CODE,
                "case {:?} racer {racer}: expected x-amzn-errortype {DENIED_CODE}, got {:?}",
                row.name, answer.error_type_header
            );
            assert_eq!(
                answer.message(),
                DENIED_MESSAGE,
                "case {:?} racer {racer}: expected the message {DENIED_MESSAGE:?}, got {:?}",
                row.name,
                answer.body
            );
        }

        // The uniformity is the point: a racer told anything another was not — a success, a
        // conflict, a 404 — would mean the decision depended on arrival order.
        let distinct: BTreeSet<(u16, &str, &str)> = answers
            .iter()
            .map(|answer| (answer.status, answer.error_code(), answer.message()))
            .collect();
        assert_eq!(
            distinct.len(),
            1,
            "case {:?}: every racer must be answered identically, got {distinct:?}",
            row.name
        );

        assert_eq!(
            harness.snapshot(),
            before,
            "case {:?}: a denied race must leave the store exactly as it was",
            row.name
        );
    }
}

/// RULE: the ACCOUNT-SCOPED listing names no resource, so the entitlement decision does not
/// cover it and it keeps answering what is stored — under the same contention that every
/// resource-scoped action is refused under.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_account_scoped_listing_answers_every_racer_what_is_stored() {
    let harness = Harness::new();
    harness.seed_set_with_destination();

    for racers in [2usize, 8] {
        let answers = harness
            .race(&Race {
                name: "ListConfigurationSets racers",
                racers,
                method: "GET",
                path: "/v1/sms-voice/configuration-sets",
                body: None,
            })
            .await;

        for (racer, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer.status, 200,
                "case {racers} racers, racer {racer}: the account-scoped listing must answer \
                 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body.get("ConfigurationSets"),
                Some(&json!([EXISTING_SET])),
                "case {racers} racers, racer {racer}: the listing must report the stored set, \
                 got {:?}",
                answer.body
            );
        }

        // The refusal is about the ACTION, not the credentials: a resource-scoped action
        // raced at by the same callers is still refused.
        let denied = harness.call("DELETE", ABSENT_SET_PATH, None).await;
        assert_eq!(
            denied.status, DENIED_STATUS,
            "case {racers} racers: the resource-scoped control must stay refused, got {} {:?}",
            denied.status, denied.body
        );
        assert_eq!(
            denied.error_code(),
            DENIED_CODE,
            "case {racers} racers: the resource-scoped control must stay {DENIED_CODE}, got {:?}",
            denied.body
        );
    }
}

/// RULE: where the account IS entitled the entitlement decision lets every racer through and
/// contention is resolved by the state layer instead, so a name is created once: exactly one
/// racer succeeds, every other is the modelled conflict (`AlreadyExistsException`, HTTP 409),
/// and the store holds that one set.
///
/// This is the control for the refusal above: it is what distinguishes a gate that refuses an
/// unentitled account from a write path that refuses everybody.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_entitled_account_resolves_the_same_contention_in_the_state_layer() {
    for racers in [2usize, 8] {
        let harness = Harness::new();
        harness.entitle();

        let answers = harness
            .race(&Race {
                name: "CreateConfigurationSet racers at one identical name",
                racers,
                method: "POST",
                path: "/v1/sms-voice/configuration-sets",
                body: Some(one_name_for_every_racer),
            })
            .await;

        let winners = answers
            .iter()
            .filter(|answer| answer.status == 200)
            .count();
        assert_eq!(
            winners,
            1,
            "case {racers} racers: exactly one racer may create the name, got {winners} of {:?}",
            answers
                .iter()
                .map(|answer| (answer.status, answer.error_code().to_string()))
                .collect::<Vec<_>>()
        );

        for (racer, answer) in answers
            .iter()
            .enumerate()
            .filter(|(_, answer)| answer.status != 200)
        {
            assert_eq!(
                answer.status, 409,
                "case {racers} racers, racer {racer}: a loser must be the modelled conflict, \
                 got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "AlreadyExistsException",
                "case {racers} racers, racer {racer}: expected AlreadyExistsException, got {:?}",
                answer.body
            );
        }

        assert_eq!(
            harness.snapshot(),
            (vec![("race-contended-set".to_string(), Vec::new())], 0),
            "case {racers} racers: the contended name must be stored exactly once"
        );
    }
}
