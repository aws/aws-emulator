// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the OpenSearch Memory *space* and *event* slice:
//! `CreateSpace`, `GetSpace`, `CreateEvent`, `GetEvent`, `ListEvents` and
//! `DeleteSpace`.
//!
//! Every case drives the service through the same `MultiServiceRouter` the emulator
//! mounts, so each request takes the production walk (deserialize → validate → handle →
//! serialize) over the awsJson1_0 wire.
//!
//! Two kinds of timing appear here, and neither depends on how fast the host runs:
//! the lifecycle cases POLL for the status move the clock owes until it holds or a
//! generous deadline passes, and every other case brings the pending move due through
//! the store itself so the settled state is observed without any waiting.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_opensearchmemory_opensearchmemory::state as osm_state;
use service_com_amazonaws_opensearchmemory_opensearchmemory::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for. Deliberately NOT the emulator's
/// `us-east-1` fallback, so a stored ARN proves it came from the request.
const REGION: &str = "us-west-2";

/// How long a poll loop waits for a move the clock owes
/// (`osm_state::TRANSITION_SPAN`) before giving up. Deliberately far larger than the
/// span, so a loaded machine cannot fail it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll loop pauses between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `2026-01-01T00:00:00Z` as the epoch seconds an `EventTimestamp` carries on this
/// protocol. The listing cases below step one second at a time from here, so ordering
/// is decided by the timestamp and by nothing else.
const BASE_EPOCH_SECONDS: i64 = 1_767_225_600;

