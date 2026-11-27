// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ENTITIES a DataAutomationLibrary holds:
//! `GetDataAutomationLibraryEntity`, the read of ONE of them, and
//! `ListDataAutomationLibraryEntities`, the listing of the ones a (library, entity type) pair
//! holds.
//!
//! RULE. The entities of a (library, entity type) pair are what the ingestion jobs accepted
//! against that library have made of it: an `UPSERT` job's inline entries stand the entity
//! its `entityId` names up (or replace what was stored under it) once that job has
//! completed, and a `DELETE` job's entries take entities out — so a read of the triple
//! reports the description, language and phrases of the latest entry that upserted the id,
//! the listing of the pair enumerates exactly those entities (in this service's listing
//! order, one page at a time, with `maxResults` capping the page and a `nextToken` handed
//! back only while something is left after it), and both refuse anything that resolves to
//! nothing (an unresolved library, another library's entity, the other modelled entity type,
//! an id nothing upserted, an id a delete removed).
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch` with a
//! signed-looking request — so routing, HTTP binding, constraint validation, the handler and
//! response serialization are all exercised, exactly as an AWS SDK `bedrock-data-automation`
//! client would.
//!
//! The suites are TABLE-driven: the rows differ along the `entityId` form, whether a
//! description is carried, the language named, the number of phrases, whether a phrase
//! carries a display form, how many entities one pair holds and the page size asked for —
//! never over one recorded request.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at, timestamp_at};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A `libraryArn` that satisfies the modelled pattern and names no library of this store.
const UNKNOWN_LIBRARY_ARN: &str =
    "arn:aws:bedrock:us-east-1:000000000000:data-automation-library/000000000000";

/// A `libraryArn` of ANOTHER Region, well formed but addressing nothing this endpoint serves.
const OTHER_REGION_LIBRARY_ARN: &str =
    "arn:aws:bedrock:eu-west-1:000000000000:data-automation-library/000000000000";

/// How long a case waits for the condition it polls for: the ingestion the submission owes
/// falls due a span after the call, and this budget is deliberately far wider than that
/// span, so a loaded host only makes the loop go round more often.
const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// How long the poll loop parks between looks. The runtime this workspace enables carries
/// no timer, so the test thread parks; a test may wait for a due instant, a handler may not.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`).
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{REGION}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return its status and decoded JSON body.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// `CreateDataAutomationLibrary`, per its modelled `@http`
/// `{method: PUT, uri: /data-automation-libraries/, code: 201}`. Returns the ARN.
async fn create_library(router: &MultiServiceRouter, name: &str) -> String {
    let (status, payload) = call(
        router,
        "PUT",
        "/data-automation-libraries/",
        &json!({ "libraryName": name }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected CreateDataAutomationLibrary 201, got {status} with {payload:?}"
    );
    string_at(&payload, "libraryArn", name)
}

/// The ingestion-job collection path of one library.
fn jobs_uri(library_arn: &str) -> String {
    format!(
        "/data-automation-libraries/{}/library-ingestion-jobs/",
        encode_label(library_arn)
    )
}

/// The entity path, per `GetDataAutomationLibraryEntity`'s modelled `@http`
/// `{method: POST, uri:
/// /data-automation-libraries/{libraryArn}/entityType/{entityType}/entities/{entityId}}`.
fn entity_uri(library_arn: &str, entity_type: &str, entity_id: &str) -> String {
    format!(
        "/data-automation-libraries/{}/entityType/{}/entities/{}",
        encode_label(library_arn),
        encode_label(entity_type),
        encode_label(entity_id)
    )
}

/// The entity COLLECTION path, per `ListDataAutomationLibraryEntities`'s modelled `@http`
/// `{method: POST, uri:
/// /data-automation-libraries/{libraryArn}/entityType/{entityType}/entities/}`.
fn entities_uri(library_arn: &str, entity_type: &str) -> String {
    format!(
        "/data-automation-libraries/{}/entityType/{}/entities/",
        encode_label(library_arn),
        encode_label(entity_type)
    )
}

/// A submission body: the four required members, with the inline payload the case names.
fn submission(entity_type: &str, operation_type: &str, payload: Value) -> Value {
    json!({
        "entityType": entity_type,
        "operationType": operation_type,
        "inputConfiguration": { "inlinePayload": payload },
        "outputConfiguration": { "s3Uri": "s3://bda-ingestion/output/" },
    })
}

/// `InvokeDataAutomationLibraryIngestionJob` for a submission that must be accepted.
async fn submit(router: &MultiServiceRouter, library_arn: &str, case: &str, body: &Value) {
    let (status, payload) = call(router, "PUT", &jobs_uri(library_arn), body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected InvokeDataAutomationLibraryIngestionJob 201, got {status} with \
         {payload:?}"
    );
}

/// `GetDataAutomationLibraryEntity`.
async fn get_entity(
    router: &MultiServiceRouter,
    library_arn: &str,
    entity_type: &str,
    entity_id: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &entity_uri(library_arn, entity_type, entity_id),
        &json!({}),
    )
    .await
}

