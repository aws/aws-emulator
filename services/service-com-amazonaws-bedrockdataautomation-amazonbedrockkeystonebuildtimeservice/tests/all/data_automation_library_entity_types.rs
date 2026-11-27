// SPDX-License-Identifier: CC0-1.0

//! What a DataAutomationLibrary REPORTS as the entity types it holds.
//!
//! THE RULE under test: a library holds the entity type its create provisions, and acquires
//! another when an `UPSERT` ingestion job naming that type is accepted against it;
//! `GetDataAutomationLibrary` reports every type the library holds in `entityTypes` — each
//! type ONCE however many jobs have populated it, each carrying the `entityMetadata` derived
//! from the jobs this store holds for the pair. A job that takes entities OUT
//! (`operationType` `DELETE`) gives the library no type.
//!
//! The suites are TABLE-driven along what the rule generalises over: the entity type
//! (`smithy_query(shape="EntityType")` -> `["VOCABULARY"]`), the
//! `operationType` (`smithy_query(shape="LibraryIngestionJobOperationType")` -> `["DELETE",
//! "UPSERT"]`), the input source the submission names (the modelled `InputConfiguration`'s
//! `s3Object` and `inlinePayload`), and the number of submissions per type.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, string_at};

/// The Region every case signs for.
const REGION: &str = "us-east-1";

/// The entity types the model admits, which is the set a library can acquire.
///
/// Source: `smithy_query(query="shape",shape="EntityType")` -> `["VOCABULARY"]`.
const ENTITY_TYPES: [&str; 1] = ["VOCABULARY"];

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

/// `CreateDataAutomationLibrary`; returns the `libraryArn` it minted.
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

/// The `library` member `GetDataAutomationLibrary` reports.
async fn read_library(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let uri = format!("/data-automation-libraries/{}/", encode_label(arn));
    let (status, payload) = call(router, "POST", &uri, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetDataAutomationLibrary 200, got {status} with {payload:?}"
    );
    payload
        .get("library")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no library in {payload:?}"))
}

/// An `inputConfiguration` carrying the modelled `InlinePayload` union's upsert arm.
fn inline_input(entity_id: &str) -> Value {
    json!({
        "inlinePayload": {
            "upsertEntitiesInfo": [{
                "vocabulary": {
                    "entityId": entity_id,
                    "language": "EN",
                    "phrases": [{ "text": "mockingbird" }],
                },
            }],
        },
    })
}

/// An `inputConfiguration` naming the S3 object a submission reads from instead.
fn s3_input(key: &str) -> Value {
    json!({ "s3Object": { "s3Uri": format!("s3://bda-ingestion/{key}.json") } })
}

/// `InvokeDataAutomationLibraryIngestionJob` for one (entity type, operation type, input)
/// triple; returns the `jobArn` it minted.
async fn submit(
    router: &MultiServiceRouter,
    library_arn: &str,
    entity_type: &str,
    operation_type: &str,
    input: Value,
    case: &str,
) -> String {
    let uri = format!(
        "/data-automation-libraries/{}/library-ingestion-jobs/",
        encode_label(library_arn)
    );
    let body = json!({
        "entityType": entity_type,
        "operationType": operation_type,
        "inputConfiguration": input,
        "outputConfiguration": { "s3Uri": "s3://bda-ingestion/output/entity-types" },
    });
    let (status, payload) = call(router, "PUT", &uri, &body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected InvokeDataAutomationLibraryIngestionJob 201, got {status} \
         with {payload:?}"
    );
    string_at(&payload, "jobArn", case)
}