/// The ARN this service must mint for `space_id`: `SpaceArn`'s modelled pattern
/// `^arn:aws:osm:[a-z0-9-]+:[0-9]{12}:space/sp-[a-f0-9]{12}$`.
fn expected_arn(space_id: &str) -> String {
    format!(
        "arn:aws:osm:{REGION}:{}:space/{space_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Whether `candidate` satisfies `SpaceId`'s modelled `^sp-[a-f0-9]{12}$`.
fn is_space_id(candidate: &str) -> bool {
    let Some(hex) = candidate.strip_prefix("sp-") else {
        return false;
    };
    hex.len() == 12 && hex.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// One emulator with the OpenSearch Memory service mounted on a shared engine.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One answered request.
struct Call {
    status: u16,
    body: Value,
    error_type_header: Option<String>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not under test here; the request still carries a SigV4
        // credential scope, because that is what routes it and names its region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, operation: &str, body: Value) -> Call {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("OpenSearchMemory.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTOPENSEARCHMEMORY/20261127/{REGION}/osm/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("serialize the request body"),
            ))
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type_header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let payload = response.body();
        let body = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(payload).unwrap_or(Value::Null)
        };
        Call {
            status,
            body,
            error_type_header,
        }
    }

    /// Bring every status move the clock owes due, the way the passage of time would.
    ///
    /// The due instant lives on the record, so moving it into the past is exactly what
    /// the clock reaching it does: the next operation materialises the move. This is
    /// what keeps the cases that are not ABOUT the span free of any waiting.
    fn bring_pending_due(&self) {
        let state = osm_state::service_state(&*self.engine).expect("the service state");
        let mut store = state.lock();
        for space in store.spaces.values_mut() {
            if let Some(pending) = space.pending.as_mut() {
                pending.due = SystemTime::UNIX_EPOCH;
            }
        }
    }

    /// The tags a space holds in the store. `ListTagsForResource` is still scaffolded,
    /// so the store is where a test can see that `CreateSpace` persisted them.
    fn stored_tags(&self, space_id: &str) -> BTreeMap<String, String> {
        let state = osm_state::service_state(&*self.engine).expect("the service state");
        let store = state.lock();
        store
            .spaces
            .get(space_id)
            .map(|space| {
                space
                    .tags
                    .iter()
                    .map(|tag| (tag.key.clone(), tag.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `CreateSpace` that must succeed, answered and then settled to `ACTIVE` without
    /// waiting.
    async fn active_space(&self, case: &str, request: Value) -> Value {
        let created = self.call("CreateSpace", request).await;
        let body = created.ok(case).clone();
        self.bring_pending_due();
        body
    }

    /// The `spaceId` of a freshly created, `ACTIVE` space named `name`.
    async fn active_space_id(&self, case: &str, name: &str) -> String {
        self.active_space(case, json!({"name": name})).await["spaceId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    async fn get_space(&self, space_id: &str) -> Call {
        self.call("GetSpace", json!({"spaceId": space_id})).await
    }

    /// Look at `GetSpace` until it reports `want`, or until [`SETTLE_DEADLINE`] passes.
    async fn poll_until_status(&self, case: &str, space_id: &str, want: &str) {
        let started = Instant::now();
        loop {
            let got = self.get_space(space_id).await;
            if got.status == 200 && got.body["status"].as_str() == Some(want) {
                return;
            }
            assert!(
                started.elapsed() < SETTLE_DEADLINE,
                "case {case:?}: GetSpace never reported status {want}; \
                 last answer was HTTP {} with body {}",
                got.status,
                got.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Look at `GetSpace` until the space is gone, or until [`SETTLE_DEADLINE`] passes.
    async fn poll_until_gone(&self, case: &str, space_id: &str) {
        let started = Instant::now();
        loop {
            let got = self.get_space(space_id).await;
            if got.status == 404 {
                assert_eq!(
                    got.code(),
                    "ResourceNotFoundException",
                    "case {case:?}: a purged space must answer ResourceNotFoundException, \
                     got {}",
                    got.body
                );
                return;
            }
            assert!(
                started.elapsed() < SETTLE_DEADLINE,
                "case {case:?}: GetSpace still reports the deleted space; \
                 last answer was HTTP {} with body {}",
                got.status,
                got.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

impl Call {
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            self.status, self.body
        );
        &self.body
    }

    /// The wire error code, from the body's `__type` or the error-type header.
    fn code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .or(self.error_type_header.as_deref())
            .map(|wire| wire.split('#').next_back().unwrap_or(wire).to_string())
            .unwrap_or_default()
    }

    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn assert_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.code().as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got HTTP {} {:?} with body {}",
            self.status,
            self.code(),
            self.body
        );
    }
}

/// A `structuredData` payload carrying `content`.
fn structured_payload(content: Value) -> Value {
    json!({"structuredData": {"source": {"json": content}}})
}

/// A `conversational` payload carrying one `role`/`text` turn.
fn conversational_payload(role: &str, text: &str) -> Value {
    json!({"conversational": {"unified": {"source": {"object": {
        "messages": [{"role": role, "content": {"text": text}}]
    }}}}})
}

/// A `blob` payload naming an S3 object of `content_type`.
fn blob_payload(content_type: &str) -> Value {
    json!({"blob": {
        "contentType": content_type,
        "source": {"s3Location": {"uri": "s3://memory-docs/runbook.pdf"}}
    }})
}

/// The whole happy path the bootstrap plan names, in order, with the `CREATING` ->
/// `ACTIVE` move the clock owes left to the clock.
#[tokio::test]
async fn space_and_event_lifecycle_answers_from_the_store() {
    let case = "lifecycle";
    let emulator = Emulator::new();

    let request = json!({
        "name": "agent memory",
        "eventExpiryDays": 30,
        "memoryExpiryDays": 180,
        "extractionModelId": "anthropic.claude-haiku-5-5",
        "encryptionConfiguration": {"kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/k1"},
        "memoryStrategies": [{"type": "SEMANTIC"}, {"type": "SUMMARY", "name": "weekly"}],
        "tags": [{"key": "team", "value": "agents"}],
    });
    let created = emulator.call("CreateSpace", request.clone()).await;
    let body = created.ok(case).clone();

    let space_id = body["spaceId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_space_id(&space_id),
        "case {case:?}: spaceId must match ^sp-[a-f0-9]{{12}}$, got {:?}",
        body["spaceId"]
    );
    assert_eq!(
        body["arn"],
        json!(expected_arn(&space_id)),
        "case {case:?}: arn must name the signed region, the emulator account and the \
         minted spaceId, got {}",
        body["arn"]
    );
    assert_eq!(
        body["name"], request["name"],
        "case {case:?}: name must come from the stored record, got {body}"
    );
    assert_eq!(
        body["status"], "CREATING",
        "case {case:?}: CreateSpace returns before provisioning finishes, got {body}"
    );
    assert!(
        body["createdAt"].as_f64().unwrap_or_default() > 1_600_000_000.0,
        "case {case:?}: createdAt must be a real epoch-seconds value, got {}",
        body["createdAt"]
    );

    // The move to ACTIVE is owed by the CLOCK: nothing here asks for it.
    emulator.poll_until_status(case, &space_id, "ACTIVE").await;

    let described = emulator.get_space(&space_id).await;
    let space = described.ok(case).clone();
    for member in ["spaceId", "arn", "name", "createdAt"] {
        assert_eq!(
            space[member], body[member],
            "case {case:?}: GetSpace must report the same {member} CreateSpace did, \
             got {space}"
        );
    }
    assert_eq!(
        space["eventExpiryDays"], request["eventExpiryDays"],
        "case {case:?}: eventExpiryDays must round-trip, got {space}"
    );
    assert_eq!(
        space["memoryExpiryDays"], request["memoryExpiryDays"],
        "case {case:?}: memoryExpiryDays must round-trip, got {space}"
    );
    assert_eq!(
        space["extractionModelId"], request["extractionModelId"],
        "case {case:?}: extractionModelId must round-trip, got {space}"
    );
    assert_eq!(
        space["encryptionConfiguration"], request["encryptionConfiguration"],
        "case {case:?}: encryptionConfiguration must round-trip, got {space}"
    );
    // The `name` member's own doc: a strategy that omits one has the service "generate
    // one from the type, for example semantic_1".
    assert_eq!(
        space["memoryStrategies"],
        json!([
            {"type": "SEMANTIC", "name": "semantic_1"},
            {"type": "SUMMARY", "name": "weekly"},
        ]),
        "case {case:?}: every stored strategy carries a name, generated where the \
         request omitted it, got {}",
        space["memoryStrategies"]
    );
    assert_eq!(
        emulator.stored_tags(&space_id),
        BTreeMap::from([("team".to_string(), "agents".to_string())]),
        "case {case:?}: CreateSpace must persist the tags it was given"
    );

    // CreateEvent: the service owns the eventId, the sessionId and the timestamp.
    let payload = structured_payload(json!({"deployment": "ok", "attempts": 2}));
    let submitted = emulator
        .call(
            "CreateEvent",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "payload": payload,
                "metadata": {"k": "v"},
            }),
        )
        .await;
    let detail = submitted.ok(case)["event"].clone();
    let event_id = detail["eventId"].as_str().unwrap_or_default().to_string();
    let session_id = detail["sessionId"].as_str().unwrap_or_default().to_string();
    assert!(
        !event_id.is_empty(),
        "case {case:?}: CreateEvent must assign an eventId, got {detail}"
    );
    assert!(
        !session_id.is_empty(),
        "case {case:?}: CreateEvent must generate a sessionId when the request omits \
         one, got {detail}"
    );
    assert_eq!(
        detail["spaceId"], json!(space_id),
        "case {case:?}: the event must report its space, got {detail}"
    );
    assert_eq!(
        detail["actorId"], "actor1",
        "case {case:?}: the event must report its actor, got {detail}"
    );
    assert!(
        !detail["eventTimestamp"].is_null(),
        "case {case:?}: eventTimestamp defaults to the receive time, got {detail}"
    );

    // GetEvent: the payload and the metadata exactly as submitted.
    let fetched = emulator
        .call(
            "GetEvent",
            json!({"spaceId": space_id, "eventId": event_id}),
        )
        .await;
    let event = fetched.ok(case)["event"].clone();
    assert_eq!(
        event["payload"], payload,
        "case {case:?}: GetEvent must report the payload as submitted, got {event}"
    );
    assert_eq!(
        event["metadata"],
        json!({"k": "v"}),
        "case {case:?}: GetEvent must report the metadata as submitted, got {event}"
    );
    assert_eq!(
        event["eventTimestamp"], detail["eventTimestamp"],
        "case {case:?}: one event has one timestamp, whichever operation reports it, \
         got {event}"
    );
    for member in ["eventId", "spaceId", "actorId", "sessionId"] {
        assert_eq!(
            event[member], detail[member],
            "case {case:?}: GetEvent and CreateEvent must agree on {member}, got {event}"
        );
    }

    // Two more events of the same actor and session, then a two-page listing.
    for text in ["second", "third"] {
        emulator
            .call(
                "CreateEvent",
                json!({
                    "spaceId": space_id,
                    "actorId": "actor1",
                    "sessionId": session_id,
                    "payload": conversational_payload("USER", text),
                }),
            )
            .await
            .ok(case);
    }

    let first_page = emulator
        .call(
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": session_id,
                "sortOrder": "ASC",
                "maxResults": 2,
            }),
        )
        .await;
    let page = first_page.ok(case).clone();
    assert_eq!(
        page["events"].as_array().map(Vec::len),
        Some(2),
        "case {case:?}: maxResults 2 must serve two summaries, got {page}"
    );
    let next_token = page["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !next_token.is_empty(),
        "case {case:?}: a listing with a third event left must hand out a nextToken, \
         got {page}"
    );
    assert_eq!(
        page["events"][0]["eventId"], json!(event_id),
        "case {case:?}: ASC must start with the oldest event, got {page}"
    );
    assert_eq!(
        page["events"][0]["metadata"],
        json!({"k": "v"}),
        "case {case:?}: an EventSummary carries the event's metadata, got {page}"
    );

    let second_page = emulator
        .call(
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": session_id,
                "sortOrder": "ASC",
                "maxResults": 2,
                "nextToken": next_token,
            }),
        )
        .await;
    let page = second_page.ok(case).clone();
    assert_eq!(
        page["events"].as_array().map(Vec::len),
        Some(1),
        "case {case:?}: the second page must continue where the first stopped, got {page}"
    );
    assert!(
        page["nextToken"].is_null(),
        "case {case:?}: an exhausted listing must hand out no nextToken, got {page}"
    );

    // An unknown space is a 404, never a 501 and never an empty success.
    let unknown = emulator.get_space("sp-000000000000").await;
    unknown.assert_error(case, 404, "ResourceNotFoundException");

    // DeleteSpace starts the delete; the purge is owed by the clock.
    let deleted = emulator
        .call("DeleteSpace", json!({"spaceId": space_id}))
        .await;
    let body = deleted.ok(case).clone();
    assert_eq!(
        body,
        json!({"spaceId": space_id, "status": "DELETING"}),
        "case {case:?}: DeleteSpace returns the space and status DELETING, got {body}"
    );
    emulator
        .call(
            "GetEvent",
            json!({"spaceId": space_id, "eventId": event_id}),
        )
        .await
        .assert_error(case, 404, "ResourceNotFoundException");
    emulator.poll_until_gone(case, &space_id).await;
    emulator
        .call(
            "GetEvent",
            json!({"spaceId": space_id, "eventId": event_id}),
        )
        .await
        .assert_error(case, 404, "ResourceNotFoundException");
}

/// Every configuration member a `CreateSpace` request can carry comes back out of
/// `GetSpace`, and one it omits comes back absent — across the whole class of requests,
/// not only the one the plan quotes.
#[tokio::test]
async fn get_space_reports_every_persisted_configuration_member() {
    // `eventExpiryDays` range 1..365 and `memoryExpiryDays` range 0..3650 are the
    // model's own (`smithy_query query="operation" operation="CreateSpace"`), so the
    // rows below walk both published bounds as well as a middle value.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "only the required name",
            json!({"name": "minimal"}),
            json!({
                "name": "minimal",
                "memoryStrategies": [],
                "eventExpiryDays": null,
                "memoryExpiryDays": null,
                "extractionModelId": null,
                "encryptionConfiguration": null,
            }),
        ),
        (
            "lowest expiry bounds",
            json!({"name": "lowbounds", "eventExpiryDays": 1, "memoryExpiryDays": 0}),
            json!({
                "name": "lowbounds",
                "memoryStrategies": [],
                "eventExpiryDays": 1,
                "memoryExpiryDays": 0,
                "extractionModelId": null,
                "encryptionConfiguration": null,
            }),
        ),
        (
            "highest expiry bounds",
            json!({"name": "highbounds", "eventExpiryDays": 365, "memoryExpiryDays": 3650}),
            json!({
                "name": "highbounds",
                "memoryStrategies": [],
                "eventExpiryDays": 365,
                "memoryExpiryDays": 3650,
                "extractionModelId": null,
                "encryptionConfiguration": null,
            }),
        ),
        (
            "an explicitly empty strategy list",
            json!({"name": "nostrategies", "memoryStrategies": []}),
            json!({
                "name": "nostrategies",
                "memoryStrategies": [],
                "eventExpiryDays": null,
                "memoryExpiryDays": null,
                "extractionModelId": null,
                "encryptionConfiguration": null,
            }),
        ),
        (
            "one strategy of every type, names generated",
            json!({
                "name": "everytype",
                "memoryStrategies": [
                    {"type": "SEMANTIC"},
                    {"type": "SUMMARY"},
                    {"type": "CUSTOM", "configuration": {"extractionPrompt": "remember releases"}},
                ],
            }),
            json!({
                "name": "everytype",
                "memoryStrategies": [
                    {"type": "SEMANTIC", "name": "semantic_1"},
                    {"type": "SUMMARY", "name": "summary_1"},
                    {"type": "CUSTOM", "name": "custom_1",
                     "configuration": {"extractionPrompt": "remember releases"}},
                ],
                "eventExpiryDays": null,
                "memoryExpiryDays": null,
                "extractionModelId": null,
                "encryptionConfiguration": null,
            }),
        ),
        (
            "a generated name steps past the one the request already took",
            json!({
                "name": "mixednames",
                "memoryStrategies": [
                    {"type": "SEMANTIC", "name": "semantic_1"},
                    {"type": "SEMANTIC", "description": "the other one"},
                ],
            }),
            json!({
                "name": "mixednames",
                "memoryStrategies": [
                    {"type": "SEMANTIC", "name": "semantic_1"},
                    {"type": "SEMANTIC", "name": "semantic_2", "description": "the other one"},
                ],
                "eventExpiryDays": null,
                "memoryExpiryDays": null,
                "extractionModelId": null,
                "encryptionConfiguration": null,
            }),
        ),
        (
            "a model id the doc lists, with a customer key",
            json!({
                "name": "pinnedmodel",
                "extractionModelId": "anthropic.claude-opus-5-5",
                "encryptionConfiguration": {"kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/k9"},
            }),
            json!({
                "name": "pinnedmodel",
                "memoryStrategies": [],
                "eventExpiryDays": null,
                "memoryExpiryDays": null,
                "extractionModelId": "anthropic.claude-opus-5-5",
                "encryptionConfiguration": {"kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/k9"},
            }),
        ),
    ];

    let emulator = Emulator::new();
    for (case, request, expected) in rows {
        let created = emulator.active_space(case, request).await;
        let space_id = created["spaceId"].as_str().unwrap_or_default().to_string();
        let described = emulator.get_space(&space_id).await;
        let space = described.ok(case).clone();

        assert_eq!(
            space["status"], "ACTIVE",
            "case {case:?}: a settled space is ACTIVE, got {space}"
        );
        assert_eq!(
            space["spaceId"], created["spaceId"],
            "case {case:?}: GetSpace must report the stored spaceId, got {space}"
        );
        assert_eq!(
            space["arn"],
            json!(expected_arn(&space_id)),
            "case {case:?}: arn must name the minted spaceId, got {}",
            space["arn"]
        );
        for (member, want) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                &space[member], want,
                "case {case:?}: expected {member} = {want}, got {}",
                space[member]
            );
        }
    }
}