/// Look for the entity until the read ANSWERS what the case is waiting for, or the generous
/// [`POLL_DEADLINE`] passes.
///
/// The ingestion a submission owes falls due a span after the call, so what a case waits for
/// is the CONDITION (the entity is there, or it is gone) rather than a length of time: the
/// loop looks again until it holds.
async fn poll_entity(
    router: &MultiServiceRouter,
    library_arn: &str,
    entity_type: &str,
    entity_id: &str,
    want: u16,
    case: &str,
) -> Value {
    let started = Instant::now();
    let mut last = (0u16, Value::Null);
    while started.elapsed() < POLL_DEADLINE {
        let answer = get_entity(router, library_arn, entity_type, entity_id).await;
        if answer.0 == want {
            return answer.1;
        }
        last = answer;
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!(
        "case {case:?}: the read never answered {want} for entity {entity_id:?}; last answer was \
         {last:?}"
    )
}

/// The `vocabulary` member of an answered `entity` union, or a panic naming the case.
fn vocabulary(answer: &Value, case: &str) -> Value {
    answer
        .get("entity")
        .and_then(|entity| entity.get("vocabulary"))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no entity.vocabulary in {answer:?}"))
}

/// One row of the upsert table: the case, the `entityId`, the entity info an upsert carries
/// and the phrase count it describes.
type UpsertRow<'a> = (&'a str, &'a str, Value, usize);

/// RULE: once the ingestion that upserted it has completed, the read of the triple reports
/// that entity exactly as the upsert described it — the id, the description, the language and
/// every phrase, with the instant the ingestion completed as its `lastModifiedTime`.
///
/// The rows differ along the `entityId` form, whether a description is carried, the language
/// named (both from the modelled `Language` enum,
/// `smithy_query(query="shape",shape="Language")` -> `["EN", "DE", "ES", "FR", "IT", "PT",
/// "JA", "KO", "CN", "TW", "HK"]`, including values the issue's own case does not use), the
/// number of phrases and whether a phrase carries a `displayAsText`.
#[tokio::test]
async fn an_upserted_entity_is_reported_as_the_ingestion_described_it() {
    let router = emulator();
    let library = create_library(&router, "entity-read-library").await;

    let rows: Vec<UpsertRow<'_>> = vec![
        (
            "one phrase, no description",
            "vocab-1",
            json!({
                "entityId": "vocab-1",
                "language": "EN",
                "phrases": [{ "text": "mockingbird" }],
            }),
            1,
        ),
        (
            "an underscored id, a description and three phrases",
            "vocab_two",
            json!({
                "entityId": "vocab_two",
                "description": "birds of the coast",
                "language": "DE",
                "phrases": [
                    { "text": "nachtigall" },
                    { "text": "amsel", "displayAsText": "Amsel" },
                    { "text": "eisvogel" },
                ],
            }),
            3,
        ),
        (
            "a numeric id and a non-latin language",
            "0123456789",
            json!({
                "entityId": "0123456789",
                "description": "a short note",
                "language": "JA",
                "phrases": [{ "text": "kawasemi", "displayAsText": "Kawasemi" }],
            }),
            1,
        ),
    ];

    for (case, entity_id, info, phrases) in rows {
        let body = submission(
            "VOCABULARY",
            "UPSERT",
            json!({ "upsertEntitiesInfo": [{ "vocabulary": info.clone() }] }),
        );
        submit(&router, &library, case, &body).await;

        let answer = poll_entity(&router, &library, "VOCABULARY", entity_id, 200, case).await;
        let reported = vocabulary(&answer, case);
        assert_eq!(
            string_at(&reported, "entityId", case),
            entity_id,
            "case {case:?}: expected the upserted id, got {reported:?}"
        );
        assert_eq!(
            reported.get("language"),
            info.get("language"),
            "case {case:?}: expected the language the upsert carried, got {reported:?}"
        );
        assert_eq!(
            reported.get("description"),
            info.get("description"),
            "case {case:?}: expected the description the upsert carried, got {reported:?}"
        );
        assert_eq!(
            reported.get("phrases"),
            info.get("phrases"),
            "case {case:?}: expected the phrases the upsert carried, got {reported:?}"
        );
        assert_eq!(
            reported
                .get("phrases")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(phrases),
            "case {case:?}: expected {phrases} phrase(s), got {reported:?}"
        );

        // Reporting an entity writes nothing, so a second read answers the same thing.
        let (status, again) = get_entity(&router, &library, "VOCABULARY", entity_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second read must answer 200, got {status} with {again:?}"
        );
        assert_eq!(
            vocabulary(&again, case),
            reported,
            "case {case:?}: two reads must report one entity, got {again:?}"
        );
        let modified = timestamp_at(&reported, "lastModifiedTime", case);
        assert!(
            modified.0 > 0,
            "case {case:?}: the entity carries the instant its ingestion completed, got \
             {reported:?}"
        );
    }
}

