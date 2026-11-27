// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the CONSTRAINT-REFUSAL wording of this service, and of the
//! value set its `EntityType` members admit.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch` with a
//! signed-looking request — so routing, HTTP binding, the centralized constraint walk, the
//! refusal seam and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule generalises
//! over (the operation, the member, whether the value arrived in a URI label or in the
//! body, the constraint class, the expression or value set the member declares) rather than
//! over one recorded request:
//!
//! 1. a clause whose requirement IS the set of values the member admits — a `@pattern`
//!    expression, or an enumerated type's value set — names the member path and that set
//!    and states NO value, while a clause whose requirement is a BOUND on the value keeps
//!    the value half the framework renders;
//! 2. every value the `EntityType` set admits is admitted ALIKE by each operation that
//!    carries one, and a value outside the set is refused with the whole set stated;
//! 3. a member the request did not CARRY is refused with no value stated at all, named at
//!    the path the MODEL traverses to it — every structure and union member name of that
//!    traversal, and each list element by its 1-based position.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`): `can_handle` resolves the route key from it.
fn authorization() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef"
        .to_string()
}

/// The emulator under test: one shared engine, one mounted service, request authentication
/// off so the suite signs nothing for real.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return its status, the raw body text (so a case can assert a
/// value is nowhere in the answer) and the decoded JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, String, Value) {
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
    let text = String::from_utf8_lossy(&bytes).to_string();
    let decoded = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, text, decoded)
}

/// Mint one library through `CreateDataAutomationLibrary` (`PUT
/// /data-automation-libraries/`) and return the `libraryArn` it answered, so the rows below
/// address a library this store really holds.
async fn live_library(router: &MultiServiceRouter, name: &str) -> String {
    let (status, _, payload) = call(
        router,
        "PUT",
        "/data-automation-libraries/",
        &json!({ "libraryName": name }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected the seed CreateDataAutomationLibrary to be 201, got {status} with {payload:?}"
    );
    string_at(&payload, "libraryArn", "seed library")
}

/// The unanchored form of the expression `DataAutomationLibraryArn` declares (source:
/// `smithy_query(shape="DataAutomationLibraryArn")`).
const LIBRARY_ARN_PATTERN: &str = "arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:\
                                   [a-zA-Z0-9-]*:[0-9]{12}:data-automation-library/\
                                   [a-zA-Z0-9-]{12,36}";

/// The unanchored form of the expression `EntityId` declares (source:
/// `smithy_query(shape="EntityId")`), which `DataAutomationLibraryName`,
/// `BlueprintName` and `DataAutomationProjectName` declare too.
const NAME_PATTERN: &str = "[a-zA-Z0-9-_]+";

/// The unanchored form of the expression `NextToken` declares (source:
/// `smithy_query(shape="NextToken")`).
const NEXT_TOKEN_PATTERN: &str = r"\S*";

/// The values `EntityType` admits, in the order this service states them (source:
/// `smithy_query(shape="EntityType")`).
const ENTITY_TYPE_SET: &str = "VOCABULARY";

/// The values `LibraryIngestionJobOperationType` admits, in the order this service states
/// them (source: `smithy_query(shape="LibraryIngestionJobOperationType")`).
const OPERATION_TYPE_SET: &str = "UPSERT, DELETE";

/// The values `Type` admits, in the order this service states them (source:
/// `smithy_query(shape="Type")`).
const BLUEPRINT_TYPE_SET: &str = "DOCUMENT, IMAGE, AUDIO, VIDEO";

/// An ARN of ANOTHER resource family of this service: well-formed, inside no library ARN's
/// expression.
const A_BLUEPRINT_ARN: &str = "arn:aws:bedrock:us-west-2:000000000000:blueprint/221c62b1-0000000000000000";

/// An ARN of a third family of this service, so the ARN rows are not one string's story.
const A_PROJECT_ARN: &str =
    "arn:aws-cn:bedrock:cn-north-1:111111111111:data-automation-project/7f3c-1111-2222-3333";

/// `InvokeDataAutomationLibraryIngestionJob`'s URI (`PUT
/// /data-automation-libraries/{libraryArn}/library-ingestion-jobs/`), which
/// `ListDataAutomationLibraryIngestionJobs` binds too (`POST`).
fn ingestion_jobs_uri(library_arn: &str) -> String {
    format!(
        "/data-automation-libraries/{}/library-ingestion-jobs/",
        encode_label(library_arn)
    )
}

/// `GetDataAutomationLibraryEntity`'s URI (`POST
/// /data-automation-libraries/{libraryArn}/entityType/{entityType}/entities/{entityId}`).
fn entity_uri(library_arn: &str, entity_type: &str, entity_id: &str) -> String {
    format!(
        "/data-automation-libraries/{}/entityType/{}/entities/{}",
        encode_label(library_arn),
        encode_label(entity_type),
        encode_label(entity_id)
    )
}

/// `ListDataAutomationLibraryEntities`'s URI (`POST
/// /data-automation-libraries/{libraryArn}/entityType/{entityType}/entities/`).
fn entities_uri(library_arn: &str, entity_type: &str) -> String {
    format!(
        "/data-automation-libraries/{}/entityType/{}/entities/",
        encode_label(library_arn),
        encode_label(entity_type)
    )
}

/// A body `InvokeDataAutomationLibraryIngestionJob` accepts every modelled constraint of,
/// with `entityType` and `operationType` chosen by the caller so one row can break exactly
/// one member.
fn ingestion_body(entity_type: &str, operation_type: &str) -> Value {
    json!({
        "entityType": entity_type,
        "operationType": operation_type,
        "inputConfiguration": {
            "inlinePayload": {
                "upsertEntitiesInfo": [
                    {
                        "vocabulary": {
                            "entityId": "entity-1",
                            "language": "EN",
                            "phrases": [{ "text": "a phrase" }],
                        }
                    }
                ]
            }
        },
        "outputConfiguration": { "s3Uri": "s3://a-test-bucket/out/" },
    })
}

/// The one sentence this service states for a refused value: the member path, the
/// requirement, and the value half only where the requirement is a BOUND on a member the
/// model does not withhold.
fn one_error(value: Option<&str>, member: &str, requirement: &str) -> String {
    match value {
        Some(value) => format!(
            "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
             constraint: {requirement}"
        ),
        None => format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             {requirement}"
        ),
    }
}

