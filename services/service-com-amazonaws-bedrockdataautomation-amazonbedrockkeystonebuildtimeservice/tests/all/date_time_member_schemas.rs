// SPDX-License-Identifier: CC0-1.0

//! The wire form of every `date-time` member this service serves, checked against the shipped
//! model itself rather than against a list written out by hand.
//!
//! THE RULE UNDER TEST: a timestamp member whose effective `@timestampFormat` is `date-time` —
//! its own trait, else the one on the shape it targets — goes onto the restJson1 wire as an
//! RFC 3339 STRING, so the member schema the codec reads the form off must report that format;
//! a member the model annotates with no format keeps the protocol default and must report none.
//!
//! The cases are driven BY THE MODEL: the first suite resolves, out of the shipped snapshot,
//! every structure and union whose members carry that format and asserts that each one's Rust
//! member schemas agree with the model, member for member — so a model update that adds a
//! timestamp member, or re-renders a declaration from the vendored codegen (which drops the
//! trait, see `src/ops/types/date_time_wire.rs`), fails here instead of silently putting an
//! epoch number on the wire. The second suite drives the library family's doors end to end,
//! over a table of submissions, and reads the answered instants back as strings.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::ops::types::{
    Blueprint, BlueprintSummary, DataAutomationLibrary, DataAutomationLibraryIngestionJob,
    DataAutomationLibraryIngestionJobSummary, DataAutomationLibrarySummary, DataAutomationProject,
    DataAutomationProjectSummary, VocabularyEntity, VocabularyEntitySummary,
};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, string_at, timestamp_at};

/// The shipped model, the one source for which members are `date-time` members.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json"
));

/// The wire form the model names for a `date-time` member.
const DATE_TIME: &str = "date-time";

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// How long a case waits for a condition it polls for: the ingestion a submission owes falls
/// due a span after the call, and this budget is deliberately far wider than that span, so a
/// loaded host only makes the loop go round more often.
const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// How long the poll loop parks between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The shapes of this model, keyed by shape id.
fn shapes() -> Map<String, Value> {
    let model: Value = serde_json::from_str(MODEL).expect("the shipped model parses as JSON");
    model
        .get("shapes")
        .and_then(Value::as_object)
        .cloned()
        .expect("the model carries a shapes object")
}

/// The local name of a shape id: `com.amazonaws.bedrockdataautomation#Blueprint` -> `Blueprint`.
fn local(shape_id: &str) -> &str {
    shape_id.rsplit('#').next().unwrap_or(shape_id)
}