/// RULE: a later `UPSERT` of the same id REPLACES what was stored under it, and a completed
/// `DELETE` takes it out — from which instant the read of the triple resolves to nothing.
#[tokio::test]
async fn an_upsert_replaces_and_a_delete_removes_the_entity() {
    let router = emulator();
    let library = create_library(&router, "entity-mutation-library").await;
    let entity_id = "vocab-replaced";

    submit(
        &router,
        &library,
        "the first upsert",
        &submission(
            "VOCABULARY",
            "UPSERT",
            json!({
                "upsertEntitiesInfo": [{
                    "vocabulary": {
                        "entityId": entity_id,
                        "language": "EN",
                        "phrases": [{ "text": "mockingbird" }],
                    },
                }],
            }),
        ),
    )
    .await;
    let first = poll_entity(
        &router,
        &library,
        "VOCABULARY",
        entity_id,
        200,
        "the first upsert",
    )
    .await;
    assert_eq!(
        vocabulary(&first, "the first upsert").get("language"),
        Some(&json!("EN")),
        "case \"the first upsert\": expected the language it carried, got {first:?}"
    );

    submit(
        &router,
        &library,
        "the second upsert",
        &submission(
            "VOCABULARY",
            "UPSERT",
            json!({
                "upsertEntitiesInfo": [{
                    "vocabulary": {
                        "entityId": entity_id,
                        "description": "revised",
                        "language": "FR",
                        "phrases": [{ "text": "rossignol" }, { "text": "merle" }],
                    },
                }],
            }),
        ),
    )
    .await;
    // The replacement is what the SECOND ingestion left, so the case waits for the member it
    // changed rather than for a length of time.
    let started = Instant::now();
    let replaced = loop {
        let (status, answer) = get_entity(&router, &library, "VOCABULARY", entity_id).await;
        let reported = vocabulary(&answer, "the second upsert");
        if status == 200 && reported.get("language") == Some(&json!("FR")) {
            break reported;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case \"the second upsert\": the replacement never landed; last answer was {answer:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    };
    assert_eq!(
        replaced.get("description"),
        Some(&json!("revised")),
        "case \"the second upsert\": expected the description it carried, got {replaced:?}"
    );
    assert_eq!(
        replaced.get("phrases").and_then(Value::as_array).map(Vec::len),
        Some(2),
        "case \"the second upsert\": expected the phrases it carried, got {replaced:?}"
    );

    submit(
        &router,
        &library,
        "the delete",
        &submission(
            "VOCABULARY",
            "DELETE",
            json!({ "deleteEntitiesInfo": { "entityIds": [entity_id] } }),
        ),
    )
    .await;
    let gone = poll_entity(
        &router,
        &library,
        "VOCABULARY",
        entity_id,
        404,
        "the delete",
    )
    .await;
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "case \"the delete\": expected the modelled refusal, got {gone:?}"
    );
    assert_eq!(
        gone.get("message").and_then(Value::as_str),
        Some(state::library_entities::ENTITY_NOT_FOUND_MESSAGE),
        "case \"the delete\": expected this service's sentence for an unresolved entity, got \
         {gone:?}"
    );
}

/// RULE: the read resolves the whole triple, so everything that resolves to nothing is
/// refused — an unresolved library and an ARN of another Region on the library, and an id of
/// another library, an id differing only in case, or one nothing upserted on the entity.
///
/// The rows differ along WHICH part of the triple fails to resolve.
#[tokio::test]
async fn what_resolves_to_nothing_is_refused() {
    let router = emulator();
    let library = create_library(&router, "entity-refusal-library").await;
    let other = create_library(&router, "entity-refusal-other").await;
    let entity_id = "vocab-of-one-library";
    submit(
        &router,
        &other,
        "fixture",
        &submission(
            "VOCABULARY",
            "UPSERT",
            json!({
                "upsertEntitiesInfo": [{
                    "vocabulary": {
                        "entityId": entity_id,
                        "language": "EN",
                        "phrases": [{ "text": "kingfisher" }],
                    },
                }],
            }),
        ),
    )
    .await;
    // The fixture entity is there before the refusals are asserted, so "absent from the
    // other library" is not merely "not ingested yet".
    poll_entity(&router, &other, "VOCABULARY", entity_id, 200, "fixture").await;

    // (case, the library addressed, the entity type, the entity id, status, error code, the
    // message AWS answers)
    let rows: [(&str, &str, &str, &str, u16, &str, &str); 5] = [
        (
            "a library this store holds nothing under",
            UNKNOWN_LIBRARY_ARN,
            "VOCABULARY",
            entity_id,
            404,
            "ResourceNotFoundException",
            state::LIBRARY_ARN_NOT_FOUND_MESSAGE,
        ),
        (
            "a library of another Region",
            OTHER_REGION_LIBRARY_ARN,
            "VOCABULARY",
            entity_id,
            400,
            "ValidationException",
            state::library_arn::WRONG_REGION_MESSAGE,
        ),
        (
            "an entity of another library",
            &library,
            "VOCABULARY",
            entity_id,
            404,
            "ResourceNotFoundException",
            state::library_entities::ENTITY_NOT_FOUND_MESSAGE,
        ),
        (
            // An `entityId` is the id the upsert named, character for character: the model
            // admits both cases (`smithy_query(shape="EntityId")` -> `^[a-zA-Z0-9-_]+$`), so
            // an id differing only in case names another entity.
            "an id differing from the upserted one only in case",
            &other,
            "VOCABULARY",
            "VOCAB-OF-ONE-LIBRARY",
            404,
            "ResourceNotFoundException",
            state::library_entities::ENTITY_NOT_FOUND_MESSAGE,
        ),
        (
            "an id nothing has upserted",
            &other,
            "VOCABULARY",
            "no_such_entity",
            404,
            "ResourceNotFoundException",
            state::library_entities::ENTITY_NOT_FOUND_MESSAGE,
        ),
    ];
    for (case, library_arn, entity_type, id, status, code, message) in rows {
        let (actual, refused) = get_entity(&router, library_arn, entity_type, id).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected {status}, got {actual} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            code,
            "case {case:?}: expected {code}, got {refused:?}"
        );
        assert_eq!(
            refused.get("message").and_then(Value::as_str),
            Some(message),
            "case {case:?}: expected {message:?}, got {refused:?}"
        );
    }
}