/// The `entityTypes` a library reports, as (entityType, entityMetadata) pairs.
fn reported_types(library: &Value, case: &str) -> Vec<(String, Option<String>)> {
    library
        .get("entityTypes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no entityTypes list in {library:?}"))
        .iter()
        .map(|item| {
            (
                string_at(item, "entityType", case),
                item.get("entityMetadata")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect()
}

/// Require that `metadata` is the state-derived JSON object a reported entity type carries:
/// the number of ingestion jobs that have populated the type, and — once one has — the
/// instant of the latest of them.
fn assert_metadata(metadata: Option<&String>, expected_jobs: u64, case: &str) {
    let text = metadata
        .unwrap_or_else(|| panic!("case {case:?}: the reported type carries no entityMetadata"));
    let document: Value = serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("case {case:?}: entityMetadata {text:?} is not JSON: {e}"));
    assert_eq!(
        document.get("ingestionJobCount").and_then(Value::as_u64),
        Some(expected_jobs),
        "case {case:?}: expected {expected_jobs} ingestion job(s) counted, got {document:?}"
    );
    let instant = document.get("lastIngestionTime").and_then(Value::as_str);
    match (expected_jobs, instant) {
        (0, None) => {}
        (0, Some(reported)) => panic!(
            "case {case:?}: a type nothing has ingested into must report no ingestion \
             instant, got {reported:?}"
        ),
        (_, None) => panic!("case {case:?}: no lastIngestionTime in {document:?}"),
        (_, Some(reported)) => {
            aws_smithy_types::DateTime::from_str(
                reported,
                aws_smithy_types::date_time::Format::DateTime,
            )
            .unwrap_or_else(|e| {
                panic!("case {case:?}: lastIngestionTime {reported:?} is not a date-time: {e}")
            });
        }
    }
}

/// RULE: a library stands up holding the entity type its create provisions, and nothing
/// beyond it — the metadata says nothing has been ingested into that type yet.
#[tokio::test]
async fn a_created_library_holds_the_provisioned_entity_type() {
    let router = emulator();
    let arn = create_library(&router, "provisioned_library").await;
    let library = read_library(&router, &arn, "a fresh library").await;
    let reported = reported_types(&library, "a fresh library");
    let named: Vec<&String> = reported.iter().map(|(name, _)| name).collect();
    assert_eq!(
        named,
        vec![state::library_entity_types::PROVISIONED_ENTITY_TYPE],
        "case \"a fresh library\": expected the provisioned type alone, got {named:?}"
    );
    assert_metadata(reported[0].1.as_ref(), 0, "a fresh library");
}

/// RULE: an accepted UPSERT submission gives the library the entity type it names, whatever
/// that type is and whichever input source the submission reads from — and the read reports
/// it with the metadata derived from the jobs this store holds for the pair.
#[tokio::test]
async fn an_upsert_submission_gives_the_library_the_type_it_names() {
    let router = emulator();

    // (case, the entity type, the input source the submission names)
    let rows: Vec<(String, &str, Value)> = ENTITY_TYPES
        .iter()
        .flat_map(|entity_type| {
            [
                (
                    format!("{entity_type} from an inline payload"),
                    *entity_type,
                    inline_input("entity-type-entity"),
                ),
                (
                    format!("{entity_type} from an S3 object"),
                    *entity_type,
                    s3_input("entity-type-source"),
                ),
            ]
        })
        .collect();

    for (index, (case, entity_type, input)) in rows.into_iter().enumerate() {
        let arn = create_library(&router, &format!("acquiring_library_{index}")).await;
        submit(&router, &arn, entity_type, "UPSERT", input, &case).await;
        let library = read_library(&router, &arn, &case).await;
        let reported = reported_types(&library, &case);
        let ingested = reported
            .iter()
            .find(|(name, _)| name == entity_type)
            .unwrap_or_else(|| panic!("case {case:?}: {entity_type} is absent from {reported:?}"));
        assert_metadata(ingested.1.as_ref(), 1, &case);
        // The library holds the type just ingested, beside the one its create provisioned —
        // and nothing else: a submission gives it the type it NAMES, never another.
        let mut named: Vec<String> = reported.iter().map(|(name, _)| name.clone()).collect();
        named.sort();
        let mut expected: Vec<String> =
            vec![state::library_entity_types::PROVISIONED_ENTITY_TYPE.to_string()];
        if entity_type != state::library_entity_types::PROVISIONED_ENTITY_TYPE {
            expected.push((*entity_type).to_string());
        }
        expected.sort();
        assert_eq!(
            named, expected,
            "case {case:?}: expected the provisioned type and the ingested one, got {named:?}"
        );
    }
}

/// RULE: the entity types are a SET — a type several jobs have populated is reported once,
/// with the metadata counting all of them — and a library reports every modelled type it has
/// been ingested into.
#[tokio::test]
async fn a_type_is_reported_once_however_many_jobs_populated_it() {
    let router = emulator();
    let arn = create_library(&router, "repeatedly_ingested_library").await;

    // (case, how many UPSERT submissions of each type have been accepted)
    for submissions in 1..=3u64 {
        for entity_type in ENTITY_TYPES {
            let case = format!("{entity_type} after {submissions} submission(s)");
            submit(
                &router,
                &arn,
                entity_type,
                "UPSERT",
                inline_input(&format!("entity-{submissions}")),
                &case,
            )
            .await;
            let library = read_library(&router, &arn, &case).await;
            let reported = reported_types(&library, &case);
            let named: Vec<&String> = reported.iter().map(|(name, _)| name).collect();
            let occurrences = named.iter().filter(|name| **name == entity_type).count();
            assert_eq!(
                occurrences, 1,
                "case {case:?}: expected the type reported exactly once, got {named:?}"
            );
            let metadata = reported
                .iter()
                .find(|(name, _)| name == entity_type)
                .map(|(_, metadata)| metadata.clone())
                .unwrap_or_else(|| panic!("case {case:?}: {entity_type} is absent from {named:?}"));
            assert_metadata(metadata.as_ref(), submissions, &case);
        }
    }

    let library = read_library(&router, &arn, "every ingested type").await;
    let mut named: Vec<String> = reported_types(&library, "every ingested type")
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    named.sort();
    let mut expected: Vec<String> = ENTITY_TYPES.iter().map(|t| (*t).to_string()).collect();
    expected.sort();
    assert_eq!(
        named, expected,
        "case \"every ingested type\": expected every ingested type reported, got {named:?}"
    );
}

/// RULE: a submission that takes entities OUT of the library gives it no entity type — the
/// library keeps reporting exactly the types it already held.
#[tokio::test]
async fn a_delete_submission_gives_the_library_no_type() {
    let router = emulator();

    for entity_type in ENTITY_TYPES {
        let case = format!("a DELETE submission of {entity_type}");
        let arn = create_library(
            &router,
            &format!("deleting_library_{}", entity_type.to_lowercase()),
        )
        .await;
        submit(
            &router,
            &arn,
            entity_type,
            "DELETE",
            json!({ "inlinePayload": { "deleteEntitiesInfo": { "entityIds": ["entity-1"] } } }),
            &case,
        )
        .await;
        let library = read_library(&router, &arn, &case).await;
        let reported = reported_types(&library, &case);
        let named: Vec<&String> = reported.iter().map(|(name, _)| name).collect();
        assert_eq!(
            named,
            vec![state::library_entity_types::PROVISIONED_ENTITY_TYPE],
            "case {case:?}: expected only the provisioned type, got {named:?}"
        );
        assert_metadata(reported[0].1.as_ref(), 0, &case);
    }
}

/// RULE: reporting the entity types writes nothing — two reads of one library answer
/// identically, and the second read reports what the first did.
#[tokio::test]
async fn reading_the_entity_types_changes_nothing() {
    let router = emulator();
    let arn = create_library(&router, "stable_entity_types").await;
    submit(
        &router,
        &arn,
        "VOCABULARY",
        "UPSERT",
        inline_input("stable-entity"),
        "a stable library",
    )
    .await;

    let first = read_library(&router, &arn, "first read").await;
    let second = read_library(&router, &arn, "second read").await;
    assert_eq!(
        first, second,
        "case \"two reads\": expected identical answers, got {first:?} then {second:?}"
    );
}