/// One request carrying a value outside a constraint its member declares.
struct Case {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    /// A value the whole answer must NOT carry, for a clause that states none.
    withheld: Option<String>,
    expected: String,
}

/// The rows: both constraint classes whose requirement is a value SET (`@pattern`, enum)
/// and both classes that are a BOUND (`@length`, `@range`), over members bound to a URI
/// label and to the body, on five operations.
fn cases(library_arn: &str) -> Vec<Case> {
    vec![
        Case {
            case: "libraryArn outside its expression, on the ingestion invoke",
            method: "PUT",
            uri: ingestion_jobs_uri(A_BLUEPRINT_ARN),
            body: ingestion_body("VOCABULARY", "UPSERT"),
            withheld: Some(A_BLUEPRINT_ARN.to_string()),
            expected: one_error(
                None,
                "libraryArn",
                &format!("Member must satisfy regular expression pattern: {LIBRARY_ARN_PATTERN}"),
            ),
        },
        Case {
            case: "libraryArn outside its expression, on the entity read",
            method: "POST",
            uri: entity_uri(A_PROJECT_ARN, "VOCABULARY", "entity-1"),
            body: json!({}),
            withheld: Some(A_PROJECT_ARN.to_string()),
            expected: one_error(
                None,
                "libraryArn",
                &format!("Member must satisfy regular expression pattern: {LIBRARY_ARN_PATTERN}"),
            ),
        },
        Case {
            case: "entityId outside its expression, a second label of the entity read",
            method: "POST",
            uri: entity_uri(library_arn, "VOCABULARY", "not a valid id!"),
            body: json!({}),
            withheld: Some("not a valid id!".to_string()),
            expected: one_error(
                None,
                "entityId",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        },
        Case {
            case: "nextToken outside its expression, a body member of the ingestion-job listing",
            method: "POST",
            uri: ingestion_jobs_uri(library_arn),
            body: json!({ "nextToken": "token with a space" }),
            withheld: Some("token with a space".to_string()),
            expected: one_error(
                None,
                "nextToken",
                &format!("Member must satisfy regular expression pattern: {NEXT_TOKEN_PATTERN}"),
            ),
        },
        Case {
            case: "libraryName outside its expression, a member the model marks @sensitive",
            method: "PUT",
            uri: "/data-automation-libraries/".to_string(),
            body: json!({ "libraryName": "not a valid name!" }),
            withheld: Some("not a valid name!".to_string()),
            expected: one_error(
                None,
                "libraryName",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        },
        Case {
            case: "entityType outside its value set, a body member of the ingestion invoke",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: ingestion_body("NOT_A_REAL_ENTITY_TYPE", "UPSERT"),
            withheld: Some("NOT_A_REAL_ENTITY_TYPE".to_string()),
            expected: one_error(
                None,
                "entityType",
                &format!("Member must satisfy enum value set: [{ENTITY_TYPE_SET}]"),
            ),
        },
        Case {
            case: "entityType outside its value set, a URI label of the entity read",
            method: "POST",
            uri: entity_uri(library_arn, "vocabulary", "entity-1"),
            body: json!({}),
            withheld: None,
            expected: one_error(
                None,
                "entityType",
                &format!("Member must satisfy enum value set: [{ENTITY_TYPE_SET}]"),
            ),
        },
        Case {
            case: "entityType outside its value set, a URI label of the entity listing",
            method: "POST",
            uri: entities_uri(library_arn, "RECOGNIZED-PERSON"),
            body: json!({}),
            withheld: Some("RECOGNIZED-PERSON".to_string()),
            expected: one_error(
                None,
                "entityType",
                &format!("Member must satisfy enum value set: [{ENTITY_TYPE_SET}]"),
            ),
        },
        Case {
            case: "operationType outside its value set: a second enumerated type",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: ingestion_body("VOCABULARY", "REPLACE"),
            withheld: Some("REPLACE".to_string()),
            expected: one_error(
                None,
                "operationType",
                &format!("Member must satisfy enum value set: [{OPERATION_TYPE_SET}]"),
            ),
        },
        Case {
            case: "type outside its value set, on a create of another resource family",
            method: "PUT",
            uri: "/blueprints/".to_string(),
            body: json!({ "blueprintName": "well-formed", "type": "SPREADSHEET", "schema": "{}" }),
            withheld: Some("SPREADSHEET".to_string()),
            expected: one_error(
                None,
                "type",
                &format!("Member must satisfy enum value set: [{BLUEPRINT_TYPE_SET}]"),
            ),
        },
        Case {
            case: "clientToken under its @length floor: a BOUND keeps the value half",
            method: "PUT",
            uri: "/data-automation-libraries/".to_string(),
            body: json!({ "libraryName": "well-formed", "clientToken": "abc" }),
            withheld: None,
            expected: one_error(
                Some("abc"),
                "clientToken",
                "Member must have length greater than or equal to 33",
            ),
        },
        Case {
            case: "maxResults under its @range floor: a BOUND keeps the value half",
            method: "POST",
            uri: ingestion_jobs_uri(library_arn),
            body: json!({ "maxResults": 0 }),
            withheld: None,
            expected: one_error(
                Some("0"),
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
        },
        Case {
            case: "maxResults over its @range ceiling: a BOUND keeps the value half",
            method: "POST",
            uri: entities_uri(library_arn, "VOCABULARY"),
            body: json!({ "maxResults": 1001 }),
            withheld: None,
            expected: one_error(
                Some("1001"),
                "maxResults",
                "Member must have value less than or equal to 1000",
            ),
        },
    ]
}

/// RULE: a value outside a constraint its member declares is refused `400`
/// `ValidationException` naming the member path and the requirement; a clause whose
/// requirement IS the set of values the member admits (a `@pattern` expression, an
/// enumerated type's value set) states NO value, whichever operation, member or binding
/// carried it, while a clause whose requirement is a BOUND on the value keeps the value
/// half.
#[tokio::test]
async fn a_requirement_that_states_the_admissible_set_states_no_value() {
    let router = emulator();
    let library_arn = live_library(&router, "constraint-rows").await;
    for row in cases(&library_arn) {
        let Case {
            case,
            method,
            uri,
            body,
            withheld,
            expected,
        } = row;
        let (status, text, payload) = call(&router, method, &uri, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled ValidationException status 400, got {status} \
             with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload:?}"
        );
        let message = string_at(&payload, "message", case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        if let Some(value) = withheld {
            assert!(
                !text.contains(&value),
                "case {case:?}: expected the refusal to state no value, but it echoed \
                 {value:?}: {text}"
            );
        }
    }
}

/// One request that names an entity type, per operation that carries one.
struct EntityTypeCase {
    case: &'static str,
    method: &'static str,
    /// The URI for one entity type, so the same row can be asked for each admitted value.
    uri: fn(&str, &str) -> String,
    body: fn(&str) -> Value,
}

/// `GetDataAutomationLibraryEntity`'s URI for one entity type.
fn entity_read_uri(library_arn: &str, entity_type: &str) -> String {
    entity_uri(library_arn, entity_type, "entity-1")
}

/// `ListDataAutomationLibraryEntities`'s URI for one entity type.
fn entity_listing_uri(library_arn: &str, entity_type: &str) -> String {
    entities_uri(library_arn, entity_type)
}

/// `InvokeDataAutomationLibraryIngestionJob`'s URI, which names the entity type in the
/// BODY rather than in a label.
fn ingestion_uri(library_arn: &str, _entity_type: &str) -> String {
    ingestion_jobs_uri(library_arn)
}

/// Every operation of this service that names an entity type, and how each carries it.
fn entity_type_cases() -> Vec<EntityTypeCase> {
    vec![
        EntityTypeCase {
            case: "GetDataAutomationLibraryEntity, entity type in a URI label",
            method: "POST",
            uri: entity_read_uri,
            body: |_| json!({}),
        },
        EntityTypeCase {
            case: "ListDataAutomationLibraryEntities, entity type in a URI label",
            method: "POST",
            uri: entity_listing_uri,
            body: |_| json!({}),
        },
        EntityTypeCase {
            case: "InvokeDataAutomationLibraryIngestionJob, entity type in the body",
            method: "PUT",
            uri: ingestion_uri,
            body: |entity_type| ingestion_body(entity_type, "UPSERT"),
        },
    ]
}

/// The values `EntityType` admits (source: `smithy_query(shape="EntityType")`); each is
/// admitted by every operation above.
const ADMITTED_ENTITY_TYPES: [&str; 1] = ["VOCABULARY"];

/// Values outside that set, differing in case, in separator, in being unknown altogether and
/// in being a value the model does not declare.
const REFUSED_ENTITY_TYPES: [&str; 5] = [
    "NOT_A_REAL_ENTITY_TYPE",
    "vocabulary",
    "RECOGNIZED_PERSON",
    "RecognizedPerson",
    "RECOGNIZED PERSON",
];

/// RULE: every value the `EntityType` set admits is admitted ALIKE — no operation refuses
/// one of them for being outside the set, and all of them get the same answer from one
/// operation — while a value outside the set is refused `400` `ValidationException` stating
/// the whole set.
#[tokio::test]
async fn every_admitted_entity_type_is_admitted_alike_and_the_rest_are_refused() {
    let router = emulator();
    let library_arn = live_library(&router, "entity-type-rows").await;
    for row in entity_type_cases() {
        let EntityTypeCase {
            case,
            method,
            uri,
            body,
        } = row;

        let mut answers: Vec<(&str, u16, String)> = Vec::new();
        for entity_type in ADMITTED_ENTITY_TYPES {
            let (status, text, payload) =
                call(&router, method, &uri(&library_arn, entity_type), &body(entity_type)).await;
            assert!(
                !text.contains("enum value set"),
                "case {case:?}: entity type {entity_type:?} is one the set admits, but the \
                 answer refused it against that set: {text}"
            );
            assert!(
                !(status == 400 && error_code(&payload) == "ValidationException"),
                "case {case:?}: entity type {entity_type:?} is one the set admits, but it was \
                 refused: {payload:?}"
            );
            answers.push((entity_type, status, error_code(&payload)));
        }
        let (first_type, first_status, first_code) = answers[0].clone();
        for (entity_type, status, code) in &answers[1..] {
            assert_eq!(
                (*status, code.as_str()),
                (first_status, first_code.as_str()),
                "case {case:?}: entity type {entity_type:?} must be admitted exactly as \
                 {first_type:?} is, but it answered {status} {code:?} against {first_status} \
                 {first_code:?}"
            );
        }

        for entity_type in REFUSED_ENTITY_TYPES {
            let (status, _, payload) =
                call(&router, method, &uri(&library_arn, entity_type), &body(entity_type)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: entity type {entity_type:?} is outside the set, so expected 400, \
                 got {status} with {payload:?}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?}: entity type {entity_type:?} is outside the set, so expected \
                 ValidationException, got {payload:?}"
            );
            let message = string_at(&payload, "message", case);
            let expected = one_error(
                None,
                "entityType",
                &format!("Member must satisfy enum value set: [{ENTITY_TYPE_SET}]"),
            );
            assert_eq!(
                message, expected,
                "case {case:?}: entity type {entity_type:?}: expected {expected:?}, got \
                 {message:?}"
            );
        }
    }
}

/// The requirement every absent `@required` member fails (source:
/// `vendor/smithy-schema-server`'s one sentence for that class, as AWS words it).
const MUST_NOT_BE_NULL: &str = "Member must not be null";

/// One complete `vocabulary` entry of an upsert payload: every member `VocabularyEntityInfo`
/// requires, so a row can drop exactly one of them (source: `smithy_query(query="closure",
/// operation="InvokeDataAutomationLibraryIngestionJob")`).
fn vocabulary(entity_id: &str) -> Value {
    json!({
        "vocabulary": {
            "entityId": entity_id,
            "language": "EN",
            "phrases": [{ "text": "a phrase" }],
        }
    })
}

/// An `InvokeDataAutomationLibraryIngestionJob` body carrying `payload` as the
/// `inlinePayload` variant, and otherwise complete.
fn ingestion_with(payload: Value, operation_type: &str) -> Value {
    json!({
        "entityType": "VOCABULARY",
        "operationType": operation_type,
        "inputConfiguration": { "inlinePayload": payload },
        "outputConfiguration": { "s3Uri": "s3://a-test-bucket/out/" },
    })
}

/// One request that omits exactly one `@required` member.
struct AbsentCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    /// The member path the refusal must name: the model's traversal to the omitted member.
    stated_at: &'static str,
}

/// The rows: a member omitted under one union, under two unions and a list, inside a list
/// nested under a variant, in the union's OTHER variant, at the top level, one and two
/// structures down, in an element of a top-level list, and on another operation entirely.
///
/// Source for every path: `smithy_query(query="closure",
/// operation="InvokeDataAutomationLibraryIngestionJob")` and
/// `smithy_query(query="operation", operation="CreateBlueprint")`.
fn absent_member_cases(library_arn: &str) -> Vec<AbsentCase> {
    vec![
        AbsentCase {
            case: "phrases omitted in the first element of the list variant",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: ingestion_with(
                json!({
                    "upsertEntitiesInfo": [
                        { "vocabulary": { "entityId": "entity-1", "language": "EN" } }
                    ]
                }),
                "UPSERT",
            ),
            stated_at:
                "inputConfiguration.inlinePayload.upsertEntitiesInfo.1.member.vocabulary.phrases",
        },
        AbsentCase {
            case: "language omitted: a second required member of the same structure",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: ingestion_with(
                json!({
                    "upsertEntitiesInfo": [
                        {
                            "vocabulary": {
                                "entityId": "entity-1",
                                "phrases": [{ "text": "a phrase" }],
                            }
                        }
                    ]
                }),
                "UPSERT",
            ),
            stated_at:
                "inputConfiguration.inlinePayload.upsertEntitiesInfo.1.member.vocabulary.language",
        },
        AbsentCase {
            case: "phrases omitted in the SECOND element: the index is that element's position",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: ingestion_with(
                json!({
                    "upsertEntitiesInfo": [
                        vocabulary("entity-1"),
                        { "vocabulary": { "entityId": "entity-2", "language": "EN" } }
                    ]
                }),
                "UPSERT",
            ),
            stated_at:
                "inputConfiguration.inlinePayload.upsertEntitiesInfo.2.member.vocabulary.phrases",
        },
        AbsentCase {
            case: "text omitted in a phrase: a list nested under the variant",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: ingestion_with(
                json!({
                    "upsertEntitiesInfo": [
                        {
                            "vocabulary": {
                                "entityId": "entity-1",
                                "language": "EN",
                                "phrases": [{ "displayAsText": "as shown" }],
                            }
                        }
                    ]
                }),
                "UPSERT",
            ),
            stated_at: "inputConfiguration.inlinePayload.upsertEntitiesInfo.1.member.vocabulary.\
                        phrases.1.member.text",
        },
        AbsentCase {
            case: "entityIds omitted in the union's other variant",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: ingestion_with(json!({ "deleteEntitiesInfo": {} }), "DELETE"),
            stated_at: "inputConfiguration.inlinePayload.deleteEntitiesInfo.entityIds",
        },
        AbsentCase {
            case: "s3Uri omitted: a nested structure crossing no union",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: json!({
                "entityType": "VOCABULARY",
                "operationType": "UPSERT",
                "inputConfiguration": {
                    "inlinePayload": { "upsertEntitiesInfo": [vocabulary("entity-1")] }
                },
                "outputConfiguration": {},
            }),
            stated_at: "outputConfiguration.s3Uri",
        },
        AbsentCase {
            case: "outputConfiguration omitted altogether: a top-level required member",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: json!({
                "entityType": "VOCABULARY",
                "operationType": "UPSERT",
                "inputConfiguration": {
                    "inlinePayload": { "upsertEntitiesInfo": [vocabulary("entity-1")] }
                },
            }),
            stated_at: "outputConfiguration",
        },
        AbsentCase {
            case: "eventBridgeEnabled omitted two structures down",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: json!({
                "entityType": "VOCABULARY",
                "operationType": "UPSERT",
                "inputConfiguration": {
                    "inlinePayload": { "upsertEntitiesInfo": [vocabulary("entity-1")] }
                },
                "outputConfiguration": { "s3Uri": "s3://a-test-bucket/out/" },
                "notificationConfiguration": { "eventBridgeConfiguration": {} },
            }),
            stated_at: "notificationConfiguration.eventBridgeConfiguration.eventBridgeEnabled",
        },
        AbsentCase {
            case: "value omitted in the second tag: an element of a top-level list",
            method: "PUT",
            uri: ingestion_jobs_uri(library_arn),
            body: json!({
                "entityType": "VOCABULARY",
                "operationType": "UPSERT",
                "inputConfiguration": {
                    "inlinePayload": { "upsertEntitiesInfo": [vocabulary("entity-1")] }
                },
                "outputConfiguration": { "s3Uri": "s3://a-test-bucket/out/" },
                "tags": [{ "key": "team", "value": "platform" }, { "key": "owner" }],
            }),
            stated_at: "tags.2.member.value",
        },
        AbsentCase {
            case: "schema omitted on a create of another resource family",
            method: "PUT",
            uri: "/blueprints/".to_string(),
            body: json!({ "blueprintName": "well-formed", "type": "DOCUMENT" }),
            stated_at: "schema",
        },
    ]
}

/// RULE: a member the request did not carry is refused `400` `ValidationException` whose
/// clause states NO value — there is no submitted value to state — and names the member at
/// the path the MODEL traverses to it: every structure and union member name of that
/// traversal, with each list element named by its 1-based position.
#[tokio::test]
async fn an_absent_required_member_is_named_at_the_path_the_model_traverses() {
    let router = emulator();
    let library_arn = live_library(&router, "absent-member-rows").await;
    for row in absent_member_cases(&library_arn) {
        let AbsentCase {
            case,
            method,
            uri,
            body,
            stated_at,
        } = row;
        let (status, text, payload) = call(&router, method, &uri, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled ValidationException status 400, got {status} \
             with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload:?}"
        );
        let message = string_at(&payload, "message", case);
        let expected = one_error(None, stated_at, MUST_NOT_BE_NULL);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !text.contains("Value null"),
            "case {case:?}: expected the refusal to state no value, but it stated one: {text}"
        );
    }
}
