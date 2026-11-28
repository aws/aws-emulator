// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the LONG-TERM memory records `CreateEvent` and `IngestData` make a
//! memory container derive: the extraction side of the memory data plane, driven over HTTP
//! through the same router the emulator serves with.
//!
//! The rule under test: a message a container accepts for long-term extraction yields, once
//! the clock has made that extraction due, one derived memory record per namespace its
//! container's active strategies resolve for the message's actor and session — carrying that
//! strategy's own `memoryStrategyId`, that one namespace, and content derived from the
//! messages it was extracted from — while a message the request excluded from extraction
//! (`extractionMode` `SKIP`) yields none and stays readable through the short-term event
//! reads.
//!
//! The rows span what that rule generalises over: which STRATEGY resolves the namespace (a
//! semantic, a summary and a user-preference strategy, each with its own template), what the
//! template resolves THROUGH (the actor and the session the service defines itself, the
//! strategy id, a variable the accepting request supplied, and one nothing supplies), which
//! WAY the container accepted the messages (an event, ingested content), whether the request
//! excluded them from extraction, whether the payload carries a conversational turn at all,
//! and which READ reports the record back (`ListMemoryRecords` by exact namespace, by
//! strategy, `GetMemoryRecord`, `RetrieveMemoryRecords`).
//!
//! A memory STRATEGY belongs to the AgentCore control plane, so that plane is mounted beside
//! this service on one shared engine — which is how the router mounts them — and every
//! fixture memory here is created through its own `CreateMemory`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;
use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as control;

use super::harness::label;

/// The actor every fixture writes for, and the two sessions it writes into.
const ACTOR: &str = "alpha";
const SESSION: &str = "s1";

/// How long a convergence poll waits for the span the emulator's extraction runs on, which
/// is far longer than that span: the test waits for the CONDITION, re-reading until it holds.
const CONVERGENCE_DEADLINE: Duration = Duration::from_secs(60);
/// How long one poll of that wait sleeps before looking again.
const CONVERGENCE_POLL: Duration = Duration::from_millis(50);