/// `ListDataAutomationLibraryEntities` with the page the case asks for.
async fn list_entities(
    router: &MultiServiceRouter,
    library_arn: &str,
    entity_type: &str,
    page: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &entities_uri(library_arn, entity_type),
        page,
    )
    .await
}

/// The `vocabulary` summaries a listing answered, in the order it served them.
fn listed(payload: &Value, case: &str) -> Vec<Value> {
    payload
        .get("entities")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no entities array in {payload:?}"))
        .iter()
        .map(|item| {
            item.get("vocabulary")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no vocabulary summary in {item:?}"))
        })
        .collect()
}

/// The `entityId`s a listing served, in that order.
fn listed_ids(payload: &Value, case: &str) -> Vec<String> {
    listed(payload, case)
        .iter()
        .map(|summary| string_at(summary, "entityId", case))
        .collect()
}

/// The entity info one upsert entry carries: the id, the language, an optional description
/// and `phrases` phrase(s) — each phrase text derived from the id, so no two entities of one
/// case carry the same phrase.
fn vocabulary_info(
    entity_id: &str,
    language: &str,
    description: Option<&str>,
    phrases: usize,
) -> Value {
    let texts: Vec<Value> = (0..phrases)
        .map(|index| json!({ "text": format!("{entity_id}-phrase-{index}") }))
        .collect();
    let mut info = json!({ "entityId": entity_id, "language": language, "phrases": texts });
    if let Some(text) = description {
        info["description"] = json!(text);
    }
    info
}

/// One upsert submission carrying every entity the case describes.
fn upsert_submission(entity_type: &str, entities: &[(String, &str, Option<&str>, usize)]) -> Value {
    let entries: Vec<Value> = entities
        .iter()
        .map(|(entity_id, language, description, phrases)| {
            json!({ "vocabulary": vocabulary_info(entity_id, language, *description, *phrases) })
        })
        .collect();
    submission(
        entity_type,
        "UPSERT",
        json!({ "upsertEntitiesInfo": entries }),
    )
}