/// One shape's `@timestampFormat`, when it declares one.
fn declared_format(shape: &Value) -> Option<String> {
    shape
        .get("traits")
        .and_then(|traits| traits.get("smithy.api#timestampFormat"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The effective `@timestampFormat` of one member: its own trait, else the trait on the shape
/// it targets — the resolution Smithy specifies, and the one a member schema has to report.
/// A member whose target is not a timestamp has none.
fn effective_format(shapes: &Map<String, Value>, member: &Value) -> Option<String> {
    let target = member.get("target").and_then(Value::as_str)?;
    let targeted = shapes.get(target)?;
    if targeted.get("type").and_then(Value::as_str) != Some("timestamp") {
        return None;
    }
    declared_format(member).or_else(|| declared_format(targeted))
}

/// Every member of one aggregate shape, with the format the model gives it.
fn members_of(shapes: &Map<String, Value>, shape: &Value) -> BTreeMap<String, Option<String>> {
    shape
        .get("members")
        .and_then(Value::as_object)
        .map(|members| {
            members
                .iter()
                .map(|(name, member)| (name.clone(), effective_format(shapes, member)))
                .collect()
        })
        .unwrap_or_default()
}

/// Every structure and union of the model that carries at least one `date-time` member, keyed
/// by local shape name, with every member of that shape and the format the model gives it.
fn shapes_carrying_a_date_time_member(
    shapes: &Map<String, Value>,
) -> BTreeMap<String, BTreeMap<String, Option<String>>> {
    let mut found = BTreeMap::new();
    for (shape_id, shape) in shapes {
        let kind = shape.get("type").and_then(Value::as_str).unwrap_or_default();
        if kind != "structure" && kind != "union" {
            continue;
        }
        let members = members_of(shapes, shape);
        if members
            .values()
            .any(|format| format.as_deref() == Some(DATE_TIME))
        {
            found.insert(local(shape_id).to_string(), members);
        }
    }
    found
}

/// The Rust schema this crate serves each such shape through. One row per shape the model puts
/// a `date-time` member on; the suite below asserts this list IS that set, so a model update
/// that moves a timestamp onto another shape fails rather than going unnoticed.
fn served_schemas() -> Vec<(&'static str, &'static Schema)> {
    vec![
        ("Blueprint", &Blueprint::SCHEMA),
        ("BlueprintSummary", &BlueprintSummary::SCHEMA),
        ("DataAutomationLibrary", &DataAutomationLibrary::SCHEMA),
        (
            "DataAutomationLibrarySummary",
            &DataAutomationLibrarySummary::SCHEMA,
        ),
        (
            "DataAutomationLibraryIngestionJob",
            &DataAutomationLibraryIngestionJob::SCHEMA,
        ),
        (
            "DataAutomationLibraryIngestionJobSummary",
            &DataAutomationLibraryIngestionJobSummary::SCHEMA,
        ),
        ("DataAutomationProject", &DataAutomationProject::SCHEMA),
        (
            "DataAutomationProjectSummary",
            &DataAutomationProjectSummary::SCHEMA,
        ),
        ("VocabularyEntity", &VocabularyEntity::SCHEMA),
        ("VocabularyEntitySummary", &VocabularyEntitySummary::SCHEMA),
    ]
}

/// RULE: every shape the model puts a `date-time` member on is served through a schema that
/// declares exactly the model's members, and reports `date-time` on exactly the members the
/// model gives that format to.
#[test]
fn every_date_time_member_schema_reports_the_format_the_model_resolves() {
    let shapes = shapes();
    let from_model = shapes_carrying_a_date_time_member(&shapes);
    let served = served_schemas();

    let covered: BTreeSet<&str> = served.iter().map(|(name, _)| *name).collect();
    let declared: BTreeSet<&str> = from_model.keys().map(String::as_str).collect();
    assert_eq!(
        covered, declared,
        "case \"coverage\": the shapes this crate serves a hand-written member schema for are \
         not the shapes the model puts a date-time member on"
    );

    for (case, schema) in served {
        let expected = &from_model[case];
        let members = schema.members();
        let names: BTreeSet<String> = members
            .iter()
            .map(|member| member.member_name().unwrap_or("<unnamed>").to_string())
            .collect();
        let wanted: BTreeSet<String> = expected.keys().cloned().collect();
        assert_eq!(
            names, wanted,
            "case {case:?}: the member schemas are not the model's members"
        );

        for (index, member) in members.iter().enumerate() {
            let name = member.member_name().unwrap_or("<unnamed>");
            assert_eq!(
                member.member_index(),
                Some(index),
                "case {case:?}: member {name:?} reports member_index {:?} at position {index}",
                member.member_index()
            );
            let format = member.timestamp_format().map(|trait_| trait_.format());
            if expected[name].as_deref() == Some(DATE_TIME) {
                assert!(
                    matches!(format, Some(TimestampFormat::DateTime)),
                    "case {case:?}: member {name:?} is a date-time member of the model, so its \
                     schema must carry @timestampFormat(date-time), got {format:?}"
                );
            } else {
                assert!(
                    format.is_none(),
                    "case {case:?}: the model resolves no format for member {name:?}, so its \
                     schema must report none, got {format:?}"
                );
            }
        }
    }
}

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`), which is what `can_handle` resolves the route key from.
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

/// One ingestion job's path, as `GetDataAutomationLibraryIngestionJob` binds it.
fn job_uri(library_arn: &str, job_arn: &str) -> String {
    format!(
        "/data-automation-libraries/{}/library-ingestion-jobs/{}",
        encode_label(library_arn),
        encode_label(job_arn)
    )
}

/// One entity's path, as `GetDataAutomationLibraryEntity` binds it.
fn entity_uri(library_arn: &str, entity_type: &str, entity_id: &str) -> String {
    format!(
        "/data-automation-libraries/{}/entityType/{}/entities/{}",
        encode_label(library_arn),
        encode_label(entity_type),
        encode_label(entity_id)
    )
}

/// The entity COLLECTION path, as `ListDataAutomationLibraryEntities` binds it.
fn entities_uri(library_arn: &str, entity_type: &str) -> String {
    format!(
        "/data-automation-libraries/{}/entityType/{}/entities/",
        encode_label(library_arn),
        encode_label(entity_type)
    )
}

/// A submission carrying one inline vocabulary upsert: the four modelled required members,
/// with the entity under the `vocabulary` arm of the modelled `UpsertEntityInfo` union
/// (`smithy_query(query="shape",shape="UpsertEntityInfo")` carries that single member).
fn upsert(entity: Value) -> Value {
    json!({
        "entityType": "VOCABULARY",
        "operationType": "UPSERT",
        "inputConfiguration": {
            "inlinePayload": { "upsertEntitiesInfo": [{ "vocabulary": entity }] },
        },
        "outputConfiguration": { "s3Uri": "s3://bda-ingestion/output/" },
    })
}

/// The `job` an answered read wraps, or a panic naming the case.
fn wrapped(payload: &Value, member: &str, case: &str) -> Value {
    payload
        .get(member)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {member:?} in {payload:?}"))
}

/// Look at one door until the answer satisfies `settled`, or the generous [`POLL_DEADLINE`]
/// passes: an ingestion falls due a span after the submission, so a case waits for the
/// CONDITION rather than for a length of time.
async fn poll_until(
    router: &MultiServiceRouter,
    uri: &str,
    case: &str,
    settled: impl Fn(u16, &Value) -> bool,
) -> Value {
    let started = Instant::now();
    let mut last = (0u16, Value::Null);
    while started.elapsed() < POLL_DEADLINE {
        let (status, payload) = call(router, "POST", uri, &json!({})).await;
        if settled(status, &payload) {
            return payload;
        }
        last = (status, payload);
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!("case {case:?}: {uri} never settled; the last answer was {last:?}")
}

/// One row of the wire table: the case, the library it uses, the entity id and the entity an
/// upsert carries. The rows differ along the entity id's form, whether a description is
/// carried, the language named and the number of phrases.
type WireRow<'a> = (&'a str, &'a str, &'a str, Value);

/// RULE (end to end): every door of the library family answers its `date-time` members as
/// RFC 3339 strings — the library read and listing (`creationTime`), the ingestion job read
/// and listing (`creationTime`, `completionTime`) and the entity read and listing
/// (`lastModifiedTime`) — and the instants they report are the stored ones, so a listing
/// agrees with the read beside it.
#[tokio::test]
async fn every_library_door_answers_its_date_time_members_as_rfc_3339_strings() {
    let router = emulator();
    let rows: Vec<WireRow<'_>> = vec![
        (
            "one phrase, no description",
            "date-time-library-one",
            "vocab-one",
            json!({
                "entityId": "vocab-one",
                "language": "EN",
                "phrases": [{ "text": "mockingbird" }],
            }),
        ),
        (
            "an underscored id, a description and two phrases",
            "date-time_library_two",
            "vocab_two",
            json!({
                "entityId": "vocab_two",
                "description": "birds of the coast",
                "language": "DE",
                "phrases": [{ "text": "nachtigall" }, { "text": "amsel" }],
            }),
        ),
    ];

    // Every library this store holds, so the listing below is asserted over more than one item.
    let mut libraries: Vec<(&str, &str, String)> = Vec::new();
    for (case, library_name, _, _) in &rows {
        libraries.push((case, library_name, create_library(&router, library_name).await));
    }

    // Each submission is accepted first, so the two ingestions fall due together rather than
    // one after the other.
    let mut submitted: Vec<(&str, &str, String, String)> = Vec::new();
    for ((case, _, entity_id, entity), (_, _, library_arn)) in rows.iter().zip(libraries.iter()) {
        let (status, payload) =
            call(&router, "PUT", &jobs_uri(library_arn), &upsert(entity.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected InvokeDataAutomationLibraryIngestionJob 201, got {status} \
             with {payload:?}"
        );
        submitted.push((
            case,
            entity_id,
            library_arn.clone(),
            string_at(&payload, "jobArn", case),
        ));
    }

    for (case, library_name, library_arn) in &libraries {
        // GetDataAutomationLibrary -> DataAutomationLibrary$creationTime.
        let (status, payload) = call(
            &router,
            "POST",
            &format!(
                "/data-automation-libraries/{}/",
                encode_label(library_arn)
            ),
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDataAutomationLibrary 200, got {status} with {payload:?}"
        );
        let library = wrapped(&payload, "library", case);
        let created = timestamp_at(&library, "creationTime", case);
        assert_eq!(
            string_at(&library, "libraryName", case),
            *library_name,
            "case {case:?}: the read reported another library: {library:?}"
        );

        // ListDataAutomationLibraries -> DataAutomationLibrarySummary$creationTime, the same
        // instant the read reports.
        let (status, page) = call(
            &router,
            "POST",
            "/data-automation-libraries/",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListDataAutomationLibraries 200, got {status} with {page:?}"
        );
        let items = page
            .get("libraries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no libraries in {page:?}"));
        assert_eq!(
            items.len(),
            libraries.len(),
            "case {case:?}: expected every stored library in the listing, got {page:?}"
        );
        let listed = items
            .iter()
            .find(|item| string_at(item, "libraryArn", case) == *library_arn)
            .unwrap_or_else(|| panic!("case {case:?}: {library_arn} is absent from {page:?}"));
        assert_eq!(
            timestamp_at(listed, "creationTime", case),
            created,
            "case {case:?}: the listed creationTime differs from the read's: {listed:?}"
        );
    }

    for (case, entity_id, library_arn, job_arn) in &submitted {
        // GetDataAutomationLibraryIngestionJob -> creationTime, and completionTime once the
        // ingestion the clock owes has fallen due.
        let read = poll_until(&router, &job_uri(library_arn, job_arn), case, |status, body| {
            status == 200
                && body
                    .get("job")
                    .and_then(|job| job.get("completionTime"))
                    .is_some()
        })
        .await;
        let job = wrapped(&read, "job", case);
        let job_created = timestamp_at(&job, "creationTime", case);
        let job_completed = timestamp_at(&job, "completionTime", case);
        assert!(
            job_completed >= job_created,
            "case {case:?}: the completion instant must not precede the creation instant, got \
             {job:?}"
        );

        // ListDataAutomationLibraryIngestionJobs -> the summary's two instants, the read's.
        let (status, page) = call(&router, "POST", &jobs_uri(library_arn), &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListDataAutomationLibraryIngestionJobs 200, got {status} \
             with {page:?}"
        );
        let items = page
            .get("jobs")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no jobs in {page:?}"));
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected the one job minted under this library, got {page:?}"
        );
        assert_eq!(
            timestamp_at(&items[0], "creationTime", case),
            job_created,
            "case {case:?}: the listed creationTime differs from the read's: {page:?}"
        );
        assert_eq!(
            timestamp_at(&items[0], "completionTime", case),
            job_completed,
            "case {case:?}: the listed completionTime differs from the read's: {page:?}"
        );

        // GetDataAutomationLibraryEntity -> VocabularyEntity$lastModifiedTime.
        let read = poll_until(
            &router,
            &entity_uri(library_arn, "VOCABULARY", entity_id),
            case,
            |status, _| status == 200,
        )
        .await;
        let entity = wrapped(&read, "entity", case);
        let vocabulary = wrapped(&entity, "vocabulary", case);
        let modified = timestamp_at(&vocabulary, "lastModifiedTime", case);

        // ListDataAutomationLibraryEntities -> VocabularyEntitySummary$lastModifiedTime.
        let (status, page) = call(
            &router,
            "POST",
            &entities_uri(library_arn, "VOCABULARY"),
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListDataAutomationLibraryEntities 200, got {status} with \
             {page:?}"
        );
        let items = page
            .get("entities")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no entities in {page:?}"));
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected the one entity this upsert stood up, got {page:?}"
        );
        let summary = wrapped(&items[0], "vocabulary", case);
        assert_eq!(
            timestamp_at(&summary, "lastModifiedTime", case),
            modified,
            "case {case:?}: the listed lastModifiedTime differs from the read's: {page:?}"
        );
    }
}