/// The maximum number of strategies the model allows is stored, each with its own
/// generated name.
#[tokio::test]
async fn create_space_stores_the_maximum_strategy_list() {
    let case = "ten strategies";
    let emulator = Emulator::new();
    let strategies: Vec<Value> = (0..osm_state::MAX_MEMORY_STRATEGIES)
        .map(|_| json!({"type": "SEMANTIC"}))
        .collect();
    let created = emulator
        .active_space(case, json!({"name": "maxstrategies", "memoryStrategies": strategies}))
        .await;
    let space_id = created["spaceId"].as_str().unwrap_or_default().to_string();
    let described = emulator.get_space(&space_id).await;
    let space = described.ok(case).clone();

    let stored = space["memoryStrategies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        stored.len(),
        osm_state::MAX_MEMORY_STRATEGIES,
        "case {case:?}: every requested strategy must be stored, got {space}"
    );
    let names: Vec<&str> = stored
        .iter()
        .map(|strategy| strategy["name"].as_str().unwrap_or_default())
        .collect();
    let expected: Vec<String> = (1..=osm_state::MAX_MEMORY_STRATEGIES)
        .map(|ordinal| format!("semantic_{ordinal}"))
        .collect();
    assert_eq!(
        names, expected,
        "case {case:?}: each generated name must be unique within the space, got {names:?}"
    );
}