/// Look at the whole listing until it holds exactly `want` entities, or the generous
/// [`POLL_DEADLINE`] passes.
///
/// The ingestion a submission owes falls due a span after the call, so what a case waits for
/// is the CONDITION (the listing holds what the ingestion left) rather than a length of time.
async fn poll_listing(
    router: &MultiServiceRouter,
    library_arn: &str,
    entity_type: &str,
    want: usize,
    case: &str,
) -> Value {
    let started = Instant::now();
    let mut last = (0u16, Value::Null);
    while started.elapsed() < POLL_DEADLINE {
        let answer = list_entities(router, library_arn, entity_type, &json!({})).await;
        if answer.0 == 200 && listed_ids(&answer.1, case).len() == want {
            return answer.1;
        }
        last = answer;
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!(
        "case {case:?}: the listing never held {want} entity/entities; last answer was {last:?}"
    );
}

/// Walk every page of one listing with the page size the case names, and return the ids in
/// the order they were served, how large each page was, and how many pages it took.
async fn walk_listing(
    router: &MultiServiceRouter,
    library_arn: &str,
    entity_type: &str,
    max_results: Option<i32>,
    case: &str,
) -> (Vec<String>, Vec<usize>, usize) {
    let mut ids: Vec<String> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut page = json!({});
        if let Some(limit) = max_results {
            page["maxResults"] = json!(limit);
        }
        if let Some(cursor) = token.as_ref() {
            page["nextToken"] = json!(cursor);
        }
        let (status, payload) = list_entities(router, library_arn, entity_type, &page).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListDataAutomationLibraryEntities 200, got {status} with \
             {payload:?}"
        );
        let served = listed_ids(&payload, case);
        sizes.push(served.len());
        ids.extend(served);
        pages += 1;
        token = payload
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return (ids, sizes, pages);
        }
        assert!(
            pages <= 64,
            "case {case:?}: the listing handed back a token on {pages} pages for {} id(s)",
            ids.len()
        );
    }
}

/// One row of the listing table: the case, the entity type ingested into and the entities one
/// upsert carries — the id, the language, an optional description and the phrase count.
type ListingRow<'a> = (
    &'a str,
    &'a str,
    Vec<(String, &'a str, Option<&'a str>, usize)>,
);

/// RULE: the listing of a (library, entity type) pair enumerates exactly the entities the
/// completed ingestions left there, each as the narrow modelled
/// `VocabularyEntitySummary` — the id, the description and language the upsert carried, HOW
/// MANY phrases it holds, and the instant the ingestion that wrote it completed — and a read
/// of each listed id resolves the same entity.
///
/// The rows differ along how many entities the pair holds (one, a few, the modelled maximum
/// of one payload, `smithy_query(query="shape",shape="UpsertEntitiesInfo")`:
/// `@length(min: 1, max: 10)`), the modelled entity type they are ingested under
/// (`smithy_query(query="shape",shape="EntityType")` -> `["VOCABULARY"]`), whether a
/// description is carried and how many phrases each entity holds.
#[tokio::test]
async fn the_listing_enumerates_what_the_completed_ingestions_left() {
    let rows: Vec<ListingRow<'_>> = vec![
        (
            "one entity",
            "VOCABULARY",
            vec![("vocab-only".to_string(), "EN", None, 1)],
        ),
        (
            "three entities, two of them described",
            "VOCABULARY",
            vec![
                ("vocab-c".to_string(), "DE", Some("coastal birds"), 2),
                ("vocab-a".to_string(), "FR", None, 1),
                ("vocab_b".to_string(), "JA", Some("garden birds"), 3),
            ],
        ),
        (
            "one payload's modelled maximum",
            "VOCABULARY",
            (0..10)
                .map(|index| {
                    (
                        format!("vocab-{index:02}"),
                        "KO",
                        (index % 2 == 0).then_some("a described entity"),
                        1 + index % 3,
                    )
                })
                .collect(),
        ),
    ];

    for (index, (case, entity_type, entities)) in rows.into_iter().enumerate() {
        let router = emulator();
        let library = create_library(&router, &format!("entity-listing-library-{index}")).await;
        submit(
            &router,
            &library,
            case,
            &upsert_submission(entity_type, &entities),
        )
        .await;

        let answer = poll_listing(&router, &library, entity_type, entities.len(), case).await;
        // Every entity of this pair was written by the one ingestion, so they share its
        // instant and the listing order is decided by the id that breaks that tie.
        let mut expected: Vec<String> = entities
            .iter()
            .map(|(entity_id, ..)| entity_id.clone())
            .collect();
        expected.sort();
        assert_eq!(
            listed_ids(&answer, case),
            expected,
            "case {case:?}: expected the ingested ids in listing order, got {answer:?}"
        );
        assert!(
            answer.get("nextToken").is_none(),
            "case {case:?}: the last page of a listing carries no token, got {answer:?}"
        );

        for summary in listed(&answer, case) {
            let entity_id = string_at(&summary, "entityId", case);
            let (_, language, description, phrases) = entities
                .iter()
                .find(|(ingested, ..)| *ingested == entity_id)
                .unwrap_or_else(|| panic!("case {case:?}: {entity_id:?} was never ingested"));
            assert_eq!(
                summary.get("language").and_then(Value::as_str),
                Some(*language),
                "case {case:?}: expected the language the upsert carried for {entity_id:?}, got \
                 {summary:?}"
            );
            assert_eq!(
                summary.get("description").and_then(Value::as_str),
                *description,
                "case {case:?}: expected the description the upsert carried for {entity_id:?}, \
                 got {summary:?}"
            );
            assert_eq!(
                summary.get("numOfPhrases").and_then(Value::as_i64),
                Some(*phrases as i64),
                "case {case:?}: expected {phrases} phrase(s) counted for {entity_id:?}, got \
                 {summary:?}"
            );
            // The summary is the NARROW projection: it counts the phrases rather than
            // serving them.
            assert!(
                summary.get("phrases").is_none(),
                "case {case:?}: a summary carries no phrases member, got {summary:?}"
            );
            let (status, read) = get_entity(&router, &library, entity_type, &entity_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: every listed id resolves, got {status} with {read:?} for \
                 {entity_id:?}"
            );
            let reported = vocabulary(&read, case);
            assert_eq!(
                reported
                    .get("phrases")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(*phrases),
                "case {case:?}: the read and the listing must agree about {entity_id:?}, got \
                 {reported:?}"
            );
            assert_eq!(
                timestamp_at(&summary, "lastModifiedTime", case),
                timestamp_at(&reported, "lastModifiedTime", case),
                "case {case:?}: the read and the listing must report one instant for \
                 {entity_id:?}"
            );
        }
    }
}