/// The emulator with BOTH AgentCore planes on one shared engine, request authentication off
/// so a test signs nothing.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(control::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an empty
/// body). The SigV4 credential scope is what routes the request to the AgentCore planes.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `CreateMemory` on the CONTROL plane: `POST /memories/create`. Answers the `memoryId` it
/// minted and, for each strategy, the `strategyId` it minted beside the name the request
/// gave it.
async fn create_memory(
    router: &MultiServiceRouter,
    name: &str,
    strategies: Value,
    namespace_keys: Value,
) -> (String, Vec<(String, String)>) {
    let (status, body) = send(
        router,
        "POST",
        "/memories/create",
        Some(json!({
            "name": name,
            // `eventExpiryDuration` is `@range(3..365)` and required.
            "eventExpiryDuration": 7,
            "memoryStrategies": strategies,
            "namespaceKeys": namespace_keys,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case the fixture memory {name}: expected HTTP 202 from CreateMemory, got {status} \
         with {body:?}"
    );
    let memory_id = body["memory"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let minted = body["memory"]["strategies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|strategy| {
            (
                strategy["name"].as_str().unwrap_or_default().to_string(),
                strategy["strategyId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    (memory_id, minted)
}

/// `CreateEvent`: `POST /memories/{memoryId}/events`, with one conversational turn.
async fn create_event(
    router: &MultiServiceRouter,
    memory: &str,
    session: &str,
    said: &str,
    extras: Value,
) -> (u16, Value) {
    let mut body = json!({
        "actorId": ACTOR,
        "sessionId": session,
        "eventTimestamp": 1_700_000_000,
        "payload": [{"conversational": {"content": {"text": said}, "role": "USER"}}],
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    send(
        router,
        "POST",
        &format!("/memories/{}/events", label(memory)),
        Some(body),
    )
    .await
}

/// `ListMemoryRecords`: `POST /memories/{memoryId}/memoryRecords`, narrowed by whatever the
/// case asks for. Answers the summaries it reported.
async fn list_records(router: &MultiServiceRouter, memory: &str, narrowing: Value) -> Vec<Value> {
    let (status, body) = send(
        router,
        "POST",
        &format!("/memories/{}/memoryRecords", label(memory)),
        Some(narrowing.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case listing {narrowing:?}: expected HTTP 200, got {status} with {body:?}"
    );
    body["memoryRecordSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `GetMemoryRecord`: `GET /memories/{memoryId}/memoryRecord/{memoryRecordId}`.
async fn get_record(router: &MultiServiceRouter, memory: &str, record: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!(
            "/memories/{}/memoryRecord/{}",
            label(memory),
            label(record)
        ),
        None,
    )
    .await
}

/// Make every transition the CONTROL plane owes the fixture memories already due, so a
/// strategy is `ACTIVE` and a case waits for no span it is not testing. The test reaching
/// into the store the handlers share, not an emulator behaviour.
fn settle_memories(engine: &engine::InMemoryEngine) {
    let already_due = SystemTime::now() - Duration::from_secs(3_600);
    let state = control::state::service_state(engine).expect("the control plane's state");
    let mut store = state.lock();
    for memory in store.memories.values_mut() {
        memory.settles_at = already_due;
    }
}

/// Make the long-term extraction of every message the container has accepted already due,
/// the same way: the test moving the clock rather than waiting for it.
fn make_extraction_due(engine: &engine::InMemoryEngine) {
    let already_due = SystemTime::now() - Duration::from_secs(3_600);
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    for memory in store.memories.values_mut() {
        for actor in memory.actors.values_mut() {
            for session in actor.sessions.values_mut() {
                for event in &mut session.events {
                    event.extraction_due_at = already_due;
                }
                for ingested in &mut session.ingestions {
                    ingested.extraction_due_at = already_due;
                }
            }
        }
    }
}

/// Hold the extraction of every accepted message open, so a case observing the state BEFORE
/// it comes due is deterministic however fast the host runs.
fn hold_extraction_open(engine: &engine::InMemoryEngine) {
    let not_yet_due = SystemTime::now() + Duration::from_secs(3_600);
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    for memory in store.memories.values_mut() {
        for actor in memory.actors.values_mut() {
            for session in actor.sessions.values_mut() {
                for event in &mut session.events {
                    event.extraction_due_at = not_yet_due;
                }
                for ingested in &mut session.ingestions {
                    ingested.extraction_due_at = not_yet_due;
                }
            }
        }
    }
}

/// The one strategy id the fixture minted for the strategy called `name`.
fn strategy_id(minted: &[(String, String)], name: &str) -> String {
    minted
        .iter()
        .find(|(strategy, _)| strategy == name)
        .map(|(_, id)| id.clone())
        .unwrap_or_else(|| panic!("case the fixture memory: no strategy named {name} was minted"))
}

/// The namespaces a reported summary carries.
fn namespaces(summary: &Value) -> Vec<String> {
    summary["namespaces"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The text content a reported summary or record carries.
fn content_text(record: &Value) -> String {
    record["content"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The four strategies every fixture memory in this suite is created with, one per template
/// shape the rule has to resolve.
fn fixture_strategies() -> Value {
    json!([
        // The actor/session template the recorded client configures, trailing separator and
        // all.
        {"semanticMemoryStrategy": {
            "name": "facts",
            "namespaceTemplates": ["/simparity/{actorId}/{sessionId}/"],
        }},
        // No namespace stated at all, so the strategy takes the control plane's default
        // template for its type.
        {"summaryMemoryStrategy": {"name": "recap"}},
        // A template through a namespace variable the ACCEPTING REQUEST supplies.
        {"userPreferenceMemoryStrategy": {
            "name": "prefs",
            "namespaceTemplates": ["/t/{tenant}/{actorId}"],
        }},
        // A template through a variable no request supplies, which therefore addresses no
        // namespace at all.
        {"semanticMemoryStrategy": {
            "name": "unscoped",
            "namespaceTemplates": ["/t/{region}/{actorId}"],
        }},
    ])
}

/// The namespace variable keys the fixture memory DEFINES, which its templates resolve
/// through.
fn fixture_namespace_keys() -> Value {
    json!([{"key": "tenant"}, {"key": "region"}])
}

/// The derived record every namespace the container's strategies RESOLVE carries, and no
/// record at all under a template nothing resolves.
///
/// One event, four strategies, and one row per strategy: the record is reported under that
/// strategy's own id, in the one namespace its template resolved to, with content derived
/// from the turn the event carried — read back by the exact namespace, by the strategy, and
/// through `GetMemoryRecord`.
#[tokio::test]
async fn a_due_extraction_derives_one_record_per_namespace_a_strategy_resolves() {
    let (router, engine) = emulator();
    let (memory, minted) = create_memory(
        &router,
        "extraction_fixture",
        fixture_strategies(),
        fixture_namespace_keys(),
    )
    .await;
    settle_memories(&engine);

    let said = "the user prefers window seats";
    let (status, created) = create_event(
        &router,
        &memory,
        SESSION,
        said,
        json!({"extractionConfig": {"namespaceVariables": {"tenant": "acme"}}}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the accepted event: expected HTTP 201, got {status} with {created:?}"
    );
    make_extraction_due(&engine);

    // (case, the strategy whose template resolved it, the namespace it resolves to)
    let rows: [(&str, &str, Option<String>); 4] = [
        (
            "the semantic strategy's own actor/session template",
            "facts",
            Some(format!("/simparity/{ACTOR}/{SESSION}/")),
        ),
        (
            "the summary strategy's default template",
            "recap",
            Some(format!(
                "/strategies/{}/actors/{ACTOR}/sessions/{SESSION}",
                strategy_id(&minted, "recap")
            )),
        ),
        (
            "a template through the variable the event supplied",
            "prefs",
            Some(format!("/t/acme/{ACTOR}")),
        ),
        (
            "a template through a variable no request supplied",
            "unscoped",
            None,
        ),
    ];

    for (case, strategy, expected) in rows {
        let id = strategy_id(&minted, strategy);
        let by_strategy = list_records(&router, &memory, json!({"memoryStrategyId": id})).await;
        let Some(expected_namespace) = expected else {
            assert!(
                by_strategy.is_empty(),
                "case {case:?}: expected no derived record for strategy {id}, got {by_strategy:?}"
            );
            continue;
        };
        assert_eq!(
            by_strategy.len(),
            1,
            "case {case:?}: expected exactly one derived record for strategy {id}, got \
             {by_strategy:?}"
        );
        let summary = &by_strategy[0];
        assert_eq!(
            namespaces(summary),
            vec![expected_namespace.clone()],
            "case {case:?}: expected namespaces [{expected_namespace}], got {:?}",
            namespaces(summary)
        );
        assert_eq!(
            summary["memoryStrategyId"].as_str().unwrap_or_default(),
            id,
            "case {case:?}: expected memoryStrategyId {id}, got {summary:?}"
        );
        assert!(
            content_text(summary).contains(said),
            "case {case:?}: expected content mentioning {said:?}, got {:?}",
            content_text(summary)
        );

        // The same record is what a listing of the EXACT namespace reports.
        let by_namespace =
            list_records(&router, &memory, json!({"namespace": expected_namespace})).await;
        assert_eq!(
            by_namespace.len(),
            1,
            "case {case:?}: expected the namespace {expected_namespace} to report one record, \
             got {by_namespace:?}"
        );
        let record_id = summary["memoryRecordId"].as_str().unwrap_or_default();
        assert_eq!(
            by_namespace[0]["memoryRecordId"]
                .as_str()
                .unwrap_or_default(),
            record_id,
            "case {case:?}: expected the namespace listing to report {record_id}, got {:?}",
            by_namespace[0]
        );

        // And what a read of it by id reports, carrying the namespace it was derived into.
        let (status, read) = get_record(&router, &memory, record_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading {record_id}, got {status} with {read:?}"
        );
        assert_eq!(
            namespaces(&read["memoryRecord"]),
            vec![expected_namespace.clone()],
            "case {case:?}: expected the read record to list [{expected_namespace}], got {:?}",
            namespaces(&read["memoryRecord"])
        );
    }

    // A relevance search over the same namespace finds the derived record too: it is a
    // record of the container like any other.
    let (status, retrieved) = send(
        &router,
        "POST",
        &format!("/memories/{}/retrieve", label(&memory)),
        Some(json!({
            "namespace": format!("/simparity/{ACTOR}/{SESSION}/"),
            "searchCriteria": {"searchQuery": "window seats"},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the relevance search: expected HTTP 200, got {status} with {retrieved:?}"
    );
    let found = retrieved["memoryRecordSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        found.len(),
        1,
        "case the relevance search: expected the derived record to be found, got {found:?}"
    );
}

/// The one knob the API exposes over the pipeline: an event created with `extractionMode`
/// `SKIP` derives nothing, in the very namespace a sibling event of the same memory derives
/// a record under, and stays readable short-term either way.
///
/// The rows differ along how the container accepted the messages (an event processed for
/// extraction as usual, an event excluded from it, an event whose payload carries no
/// conversational turn at all, and content submitted through `IngestData`), which is what
/// decides whether a namespace of the same memory holds a derived record.
#[tokio::test]
async fn only_the_messages_extraction_reads_derive_a_record() {
    let (router, engine) = emulator();
    let (memory, minted) = create_memory(
        &router,
        "extraction_modes",
        fixture_strategies(),
        fixture_namespace_keys(),
    )
    .await;
    settle_memories(&engine);
    let facts = strategy_id(&minted, "facts");

    // An event processed for extraction as usual.
    let (status, _) = create_event(&router, &memory, "kept", "remember the aisle seat", json!({}))
        .await;
    assert_eq!(status, 201, "case the kept event: expected HTTP 201");
    // The same event, excluded from long-term extraction.
    let (status, _) = create_event(
        &router,
        &memory,
        "skipped",
        "remember the aisle seat",
        json!({"extractionMode": "SKIP"}),
    )
    .await;
    assert_eq!(status, 201, "case the skipped event: expected HTTP 201");
    // An event whose payload carries no conversational turn, so it composes no message.
    let (status, body) = send(
        &router,
        "POST",
        &format!("/memories/{}/events", label(&memory)),
        Some(json!({
            "actorId": ACTOR,
            "sessionId": "structured",
            "eventTimestamp": 1_700_000_000,
            "payload": [{"json": {"content": {"clicked": "seat-map"}}}],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the json-only event: expected HTTP 201, got {status} with {body:?}"
    );
    // Content submitted for long-term extraction through the other door.
    let (status, body) = send(
        &router,
        "POST",
        &format!("/memories/{}/ingest", label(&memory)),
        Some(json!({
            "actorId": ACTOR,
            "sessionId": "ingested",
            "contentTimestamp": 1_700_000_000,
            "source": {"inline": {"payload": [
                {"conversational": {"content": {"text": "remember the aisle seat"}, "role": "USER"}}
            ]}},
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case the ingested content: expected HTTP 202, got {status} with {body:?}"
    );

    make_extraction_due(&engine);

    // (case, the session the messages went into, whether its namespace holds a derived
    //  record)
    let rows: [(&str, &str, bool); 4] = [
        ("an event processed for extraction as usual", "kept", true),
        ("an event created with extractionMode SKIP", "skipped", false),
        ("an event carrying no conversational turn", "structured", false),
        ("content submitted through IngestData", "ingested", true),
    ];
    for (case, session, derives) in rows {
        let namespace = format!("/simparity/{ACTOR}/{session}/");
        let reported = list_records(&router, &memory, json!({"namespace": namespace})).await;
        assert_eq!(
            reported.len(),
            usize::from(derives),
            "case {case:?}: expected {} record(s) under {namespace}, got {reported:?}",
            usize::from(derives)
        );
        if derives {
            assert_eq!(
                reported[0]["memoryStrategyId"].as_str().unwrap_or_default(),
                facts,
                "case {case:?}: expected the derived record under strategy {facts}, got {:?}",
                reported[0]
            );
        }
    }

    // Both events are still short-term memory, SKIP or not: the mode withholds the derived
    // record, not the event.
    for session in ["kept", "skipped"] {
        let (status, body) = send(
            &router,
            "POST",
            &format!(
                "/memories/{}/actor/{}/sessions/{}",
                label(&memory),
                label(ACTOR),
                label(session)
            ),
            Some(json!({"includePayloads": true})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case ListEvents on {session}: expected HTTP 200, got {status} with {body:?}"
        );
        let events = body["events"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            events.len(),
            1,
            "case ListEvents on {session}: expected the event to be readable, got {events:?}"
        );
        let said = events[0]["payload"][0]["conversational"]["content"]["text"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            said, "remember the aisle seat",
            "case ListEvents on {session}: expected the stored turn, got {:?}",
            events[0]
        );
    }
}

/// Nothing is derived before the clock makes the extraction due, and a message is extracted
/// exactly once however often the namespace is read; a further message of the same session
/// is extracted in its turn.
#[tokio::test]
async fn an_extraction_runs_once_and_only_when_it_is_due() {
    let (router, engine) = emulator();
    let (memory, _) = create_memory(
        &router,
        "extraction_once",
        fixture_strategies(),
        fixture_namespace_keys(),
    )
    .await;
    settle_memories(&engine);
    let namespace = format!("/simparity/{ACTOR}/{SESSION}/");

    let (status, _) = create_event(&router, &memory, SESSION, "first turn", json!({})).await;
    assert_eq!(status, 201, "case the first event: expected HTTP 201");

    // Still on its way: the extraction this call started is the clock's, so the namespace
    // the client polls is empty until it comes due.
    hold_extraction_open(&engine);
    let waiting = list_records(&router, &memory, json!({"namespace": namespace.clone()})).await;
    assert!(
        waiting.is_empty(),
        "case before the extraction is due: expected {namespace} to be empty, got {waiting:?}"
    );

    make_extraction_due(&engine);
    let first = list_records(&router, &memory, json!({"namespace": namespace.clone()})).await;
    assert_eq!(
        first.len(),
        1,
        "case the due extraction: expected one record under {namespace}, got {first:?}"
    );
    let derived_id = first[0]["memoryRecordId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Reading again extracts nothing further: the pick-up is recorded, so the same message
    // is not extracted twice.
    for read in 1..=3 {
        let again = list_records(&router, &memory, json!({"namespace": namespace.clone()})).await;
        assert_eq!(
            again.len(),
            1,
            "case read {read} after the extraction: expected the one record to stay, got \
             {again:?}"
        );
        assert_eq!(
            again[0]["memoryRecordId"].as_str().unwrap_or_default(),
            derived_id,
            "case read {read} after the extraction: expected record {derived_id}, got {:?}",
            again[0]
        );
    }

    // A further message of the same session is its own unit of work, extracted in its turn.
    let (status, _) = create_event(&router, &memory, SESSION, "second turn", json!({})).await;
    assert_eq!(status, 201, "case the second event: expected HTTP 201");
    make_extraction_due(&engine);
    let both = list_records(&router, &memory, json!({"namespace": namespace.clone()})).await;
    assert_eq!(
        both.len(),
        2,
        "case the second due extraction: expected two records under {namespace}, got {both:?}"
    );
    assert!(
        both.iter().any(|summary| content_text(summary).contains("second turn")),
        "case the second due extraction: expected a record derived from the second turn, got \
         {both:?}"
    );
}

/// The same convergence on the EMULATOR's own clock, with no test clock at all: a client
/// that writes an event and polls the strategy's namespace sees the derived record appear.
#[tokio::test]
async fn long_term_extraction_converges_on_the_clock_alone() {
    let (router, engine) = emulator();
    let (memory, minted) = create_memory(
        &router,
        "extraction_converges",
        fixture_strategies(),
        fixture_namespace_keys(),
    )
    .await;
    let namespace = format!("/simparity/{ACTOR}/{SESSION}/");

    // The memory reaches ACTIVE on the control plane's own span, which the client waits out
    // the same way it waits for anything else: by looking again.
    let deadline = SystemTime::now() + CONVERGENCE_DEADLINE;
    loop {
        let (status, body) = send(
            &router,
            "GET",
            &format!("/memories/{}/details", label(&memory)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case waiting for ACTIVE: expected HTTP 200, got {status} with {body:?}"
        );
        if body["memory"]["status"].as_str() == Some("ACTIVE") {
            break;
        }
        assert!(
            SystemTime::now() < deadline,
            "case waiting for ACTIVE: the memory never became ACTIVE, last read {body:?}"
        );
        tokio::time::sleep(CONVERGENCE_POLL).await;
    }

    let said = "remember the window seat";
    let (status, created) = create_event(&router, &memory, SESSION, said, json!({})).await;
    assert_eq!(
        status, 201,
        "case the accepted event: expected HTTP 201, got {status} with {created:?}"
    );
    // The engine is only here to prove the test drove the emulator's own clock rather than
    // moving it.
    let _ = &engine;

    let deadline = SystemTime::now() + CONVERGENCE_DEADLINE;
    let reported = loop {
        let reported = list_records(&router, &memory, json!({"namespace": namespace.clone()})).await;
        if !reported.is_empty() {
            break reported;
        }
        assert!(
            SystemTime::now() < deadline,
            "case the convergence poll: {namespace} never reported a derived record"
        );
        tokio::time::sleep(CONVERGENCE_POLL).await;
    };
    assert_eq!(
        reported.len(),
        1,
        "case the convergence poll: expected one derived record, got {reported:?}"
    );
    assert_eq!(
        reported[0]["memoryStrategyId"].as_str().unwrap_or_default(),
        strategy_id(&minted, "facts"),
        "case the convergence poll: expected the semantic strategy's id, got {:?}",
        reported[0]
    );
    assert!(
        content_text(&reported[0]).contains(said),
        "case the convergence poll: expected content mentioning {said:?}, got {:?}",
        content_text(&reported[0])
    );
}