/// `ListEvents` orders, filters and pages the stored events: a table over the sort order
/// and the page size, plus the time window and the metadata filter.
#[tokio::test]
async fn list_events_orders_filters_and_pages_the_store() {
    let setup = "listing setup";
    let emulator = Emulator::new();
    let space_id = emulator.active_space_id(setup, "listingspace").await;

    // Five events of one actor and session, at one-second steps, so ordering is
    // decided by `eventTimestamp` and not by insertion order. They are submitted out
    // of time order on purpose.
    let offsets = [3_i64, 1, 5, 2, 4];
    let mut by_offset: BTreeMap<i64, String> = BTreeMap::new();
    for (index, offset) in offsets.iter().enumerate() {
        let created = emulator
            .call(
                "CreateEvent",
                json!({
                    "spaceId": space_id,
                    "actorId": "actor1",
                    "sessionId": "session1",
                    "eventTimestamp": BASE_EPOCH_SECONDS + offset,
                    "payload": structured_payload(json!({"step": index})),
                    "metadata": {"phase": if index % 2 == 0 { "even" } else { "odd" }},
                }),
            )
            .await;
        let event_id = created.ok(setup)["event"]["eventId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        by_offset.insert(*offset, event_id);
    }
    // Another actor and another session, which no listing of actor1/session1 may show.
    for (actor, session) in [("actor2", "session1"), ("actor1", "session2")] {
        emulator
            .call(
                "CreateEvent",
                json!({
                    "spaceId": space_id,
                    "actorId": actor,
                    "sessionId": session,
                    "eventTimestamp": BASE_EPOCH_SECONDS + 9,
                    "payload": structured_payload(json!({"other": true})),
                }),
            )
            .await
            .ok(setup);
    }

    let ascending: Vec<String> = by_offset.values().cloned().collect();
    let descending: Vec<String> = ascending.iter().rev().cloned().collect();

    // Order x page size: paging all the way through must enumerate exactly the ordering
    // the request asked for, whatever the page size.
    //
    // One row: what to call it, the `sortOrder` to send (or none), the `maxResults` to
    // send (or none), and the whole ordering paging through must enumerate.
    type ListingRow = (String, Option<&'static str>, Option<i64>, Vec<String>);
    let mut rows: Vec<ListingRow> = Vec::new();
    for (order, expected) in [
        (None, &descending),
        (Some("DESC"), &descending),
        (Some("ASC"), &ascending),
    ] {
        for page in [None, Some(1), Some(2), Some(5), Some(100)] {
            rows.push((
                format!("order {order:?} page {page:?}"),
                order,
                page,
                expected.clone(),
            ));
        }
    }

    for (case, order, page, expected) in rows {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
            });
            if let Some(order) = order {
                request["sortOrder"] = json!(order);
            }
            if let Some(page) = page {
                request["maxResults"] = json!(page);
            }
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let listed = emulator.call("ListEvents", request).await;
            let body = listed.ok(&case).clone();
            for event in body["events"].as_array().cloned().unwrap_or_default() {
                seen.push(event["eventId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging did not terminate, saw {seen:?}"
            );
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must enumerate the requested ordering exactly"
        );
    }

    // The time window is inclusive at both ends.
    let window = emulator
        .call(
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "sortOrder": "ASC",
                "eventTimestampAfter": BASE_EPOCH_SECONDS + 2,
                "eventTimestampBefore": BASE_EPOCH_SECONDS + 4,
            }),
        )
        .await;
    let body = window.ok("inclusive window").clone();
    let ids: Vec<&str> = body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .map(|event| event["eventId"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![
            by_offset[&2].as_str(),
            by_offset[&3].as_str(),
            by_offset[&4].as_str(),
        ],
        "case {:?}: both bounds are inclusive, got {body}",
        "inclusive window"
    );

    // A metadata filter keeps only events whose metadata contains every pair.
    let filtered = emulator
        .call(
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "sortOrder": "ASC",
                "metadataFilters": {"phase": "odd"},
            }),
        )
        .await;
    let body = filtered.ok("metadata filter").clone();
    let ids: Vec<&str> = body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .map(|event| event["eventId"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![
            by_offset[&1].as_str(),
            by_offset[&2].as_str(),
        ],
        "case {:?}: only events carrying every filter pair are returned, got {body}",
        "metadata filter"
    );

    // A cursor names a position in ONE listing: one issued for the ascending order is
    // not a position in the descending one.
    let ascending_page = emulator
        .call(
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "sortOrder": "ASC",
                "maxResults": 2,
            }),
        )
        .await;
    let token = ascending_page.ok("cross-order cursor")["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    emulator
        .call(
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "sortOrder": "DESC",
                "maxResults": 2,
                "nextToken": token,
            }),
        )
        .await
        .assert_error("cross-order cursor", 400, "ValidationException");
}