/// RULE: `maxResults` caps the page, a `nextToken` is handed back only while something is
/// left after it, and resuming with that token continues the same listing — so walking the
/// pages of one listing serves every entity of the pair exactly once, in the same order,
/// whatever page size was asked for.
///
/// The rows differ along the page size: one at a time, a size that divides the collection, a
/// size that does not, a size larger than the collection, the modelled maximum
/// (`smithy_query(query="operation",operation="ListDataAutomationLibraryEntities")`:
/// `maxResults` is `@range(min: 1, max: 1000)`) and no `maxResults` at all.
#[tokio::test]
async fn a_page_is_capped_and_a_token_resumes_the_listing() {
    let router = emulator();
    let library = create_library(&router, "entity-paging-library").await;
    let entities: Vec<(String, &str, Option<&str>, usize)> = (0..5)
        .map(|index| (format!("vocab-{index:02}"), "ES", None, 1))
        .collect();
    submit(
        &router,
        &library,
        "the paged fixture",
        &upsert_submission("VOCABULARY", &entities),
    )
    .await;
    let whole = poll_listing(
        &router,
        &library,
        "VOCABULARY",
        entities.len(),
        "the paged fixture",
    )
    .await;
    let order = listed_ids(&whole, "the paged fixture");

    // (case, the maxResults asked for, how many pages it takes)
    let rows: [(&str, Option<i32>, usize); 6] = [
        ("one at a time", Some(1), 5),
        ("a size that does not divide the collection", Some(2), 3),
        ("a size that divides the collection", Some(5), 1),
        ("a size larger than the collection", Some(6), 1),
        ("the modelled maximum", Some(1000), 1),
        ("no maxResults at all", None, 1),
    ];
    for (case, max_results, pages) in rows {
        let (served, sizes, walked) =
            walk_listing(&router, &library, "VOCABULARY", max_results, case).await;
        assert_eq!(
            served, order,
            "case {case:?}: the pages must serve the listing's order exactly once, got {served:?}"
        );
        assert_eq!(
            walked, pages,
            "case {case:?}: expected {pages} page(s), got {walked} of sizes {sizes:?}"
        );
        if let Some(limit) = max_results {
            for size in &sizes {
                assert!(
                    *size <= limit as usize,
                    "case {case:?}: a page must carry at most {limit} item(s), got {sizes:?}"
                );
            }
        }
    }
}

/// RULE: the listing is OF one (library, entity type) pair, so a pair nothing has ingested
/// into enumerates nothing, and everything that resolves to nothing — an unresolved library,
/// a library ARN of another Region, a `nextToken` this service never issued — is refused with
/// the modelled error for it.
#[tokio::test]
async fn the_listing_is_of_one_pair_and_refuses_what_resolves_to_nothing() {
    let router = emulator();
    let library = create_library(&router, "entity-listing-scope-library").await;
    let other = create_library(&router, "entity-listing-scope-other").await;
    let emptied = create_library(&router, "entity-listing-scope-emptied").await;
    submit(
        &router,
        &library,
        "fixture",
        &upsert_submission(
            "VOCABULARY",
            &[("vocab-of-one-pair".to_string(), "IT", None, 1)],
        ),
    )
    .await;
    poll_listing(&router, &library, "VOCABULARY", 1, "fixture").await;
    // A job that takes entities OUT leaves the pair it names holding nothing, so its listing
    // is as empty as one no job has ever named.
    submit(
        &router,
        &emptied,
        "the emptied pair",
        &submission(
            "VOCABULARY",
            "DELETE",
            json!({ "deleteEntitiesInfo": { "entityIds": ["vocab-of-one-pair"] } }),
        ),
    )
    .await;

    // (case, the library addressed, the entity type)
    let empty: [(&str, &str, &str); 2] = [
        ("a library nothing has ingested into", &other, "VOCABULARY"),
        (
            "a library whose only accepted job takes entities out",
            &emptied,
            "VOCABULARY",
        ),
    ];
    for (case, library_arn, entity_type) in empty {
        let (status, payload) = list_entities(&router, library_arn, entity_type, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an empty pair is enumerated, not refused, got {status} with \
             {payload:?}"
        );
        assert_eq!(
            listed_ids(&payload, case),
            Vec::<String>::new(),
            "case {case:?}: expected no entity, got {payload:?}"
        );
        assert!(
            payload.get("nextToken").is_none(),
            "case {case:?}: an empty listing carries no token, got {payload:?}"
        );
    }

    // (case, the library addressed, the page asked for, status, error code, the message AWS
    // answers)
    let refusals: [(&str, &str, Value, u16, &str, &str); 3] = [
        (
            "a library this store holds nothing under",
            UNKNOWN_LIBRARY_ARN,
            json!({}),
            404,
            "ResourceNotFoundException",
            state::LIBRARY_ARN_NOT_FOUND_MESSAGE,
        ),
        (
            "a library of another Region",
            OTHER_REGION_LIBRARY_ARN,
            json!({}),
            400,
            "ValidationException",
            state::library_arn::WRONG_REGION_MESSAGE,
        ),
        (
            "a nextToken this service never issued",
            &library,
            json!({ "nextToken": "not-a-token-this-service-issued" }),
            400,
            "ValidationException",
            "nextToken is not a token this service issued",
        ),
    ];
    for (case, library_arn, page, status, code, message) in refusals {
        let (actual, refused) = list_entities(&router, library_arn, "VOCABULARY", &page).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected {status}, got {actual} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            code,
            "case {case:?}: expected {code}, got {refused:?}"
        );
        assert_eq!(
            refused.get("message").and_then(Value::as_str),
            Some(message),
            "case {case:?}: expected {message:?}, got {refused:?}"
        );
    }
}

/// RULE: the listing and the read of one entity are two projections of ONE fold, so a
/// completed `DELETE` takes the entity it names out of both while every other entity of the
/// pair stays where it was.
#[tokio::test]
async fn a_completed_delete_leaves_the_listing_without_that_entity() {
    let router = emulator();
    let library = create_library(&router, "entity-listing-delete-library").await;
    let entities: Vec<(String, &str, Option<&str>, usize)> = ["vocab-kept-a", "vocab-removed", "vocab-kept-b"]
        .iter()
        .map(|entity_id| ((*entity_id).to_string(), "PT", None, 2))
        .collect();
    submit(
        &router,
        &library,
        "the fixture",
        &upsert_submission("VOCABULARY", &entities),
    )
    .await;
    poll_listing(&router, &library, "VOCABULARY", entities.len(), "the fixture").await;

    submit(
        &router,
        &library,
        "the delete",
        &submission(
            "VOCABULARY",
            "DELETE",
            json!({ "deleteEntitiesInfo": { "entityIds": ["vocab-removed"] } }),
        ),
    )
    .await;
    let after = poll_listing(
        &router,
        &library,
        "VOCABULARY",
        entities.len() - 1,
        "the delete",
    )
    .await;
    assert_eq!(
        listed_ids(&after, "the delete"),
        vec!["vocab-kept-a".to_string(), "vocab-kept-b".to_string()],
        "case \"the delete\": expected the entities the delete did not name, got {after:?}"
    );
    let (status, gone) = get_entity(&router, &library, "VOCABULARY", "vocab-removed").await;
    assert_eq!(
        status, 404,
        "case \"the delete\": the read agrees with the listing, got {status} with {gone:?}"
    );
}

/// One row of the refused-submission table: the case, the submission body, the `libraryArn`
/// it was addressed to, and the (entity type, entity id) pair the read then probes in the
/// library that exists.
type RefusedSubmissionRow<'a> = (&'a str, Value, String, &'a str, String);