/// `GetEvent`'s optional `actorId` / `sessionId` are a check, not a lookup key: a value
/// that does not match the stored event answers as if the event did not exist.
#[tokio::test]
async fn get_event_checks_the_actor_and_session_it_is_given() {
    let case = "get event checks";
    let emulator = Emulator::new();
    let space_id = emulator.active_space_id(case, "checkedreads").await;
    let created = emulator
        .call(
            "CreateEvent",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "payload": blob_payload("application/pdf"),
            }),
        )
        .await;
    let event_id = created.ok(case)["event"]["eventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let rows: Vec<(&str, Value, Option<u16>)> = vec![
        ("no check at all", json!({}), None),
        ("the matching actor", json!({"actorId": "actor1"}), None),
        ("the matching session", json!({"sessionId": "session1"}), None),
        (
            "both matching",
            json!({"actorId": "actor1", "sessionId": "session1"}),
            None,
        ),
        (
            "another actor",
            json!({"actorId": "actor2"}),
            Some(404),
        ),
        (
            "another session",
            json!({"sessionId": "session2"}),
            Some(404),
        ),
        (
            "the right actor in the wrong session",
            json!({"actorId": "actor1", "sessionId": "session2"}),
            Some(404),
        ),
    ];

    for (case, extra, refused) in rows {
        let mut request = json!({"spaceId": space_id, "eventId": event_id});
        for (member, value) in extra.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }
        let got = emulator.call("GetEvent", request).await;
        match refused {
            None => {
                assert_eq!(
                    got.ok(case)["event"]["eventId"],
                    json!(event_id),
                    "case {case:?}: a matching check must still answer the event"
                );
            }
            Some(status) => got.assert_error(case, status, "ResourceNotFoundException"),
        }
    }
}