/// RULE: a read of a (library, entity type, entity id) triple answers from what the
/// COMPLETED ingestions of that library left under the pair, so a submission the service
/// REFUSED stands nothing up: every well-formed read of an id only such a submission named
/// resolves to nothing, and is answered with the operation's modelled
/// `ResourceNotFoundException` (HTTP 404) carrying this service's fixed sentence for an
/// unresolved entity — never with an outcome the operation does not model.
///
/// The rows differ along HOW the submission was refused (an `entityType` outside the modelled
/// enum, an `operationType` outside it, a `libraryArn` naming another resource type, and a
/// submission naming no input source), along the modelled entity type the read then probes
/// (`smithy_query(query="shape",shape="EntityType")` -> `["VOCABULARY"]`) and along the
/// `entityId` form it names — hyphens, underscores, digits and the longest id the modelled
/// `@length(min: 1, max: 128)` admits.
#[tokio::test]
async fn an_id_only_a_refused_ingestion_named_is_refused_by_the_read() {
    let router = emulator();
    let library = create_library(&router, "refused-ingestion-library").await;
    // The longest `entityId` the modelled length admits: 128 characters of the modelled
    // `^[a-zA-Z0-9-_]+$`.
    let longest_id = format!("probe_{}", "a".repeat(122));
    assert_eq!(
        longest_id.chars().count(),
        128,
        "the longest-id row must name the modelled maximum, got {longest_id:?}"
    );
    /// A `libraryArn`-shaped value of ANOTHER resource type, which the modelled
    /// `DataAutomationLibraryArn` pattern does not admit.
    const WRONG_ARN_TYPE: &str =
        "arn:aws:bedrock:us-east-1:000000000000:data-automation-project/abcdefghijkl";

    // The sentence this error class carries, spelled out once here so the wording itself is
    // pinned and not merely compared with whatever the service happens to hold: it is fixed
    // text naming the two input members that address an entity, with neither of their values
    // interpolated.
    assert_eq!(
        state::library_entities::ENTITY_NOT_FOUND_MESSAGE,
        "The specified entity ID and entity type is not found.",
        "the sentence an unresolved entity is refused with must read as the service words it"
    );

    let upsert_of = |entity_id: &str| {
        json!({
            "upsertEntitiesInfo": [{
                "vocabulary": {
                    "entityId": entity_id,
                    "language": "EN",
                    "phrases": [{ "text": "kingfisher" }],
                },
            }],
        })
    };

    let rows: Vec<RefusedSubmissionRow<'_>> = vec![
        (
            "an entityType outside the modelled enum",
            submission("SPEAKER", "UPSERT", upsert_of("probe-unmodelled-type")),
            library.clone(),
            "VOCABULARY",
            "probe-unmodelled-type".to_string(),
        ),
        (
            "an operationType outside the modelled enum",
            submission("VOCABULARY", "REPLACE", upsert_of("probe_unmodelled_operation")),
            library.clone(),
            "VOCABULARY",
            "probe_unmodelled_operation".to_string(),
        ),
        (
            "a libraryArn naming another resource type",
            submission("VOCABULARY", "UPSERT", upsert_of("0123456789")),
            WRONG_ARN_TYPE.to_string(),
            "VOCABULARY",
            "0123456789".to_string(),
        ),
        (
            "a submission naming no input source",
            json!({
                "entityType": "VOCABULARY",
                "operationType": "UPSERT",
                "inputConfiguration": {},
                "outputConfiguration": { "s3Uri": "s3://bda-ingestion/output/" },
            }),
            library.clone(),
            "VOCABULARY",
            longest_id.clone(),
        ),
    ];

    for (case, body, submitted_to, entity_type, entity_id) in rows {
        let (status, refused) = call(&router, "PUT", &jobs_uri(&submitted_to), &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the submission refused with 400, got {status} with \
             {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {refused:?}"
        );
        assert!(
            refused.get("jobArn").is_none(),
            "case {case:?}: a refused submission mints no job, got {refused:?}"
        );

        // The read of the id that submission named: it resolves to nothing, because nothing
        // was ever ingested under it.
        let (status, absent) = get_entity(&router, &library, entity_type, &entity_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for the never-ingested id, got {status} with {absent:?}"
        );
        assert_eq!(
            error_code(&absent),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled refusal, got {absent:?}"
        );
        assert_eq!(
            absent.get("message").and_then(Value::as_str),
            Some(state::library_entities::ENTITY_NOT_FOUND_MESSAGE),
            "case {case:?}: expected this service's sentence for an unresolved entity, got \
             {absent:?}"
        );
        assert!(
            absent.get("entity").is_none(),
            "case {case:?}: a refusal carries no entity, got {absent:?}"
        );

        // The read writes nothing, so asking again answers exactly the same refusal.
        let (again_status, again) = get_entity(&router, &library, entity_type, &entity_id).await;
        assert_eq!(
            (again_status, &again),
            (status, &absent),
            "case {case:?}: two reads must answer the same refusal, got {again_status} with \
             {again:?}"
        );
    }
}