/// Every refusal this slice words, as a table: the status and the error code a client
/// branches on.
#[tokio::test]
async fn refusals_carry_the_modelled_status_and_code() {
    let setup = "refusal setup";
    let emulator = Emulator::new();
    let space_id = emulator.active_space_id(setup, "refusalspace").await;
    let created = emulator
        .call(
            "CreateEvent",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "payload": structured_payload(json!({"a": 1})),
            }),
        )
        .await;
    let event_id = created.ok(setup)["event"]["eventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A space that is still provisioning: its `CREATING` move is deliberately NOT
    // brought due, so these rows observe the transient state the span exists for.
    let provisioning = emulator
        .call("CreateSpace", json!({"name": "stillcreating"}))
        .await;
    let provisioning_id = provisioning.ok(setup)["spaceId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "GetSpace on an id nothing minted",
            "GetSpace",
            json!({"spaceId": "sp-000000000000"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteSpace on an id nothing minted",
            "DeleteSpace",
            json!({"spaceId": "sp-000000000000"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateEvent in a space nothing minted",
            "CreateEvent",
            json!({
                "spaceId": "sp-000000000000",
                "actorId": "actor1",
                "payload": structured_payload(json!({"a": 1})),
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetEvent in a space nothing minted",
            "GetEvent",
            json!({"spaceId": "sp-000000000000", "eventId": "ev-000000000001"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "ListEvents in a space nothing minted",
            "ListEvents",
            json!({
                "spaceId": "sp-000000000000",
                "actorId": "actor1",
                "sessionId": "session1",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetEvent for an eventId the space never issued",
            "GetEvent",
            json!({"spaceId": space_id, "eventId": "ev-ffffffffffff"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateSpace reusing a name the account already holds",
            "CreateSpace",
            json!({"name": "refusalspace"}),
            409,
            "ConflictException",
        ),
        (
            "CreateEvent while the space is still provisioning",
            "CreateEvent",
            json!({
                "spaceId": provisioning_id,
                "actorId": "actor1",
                "payload": structured_payload(json!({"a": 1})),
            }),
            409,
            "ConflictException",
        ),
        (
            "DeleteSpace while the space is still provisioning",
            "DeleteSpace",
            json!({"spaceId": provisioning_id}),
            409,
            "ConflictException",
        ),
        (
            "GetEvent in a space that is still provisioning",
            "GetEvent",
            json!({"spaceId": provisioning_id, "eventId": "ev-000000000001"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "ListEvents in a space that is still provisioning",
            "ListEvents",
            json!({
                "spaceId": provisioning_id,
                "actorId": "actor1",
                "sessionId": "session1",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateSpace naming a model id the doc does not list",
            "CreateSpace",
            json!({"name": "badmodel", "extractionModelId": "anthropic.claude-sonnet-1-0"}),
            400,
            "ValidationException",
        ),
        (
            "CreateSpace with a CUSTOM strategy and no extraction prompt",
            "CreateSpace",
            json!({"name": "nocustomprompt", "memoryStrategies": [{"type": "CUSTOM"}]}),
            400,
            "ValidationException",
        ),
        (
            "CreateSpace with two strategies of the same name",
            "CreateSpace",
            json!({
                "name": "duplicatenames",
                "memoryStrategies": [
                    {"type": "SEMANTIC", "name": "shared"},
                    {"type": "SUMMARY", "name": "shared"},
                ],
            }),
            400,
            "ValidationException",
        ),
        (
            "CreateEvent with a metadata key in the reserved prefix",
            "CreateEvent",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "payload": structured_payload(json!({"a": 1})),
                "metadata": {"x-amz-internal": "no"},
            }),
            400,
            "ValidationException",
        ),
        (
            "CreateEvent with a reserved metadata key",
            "CreateEvent",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "payload": structured_payload(json!({"a": 1})),
                "metadata": {"documentS3Url": "no"},
            }),
            400,
            "ValidationException",
        ),
        (
            "CreateEvent with a blob media type the service does not support",
            "CreateEvent",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "payload": blob_payload("image/png"),
            }),
            400,
            "ValidationException",
        ),
        (
            "ListEvents with the window the wrong way round",
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "eventTimestampAfter": BASE_EPOCH_SECONDS + 86_400,
                "eventTimestampBefore": BASE_EPOCH_SECONDS,
            }),
            400,
            "ValidationException",
        ),
        (
            "ListEvents with a nextToken this service never issued",
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "nextToken": "not-a-cursor",
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, operation, request, status, code) in rows {
        emulator
            .call(operation, request)
            .await
            .assert_error(case, status, code);
    }

    // The event created above is still readable: none of the refusals above changed the
    // store.
    let fetched = emulator
        .call(
            "GetEvent",
            json!({"spaceId": space_id, "eventId": event_id}),
        )
        .await;
    assert_eq!(
        fetched.ok("refusals left the store alone")["event"]["eventId"],
        json!(event_id),
        "a refused request must not disturb the stored events"
    );

    // The unsupported-model refusal lists the ids the doc names, so a caller can fix the
    // request from the message alone — and every one it lists is accepted.
    let refused = emulator
        .call(
            "CreateSpace",
            json!({"name": "badmodel2", "extractionModelId": "gpt-not-bedrock"}),
        )
        .await;
    for supported in osm_state::SUPPORTED_EXTRACTION_MODEL_IDS {
        assert!(
            refused.message().contains(supported),
            "the refusal must list the supported model id {supported}, got {:?}",
            refused.message()
        );
        let case = format!("supported model {supported}");
        let created = emulator
            .active_space(
                &case,
                json!({
                    "name": supported.replace(['.', '-'], "_"),
                    "extractionModelId": supported,
                }),
            )
            .await;
        let space_id = created["spaceId"].as_str().unwrap_or_default().to_string();
        let described = emulator.get_space(&space_id).await;
        assert_eq!(
            described.ok(&case)["extractionModelId"],
            json!(supported),
            "case {case:?}: a supported model id must be stored and reported back"
        );
    }
}

/// An `@idempotencyToken` replay answers the result the FIRST request produced, and the
/// same token with different parameters conflicts.
#[tokio::test]
async fn client_tokens_replay_the_first_result() {
    let case = "space token replay";
    let emulator = Emulator::new();

    let request = json!({"name": "tokenspace", "eventExpiryDays": 7, "clientToken": "tok-space-1"});
    let first = emulator.call("CreateSpace", request.clone()).await;
    let first_body = first.ok(case).clone();
    let replay = emulator.call("CreateSpace", request.clone()).await;
    assert_eq!(
        replay.ok(case),
        &first_body,
        "case {case:?}: a replay must answer the space the first request created"
    );

    // One space, not two: the name is unique within the account, so a second create
    // would have conflicted anyway — the replay did not create one.
    let mut reordered = request.clone();
    reordered["eventExpiryDays"] = json!(8);
    emulator
        .call("CreateSpace", reordered)
        .await
        .assert_error(
            "space token with different parameters",
            409,
            "ConflictException",
        );

    emulator.bring_pending_due();
    let space_id = first_body["spaceId"].as_str().unwrap_or_default().to_string();
    let described = emulator.get_space(&space_id).await;
    assert_eq!(
        described.ok(case)["eventExpiryDays"],
        json!(7),
        "case {case:?}: the conflicting retry must not have changed the stored space"
    );

    // CreateEvent: the same token with the same request, INCLUDING the same sessionId,
    // returns the same event.
    let event_request = json!({
        "spaceId": space_id,
        "actorId": "actor1",
        "sessionId": "session1",
        "payload": structured_payload(json!({"nested": {"b": [1, 2, 3]}})),
        "clientToken": "tok-event-1",
    });
    let first = emulator.call("CreateEvent", event_request.clone()).await;
    let first_event = first.ok("event token replay")["event"].clone();
    let replay = emulator.call("CreateEvent", event_request.clone()).await;
    assert_eq!(
        replay.ok("event token replay")["event"], first_event,
        "case {:?}: a replay must answer the event the first request created",
        "event token replay"
    );

    let mut different = event_request.clone();
    different["payload"] = structured_payload(json!({"nested": {"b": [1, 2, 4]}}));
    emulator
        .call("CreateEvent", different)
        .await
        .assert_error(
            "event token with a different payload",
            409,
            "ConflictException",
        );

    // And with no sessionId the service generates a new one on each attempt, so a retry
    // cannot be a replay.
    let unsessioned = json!({
        "spaceId": space_id,
        "actorId": "actor1",
        "payload": structured_payload(json!({"a": 1})),
        "clientToken": "tok-event-2",
    });
    emulator
        .call("CreateEvent", unsessioned.clone())
        .await
        .ok("unsessioned first attempt");
    emulator
        .call("CreateEvent", unsessioned)
        .await
        .assert_error(
            "unsessioned retry cannot replay",
            409,
            "ConflictException",
        );

    // Exactly the events the successful calls created, and no duplicates.
    let listed = emulator
        .call(
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
            }),
        )
        .await;
    assert_eq!(
        listed.ok("no duplicate events")["events"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "a replayed CreateEvent must not append a second event, got {}",
        listed.body
    );
}

/// `DeleteSpace` is a move the clock finishes: the space reports `DELETING`, its events
/// stop being readable at once, a second delete answers `DELETING` again, and the purge
/// comes due on its own.
#[tokio::test]
async fn delete_space_reports_deleting_and_then_purges() {
    let case = "delete lifecycle";
    let emulator = Emulator::new();
    let space_id = emulator.active_space_id(case, "doomedspace").await;
    let created = emulator
        .call(
            "CreateEvent",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "payload": conversational_payload("ASSISTANT", "noted"),
            }),
        )
        .await;
    let event_id = created.ok(case)["event"]["eventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let deleted = emulator
        .call("DeleteSpace", json!({"spaceId": space_id}))
        .await;
    assert_eq!(
        deleted.ok(case),
        &json!({"spaceId": space_id, "status": "DELETING"}),
        "case {case:?}: DeleteSpace answers the space and status DELETING"
    );

    // While the delete is in flight the space is still reported, with its status.
    let described = emulator.get_space(&space_id).await;
    assert_eq!(
        described.ok(case)["status"], "DELETING",
        "case {case:?}: GetSpace reports DELETING until the delete finishes, got {}",
        described.body
    );

    // "Deleting a space that is already DELETING returns DELETING again."
    let again = emulator
        .call("DeleteSpace", json!({"spaceId": space_id}))
        .await;
    assert_eq!(
        again.ok(case),
        &json!({"spaceId": space_id, "status": "DELETING"}),
        "case {case:?}: a second DeleteSpace answers DELETING again"
    );

    // "After you delete the space, you can't retrieve its events."
    emulator
        .call(
            "GetEvent",
            json!({"spaceId": space_id, "eventId": event_id}),
        )
        .await
        .assert_error(case, 404, "ResourceNotFoundException");
    emulator
        .call(
            "ListEvents",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
            }),
        )
        .await
        .assert_error(case, 404, "ResourceNotFoundException");
    emulator
        .call(
            "CreateEvent",
            json!({
                "spaceId": space_id,
                "actorId": "actor1",
                "sessionId": "session1",
                "payload": structured_payload(json!({"late": true})),
            }),
        )
        .await
        .assert_error(case, 409, "ConflictException");

    // The purge is owed by the CLOCK: nothing here asks for it.
    emulator.poll_until_gone(case, &space_id).await;

    // And the name is free again.
    let reused = emulator
        .call("CreateSpace", json!({"name": "doomedspace"}))
        .await;
    assert!(
        is_space_id(reused.ok(case)["spaceId"].as_str().unwrap_or_default()),
        "case {case:?}: a purged space frees its name, got {}",
        reused.body
    );
}

/// The store is bounded: past [`osm_state::MAX_SPACES`] `CreateSpace` answers the quota
/// error it models, and a delete frees a slot again.
#[tokio::test]
async fn create_space_refuses_past_the_space_quota() {
    let case = "space quota";
    let emulator = Emulator::new();
    let mut ids: Vec<String> = Vec::new();
    for ordinal in 0..osm_state::MAX_SPACES {
        let created = emulator
            .active_space(case, json!({"name": format!("quota{ordinal}")}))
            .await;
        ids.push(created["spaceId"].as_str().unwrap_or_default().to_string());
    }

    emulator
        .call("CreateSpace", json!({"name": "onetoomany"}))
        .await
        .assert_error(case, 402, "ServiceQuotaExceededException");

    // A delete frees a slot: the quota is a LIVE count of the spaces held, not a
    // monotonic counter.
    emulator
        .call("DeleteSpace", json!({"spaceId": ids[0]}))
        .await
        .ok(case);
    emulator.bring_pending_due();
    emulator.get_space(&ids[0]).await.assert_error(
        case,
        404,
        "ResourceNotFoundException",
    );

    let created = emulator
        .call("CreateSpace", json!({"name": "afterthedelete"}))
        .await;
    assert!(
        is_space_id(created.ok(case)["spaceId"].as_str().unwrap_or_default()),
        "case {case:?}: a freed slot must accept another create, got {}",
        created.body
    );
}
