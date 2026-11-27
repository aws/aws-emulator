// SPDX-License-Identifier: CC0-1.0

//! The dataset lifecycle, end to end over the wire: `CreateDataset`, `DescribeDataset`,
//! `StartDataIngestionJob`, `DescribeDataIngestionJob`, `ListDatasets`, `DeleteDataset`.
//!
//! Every call goes through the real `MultiServiceRouter` the production emulator mounts
//! (`POST /` with `X-Amz-Target: AWSLookoutEquipmentFrontendService.<Op>`, awsJson1_0, the
//! signing name `lookoutequipment` — OPERATIONS.md, "Wire protocol"), so what these tests
//! assert is what an SDK client sees: every response is built from the persisted record,
//! not from the request.
//!
//! The tables below span the classes the rules generalise over: the `DatasetName`
//! character classes and its length bound, every form `ServerSideKmsKeyId` accepts, the
//! optional members of `IngestionS3InputConfiguration`, page sizes from 1 to past the item
//! count plus both published `MaxResults` bounds and one past each, and prefixes that
//! select all, some and none of the stored datasets.
//!
//! The last two tests widen the table to the whole operation surface of the model this
//! crate is built from: every operation the service binds is routed to its own handler,
//! and the six implemented here answer their modelled output.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_lookoutequipment_awslookoutequipmentfrontendservice::state;
use service_com_amazonaws_lookoutequipment_awslookoutequipmentfrontendservice::EmulatedService;
use service_com_amazonaws_lookoutequipment_awslookoutequipmentfrontendservice::ComAmazonawsLookoutequipmentAwslookoutequipmentfrontendserviceState as Store;

/// The region these tests sign for; the service reads it off the SigV4 credential scope,
/// so it has to appear in every ARN it mints.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// Comfortably longer than [`state::TRANSITION_SPAN`], so a transition the clock owes has
/// certainly come due by the time the next call runs.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// A router serving this one service over a fresh engine, as `build_router` does in
/// production.
fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The same router, with the shared engine it was built over — so a test can seed a store
/// the API would take a very long time to fill.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Call one operation and return `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/lookoutequipment/\
                 aws4_request, SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000"
            ),
        )
        .header("host", format!("lookoutequipment.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AWSLookoutEquipmentFrontendService.{operation}"),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain: the call must answer 200.
async fn ok_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected 200, got {status}: {parsed}"
    );
    parsed
}

/// The wire error code (`__type`) of an awsJson error body, without its namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A member as a string, or a panic naming the case.
fn text(body: &Value, member: &str, case: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a string {member}, got {body}"))
        .to_string()
}

/// Whether `arn` is the `DatasetArn` this emulator must mint for `dataset_name`:
/// `arn:aws:lookoutequipment:<region>:<account>:dataset/<name>/<tail>`, per `DatasetArn`'s
/// modelled `@pattern`
/// `^arn:aws(-[^:]+)?:lookoutequipment:[a-zA-Z0-9\-]*:[0-9]{12}:dataset\/[0-9a-zA-Z_-]{1,200}\/.+$`
/// and `@length(20, 2048)` (source: `smithy_query(query="operation",
/// operation="CreateDataset")`). Checked by hand because this crate has no regex
/// dependency.
fn is_dataset_arn(arn: &str, dataset_name: &str) -> bool {
    let prefix = format!("arn:aws:lookoutequipment:{REGION}:{ACCOUNT}:dataset/{dataset_name}/");
    (20..=2048).contains(&arn.len())
        && arn.starts_with(&prefix)
        && !arn[prefix.len()..].is_empty()
}

/// Whether `job_id` satisfies `IngestionJobId`'s `^[A-Fa-f0-9]{0,32}$` with `@length(0,
/// 32)`, and is the full 32 characters a started job carries.
fn is_job_id(job_id: &str) -> bool {
    job_id.len() == 32 && job_id.chars().all(|c| c.is_ascii_hexdigit())
}

/// A client token that is unique within a test but carries no information the service may
/// derive a response from.
fn token(label: &str) -> String {
    format!("token-{label}")
}

/// One create case: the dataset name, the inline schema, the `ServerSideKmsKeyId` form and
/// the tags.
struct CreateCase {
    name: String,
    schema: Option<Value>,
    kms: Option<String>,
    /// The `KmsKeyArn` `DescribeDataset` must answer with for `kms`.
    expected_kms: Option<String>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The create table: rows differ along `DatasetName`'s character classes and its 1..200
/// length bound (`^[0-9a-zA-Z_-]{1,200}$`), whether an inline schema was supplied, and
/// every form `ServerSideKmsKeyId`'s `NameOrArn` accepts — a bare key id, an alias and a
/// full key ARN — plus none at all. Sources: the `DatasetName`, `ServerSideKmsKeyId` and
/// `DatasetSchema` members reported by `smithy_query(query="operation",
/// operation="CreateDataset")`.
fn create_cases() -> Vec<CreateCase> {
    let key_id = "12345678-1234-1234-1234-123456789012";
    let key_arn = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{key_id}");
    vec![
        CreateCase {
            name: "demo-dataset".to_string(),
            schema: Some(json!({
                "Components": [{
                    "ComponentName": "pump",
                    "Columns": [
                        { "Name": "Timestamp", "Type": "DATETIME" },
                        { "Name": "pressure", "Type": "DOUBLE" }
                    ]
                }]
            })),
            kms: None,
            expected_kms: None,
            tags: vec![],
        },
        CreateCase {
            // Digits and underscores: the other classes `DatasetName` allows.
            name: "Dataset_0123".to_string(),
            schema: Some(json!({ "Components": [] })),
            kms: Some(key_id.to_string()),
            expected_kms: Some(key_arn.clone()),
            tags: vec![("line", "A")],
        },
        CreateCase {
            name: "aliased".to_string(),
            schema: None,
            kms: Some("alias/lookout".to_string()),
            expected_kms: Some(format!("arn:aws:kms:{REGION}:{ACCOUNT}:alias/lookout")),
            tags: vec![("line", "B"), ("env", "test")],
        },
        CreateCase {
            // The published maximum `DatasetName` length.
            name: "z".repeat(200),
            schema: Some(json!({ "Components": [{ "ComponentName": "c", "Columns": [] }] })),
            kms: Some(key_arn.clone()),
            // Already an ARN, so it is reported unchanged.
            expected_kms: Some(key_arn),
            tags: vec![],
        },
    ]
}

/// The `CreateDataset` request body for a case.
fn create_body(case: &CreateCase, client_token: &str) -> Value {
    let mut body = json!({
        "DatasetName": case.name,
        "ClientToken": client_token,
    });
    if let Some(schema) = &case.schema {
        body["DatasetSchema"] = json!({
            "InlineDataSchema": serde_json::to_string(schema).expect("the schema serializes"),
        });
    }
    if let Some(kms) = &case.kms {
        body["ServerSideKmsKeyId"] = json!(kms);
    }
    if !case.tags.is_empty() {
        body["Tags"] = Value::Array(
            case.tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    body
}

/// A create is answered from the record it stored: `DescribeDataset` reports the same ARN,
/// status, inline schema and resolved KMS key, for every row of the table.
#[tokio::test]
async fn create_persists_the_dataset_and_describe_answers_the_same_record() {
    let router = router();

    for case in create_cases() {
        let created = ok_call(
            &router,
            "CreateDataset",
            &create_body(&case, &token(&case.name)),
        )
        .await;

        assert_eq!(
            created.get("DatasetName").and_then(Value::as_str),
            Some(case.name.as_str()),
            "case {}: CreateDataset must echo the stored name, got {created}",
            case.name
        );
        assert_eq!(
            created.get("Status").and_then(Value::as_str),
            Some(state::DATASET_STATUS_CREATED),
            "case {}: a fresh dataset is {}, got {created}",
            case.name,
            state::DATASET_STATUS_CREATED
        );
        let arn = text(&created, "DatasetArn", &case.name);
        assert!(
            is_dataset_arn(&arn, &case.name),
            "case {}: expected a DatasetArn for this dataset, got {arn:?}",
            case.name
        );

        let described = ok_call(
            &router,
            "DescribeDataset",
            &json!({ "DatasetName": case.name }),
        )
        .await;

        assert_eq!(
            described.get("DatasetArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {}: DescribeDataset must answer the stored ARN, got {described}",
            case.name
        );
        assert_eq!(
            described.get("Status").and_then(Value::as_str),
            Some(state::DATASET_STATUS_CREATED),
            "case {}: expected the stored status, got {described}",
            case.name
        );
        let expected_schema = case
            .schema
            .as_ref()
            .map(|schema| serde_json::to_string(schema).expect("the schema serializes"));
        assert_eq!(
            described.get("Schema").and_then(Value::as_str),
            expected_schema.as_deref(),
            "case {}: expected the inline schema supplied at create time, got {described}",
            case.name
        );
        assert_eq!(
            described
                .get("ServerSideKmsKeyId")
                .and_then(Value::as_str),
            case.expected_kms.as_deref(),
            "case {}: expected the resolved KMS key ARN, got {described}",
            case.name
        );
        assert!(
            described.get("CreatedAt").and_then(Value::as_f64).is_some()
                && described
                    .get("LastUpdatedAt")
                    .and_then(Value::as_f64)
                    .is_some(),
            "case {}: expected CreatedAt and LastUpdatedAt timestamps, got {described}",
            case.name
        );
        // Nothing has ingested into this dataset, so the ingestion-derived members are
        // absent rather than invented.
        for member in ["IngestionInputConfiguration", "RoleArn", "DataStartTime"] {
            assert!(
                described.get(member).is_none(),
                "case {}: {member} must stay absent before any ingestion, got {described}",
                case.name
            );
        }

        // A second read answers the same timestamps: a read never moves the record.
        let again = ok_call(
            &router,
            "DescribeDataset",
            &json!({ "DatasetName": case.name }),
        )
        .await;
        assert_eq!(
            again.get("CreatedAt"),
            described.get("CreatedAt"),
            "case {}: CreatedAt must be stable across reads, got {again}",
            case.name
        );
    }
}

/// Replaying a `ClientToken` answers the first result; a new token on an existing name is
/// the modelled 409. Both hold for every row of the create table, and state keeps exactly
/// one dataset per name.
#[tokio::test]
async fn a_replayed_client_token_returns_the_original_and_a_new_one_conflicts() {
    let router = router();

    for case in create_cases() {
        let first = ok_call(
            &router,
            "CreateDataset",
            &create_body(&case, &token(&case.name)),
        )
        .await;

        let replay = ok_call(
            &router,
            "CreateDataset",
            &create_body(&case, &token(&case.name)),
        )
        .await;
        assert_eq!(
            replay, first,
            "case {}: replaying the ClientToken must answer the original body, got {replay}",
            case.name
        );

        let (status, conflict) = call(
            &router,
            "CreateDataset",
            &create_body(&case, &token(&format!("{}-second", case.name))),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {}: a new ClientToken on an existing name is a conflict, got {status}: {conflict}",
            case.name
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {}: expected ConflictException, got {conflict}",
            case.name
        );
    }

    // Exactly one dataset per name survived all of that.
    let listed = ok_call(&router, "ListDatasets", &json!({ "MaxResults": 500 })).await;
    let names: Vec<String> = summary_names(&listed);
    let expected: Vec<String> = {
        let mut names: Vec<String> = create_cases().into_iter().map(|case| case.name).collect();
        names.sort();
        names
    };
    assert_eq!(
        names, expected,
        "expected exactly one dataset per created name, got {listed}"
    );
}

/// Every operation of this slice answers the modelled 404 for an identifier that is not in
/// state — never a 501 and never an empty 200.
#[tokio::test]
async fn an_identifier_that_is_not_in_state_is_a_resource_not_found() {
    let router = router();

    // Rows differ along the operation and the kind of identifier it keys on.
    let cases: Vec<(&str, Value)> = vec![
        ("DescribeDataset", json!({ "DatasetName": "never-created" })),
        ("DeleteDataset", json!({ "DatasetName": "never-created" })),
        (
            "DescribeDataIngestionJob",
            json!({ "JobId": "0123456789abcdef0123456789abcdef" }),
        ),
        (
            "StartDataIngestionJob",
            json!({
                "DatasetName": "never-created",
                "IngestionInputConfiguration": { "S3InputConfiguration": { "Bucket": "sensor-data" } },
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/LookoutRole"),
                "ClientToken": token("missing-dataset"),
            }),
        ),
    ];

    for (operation, body) in cases {
        let (status, parsed) = call(&router, operation, &body).await;
        assert_eq!(
            status, 404,
            "case {operation}: expected 404 for an unknown identifier, got {status}: {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {operation}: expected ResourceNotFoundException, got {parsed}"
        );
    }
}

/// The ingestion input configurations the table covers: rows differ along the optional
/// members of `IngestionS3InputConfiguration` (`Prefix`, `KeyPattern`) and the bucket name,
/// which is what the job and its parent dataset must report back verbatim. Source:
/// `smithy_query(query="shape",shape="IngestionS3InputConfiguration")`.
fn ingestion_configurations() -> Vec<(&'static str, Value)> {
    vec![
        ("bucket only", json!({ "Bucket": "sensor-data" })),
        (
            "bucket and prefix",
            json!({ "Bucket": "sensor-data", "Prefix": "pump/raw/" }),
        ),
        (
            "bucket, prefix and key pattern",
            json!({
                "Bucket": "a.b-c123",
                "Prefix": "site-1/",
                "KeyPattern": "{prefix}/{component_name}/*",
            }),
        ),
    ]
}

/// `StartDataIngestionJob` persists the job and records its configuration on the parent
/// dataset; both reads answer what was submitted, and both settle to their terminal values
/// once the clock owes them.
#[tokio::test]
async fn ingestion_records_its_configuration_on_the_job_and_on_the_dataset() {
    for (case, s3) in ingestion_configurations() {
        let router = router();
        let dataset = "ingested";
        let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/LookoutRole");
        ok_call(
            &router,
            "CreateDataset",
            &json!({ "DatasetName": dataset, "ClientToken": token(case) }),
        )
        .await;

        let input_configuration = json!({ "S3InputConfiguration": s3 });
        let started = ok_call(
            &router,
            "StartDataIngestionJob",
            &json!({
                "DatasetName": dataset,
                "IngestionInputConfiguration": input_configuration,
                "RoleArn": role_arn,
                "ClientToken": token(&format!("{case}-ingest")),
            }),
        )
        .await;

        let job_id = text(&started, "JobId", case);
        assert!(
            is_job_id(&job_id),
            "case {case}: expected a 32-hex-character JobId, got {job_id:?}"
        );
        assert_eq!(
            started.get("Status").and_then(Value::as_str),
            Some(state::INGESTION_STATUS_IN_PROGRESS),
            "case {case}: a started job is {}, got {started}",
            state::INGESTION_STATUS_IN_PROGRESS
        );

        // Straight after the call, the transient states are observable: the span the clock
        // owes is never zero.
        let job = ok_call(
            &router,
            "DescribeDataIngestionJob",
            &json!({ "JobId": job_id }),
        )
        .await;
        assert_eq!(
            job.get("Status").and_then(Value::as_str),
            Some(state::INGESTION_STATUS_IN_PROGRESS),
            "case {case}: expected the transient job status, got {job}"
        );
        assert_eq!(
            job.get("IngestionInputConfiguration"),
            Some(&input_configuration),
            "case {case}: the job must report the configuration submitted, got {job}"
        );
        assert_eq!(
            job.get("RoleArn").and_then(Value::as_str),
            Some(role_arn.as_str()),
            "case {case}: the job must report the role submitted, got {job}"
        );
        let dataset_arn = text(&job, "DatasetArn", case);
        assert!(
            is_dataset_arn(&dataset_arn, dataset),
            "case {case}: expected the parent dataset's ARN, got {dataset_arn:?}"
        );
        assert!(
            job.get("CreatedAt").and_then(Value::as_f64).is_some(),
            "case {case}: expected a CreatedAt on the job, got {job}"
        );

        let mid = ok_call(
            &router,
            "DescribeDataset",
            &json!({ "DatasetName": dataset }),
        )
        .await;
        assert_eq!(
            mid.get("Status").and_then(Value::as_str),
            Some(state::DATASET_STATUS_INGESTION_IN_PROGRESS),
            "case {case}: expected the transient dataset status, got {mid}"
        );
        assert_eq!(
            mid.get("IngestionInputConfiguration"),
            Some(&input_configuration),
            "case {case}: the dataset must report the ingestion configuration, got {mid}"
        );
        assert_eq!(
            mid.get("RoleArn").and_then(Value::as_str),
            Some(role_arn.as_str()),
            "case {case}: the dataset must report the ingestion role, got {mid}"
        );

        // Starting a second ingestion while one is still owed conflicts with the dataset's
        // current state.
        let (status, conflict) = call(
            &router,
            "StartDataIngestionJob",
            &json!({
                "DatasetName": dataset,
                "IngestionInputConfiguration": input_configuration,
                "RoleArn": role_arn,
                "ClientToken": token(&format!("{case}-concurrent")),
            }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case}: a second ingestion mid-flight is a conflict, got {status}: {conflict}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;

        let settled_job = ok_call(
            &router,
            "DescribeDataIngestionJob",
            &json!({ "JobId": job_id }),
        )
        .await;
        assert_eq!(
            settled_job.get("Status").and_then(Value::as_str),
            Some(state::INGESTION_STATUS_SUCCESS),
            "case {case}: the job must settle to {}, got {settled_job}",
            state::INGESTION_STATUS_SUCCESS
        );
        assert_eq!(
            settled_job.get("CreatedAt"),
            job.get("CreatedAt"),
            "case {case}: settling must not move CreatedAt, got {settled_job}"
        );

        let settled = ok_call(
            &router,
            "DescribeDataset",
            &json!({ "DatasetName": dataset }),
        )
        .await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some(state::DATASET_STATUS_ACTIVE),
            "case {case}: the dataset must settle to {}, got {settled}",
            state::DATASET_STATUS_ACTIVE
        );
        assert_eq!(
            settled.get("IngestionInputConfiguration"),
            Some(&input_configuration),
            "case {case}: settling must not drop the ingestion configuration, got {settled}"
        );
        assert_eq!(
            ok_call(
                &router,
                "DescribeDataset",
                &json!({ "DatasetName": dataset })
            )
            .await
            .get("LastUpdatedAt"),
            settled.get("LastUpdatedAt"),
            "case {case}: LastUpdatedAt must be stable once settled, got {settled}"
        );

        // With the dataset settled, a further ingestion is accepted again.
        let next = ok_call(
            &router,
            "StartDataIngestionJob",
            &json!({
                "DatasetName": dataset,
                "IngestionInputConfiguration": input_configuration,
                "RoleArn": role_arn,
                "ClientToken": token(&format!("{case}-again")),
            }),
        )
        .await;
        assert_ne!(
            text(&next, "JobId", case),
            job_id,
            "case {case}: a second ingestion must mint a different JobId, got {next}"
        );
    }
}

/// A replayed `StartDataIngestionJob` `ClientToken` answers the first job instead of
/// starting a second.
#[tokio::test]
async fn a_replayed_ingestion_client_token_returns_the_original_job() {
    let router = router();
    let dataset = "replayed";
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/LookoutRole");
    ok_call(
        &router,
        "CreateDataset",
        &json!({ "DatasetName": dataset, "ClientToken": token("replayed") }),
    )
    .await;

    let body = json!({
        "DatasetName": dataset,
        "IngestionInputConfiguration": { "S3InputConfiguration": { "Bucket": "sensor-data" } },
        "RoleArn": role_arn,
        "ClientToken": token("replayed-ingest"),
    });
    let first = ok_call(&router, "StartDataIngestionJob", &body).await;
    let replay = ok_call(&router, "StartDataIngestionJob", &body).await;
    assert_eq!(
        replay, first,
        "case replay: the same ClientToken must answer the original job, got {replay}"
    );
}

/// The dataset names a page walk is run over: distinct prefixes, so the same table drives
/// the prefix filter.
const PAGED_NAMES: &[&str] = &[
    "demo-alpha",
    "demo-beta",
    "demo-gamma",
    "other-delta",
    "other-epsilon",
];

/// The `DatasetName`s of a `ListDatasets` body, in the order returned.
fn summary_names(body: &Value) -> Vec<String> {
    body.get("DatasetSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected DatasetSummaries, got {body}"))
        .iter()
        .map(|summary| {
            summary
                .get("DatasetName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected a DatasetName in {summary}"))
                .to_string()
        })
        .collect()
}

/// Walk every page at `max_results` and return the names seen, in order.
async fn walk_pages(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    prefix: Option<&str>,
) -> Vec<String> {
    let case = format!("MaxResults={max_results:?} prefix={prefix:?}");
    let mut seen = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(prefix) = prefix {
            body["DatasetNameBeginsWith"] = json!(prefix);
        }
        if let Some(cursor) = &next_token {
            body["NextToken"] = json!(cursor);
        }
        let page = ok_call(router, "ListDatasets", &body).await;
        let names = summary_names(&page);
        if let Some(max) = max_results {
            assert!(
                names.len() <= max as usize,
                "case {case}: a page must hold at most MaxResults items, got {names:?}"
            );
        }
        seen.extend(names);
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            seen.len() < PAGED_NAMES.len() + 1,
            "case {case}: a NextToken was issued past the last dataset, saw {seen:?}"
        );
    }
    seen
}

/// Paging covers every dataset exactly once at every page size, emits a `NextToken` only
/// while more remain, and honours the prefix filter.
#[tokio::test]
async fn list_datasets_pages_and_filters_over_the_stored_datasets() {
    let router = router();
    for name in PAGED_NAMES {
        ok_call(
            &router,
            "CreateDataset",
            &json!({ "DatasetName": name, "ClientToken": token(name) }),
        )
        .await;
    }

    let mut all: Vec<String> = PAGED_NAMES.iter().map(|name| name.to_string()).collect();
    all.sort();

    // Page sizes 1 through one past the item count, plus both published `MaxResults`
    // bounds (1..500) and no `MaxResults` at all.
    let sizes: Vec<Option<i32>> = (1..=PAGED_NAMES.len() as i32 + 1)
        .map(Some)
        .chain([Some(500), None])
        .collect();
    for max_results in sizes {
        let seen = walk_pages(&router, max_results, None).await;
        assert_eq!(
            seen, all,
            "case MaxResults={max_results:?}: paging must return every dataset exactly once, \
             in name order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            seen.len(),
            "case MaxResults={max_results:?}: paging must not repeat a dataset, got {seen:?}"
        );
    }

    // Prefix rows: all, some, one, and none of the stored datasets.
    let prefix_cases: Vec<(&str, Vec<String>)> = vec![
        (
            "demo",
            vec![
                "demo-alpha".to_string(),
                "demo-beta".to_string(),
                "demo-gamma".to_string(),
            ],
        ),
        ("other", vec!["other-delta".to_string(), "other-epsilon".to_string()]),
        ("demo-alpha", vec!["demo-alpha".to_string()]),
        ("nothing", vec![]),
    ];
    for (prefix, expected) in prefix_cases {
        // Paged at 2, so the filter and the cursor have to compose.
        let seen = walk_pages(&router, Some(2), Some(prefix)).await;
        assert_eq!(
            seen, expected,
            "case prefix={prefix:?}: expected only the matching datasets"
        );
    }
}

/// `MaxResults` outside its published `@range(1, 500)` is refused at the boundary, so no
/// page is served for it.
#[tokio::test]
async fn max_results_past_a_published_bound_is_refused() {
    let router = router();
    for max_results in [0, 501] {
        let (status, parsed) = call(
            &router,
            "ListDatasets",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected a 400 outside @range(1, 500), \
             got {status}: {parsed}"
        );
    }
}

/// A `NextToken` this service never issued names no position to resume from.
#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let router = router();
    ok_call(
        &router,
        "CreateDataset",
        &json!({ "DatasetName": "only", "ClientToken": token("only") }),
    )
    .await;

    // Rows: not hex at all, hex of text without the cursor prefix, and hex of a different
    // service's cursor text.
    for bad in [
        "not-a-token",
        &hex::encode("demo-alpha"),
        &hex::encode("osis-after-pipeline:demo-alpha"),
    ] {
        let (status, parsed) = call(&router, "ListDatasets", &json!({ "NextToken": bad })).await;
        assert_eq!(
            status, 400,
            "case NextToken={bad:?}: expected a 400, got {status}: {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case NextToken={bad:?}: expected ValidationException, got {parsed}"
        );
    }

    // The empty token is a legal `NextToken` (`@length(0, 8192)`) and names the start.
    let page = ok_call(&router, "ListDatasets", &json!({ "NextToken": "" })).await;
    assert_eq!(
        summary_names(&page),
        vec!["only".to_string()],
        "case NextToken=\"\": the empty token starts at the beginning, got {page}"
    );
}

/// An inline schema that is not a JSON document cannot be reported back as the modelled
/// `application/json` `Schema`, so it is refused rather than stored.
#[tokio::test]
async fn an_inline_schema_that_is_not_json_is_refused() {
    let router = router();

    // Rows differ along the way the text fails to be JSON.
    for (case, inline) in [
        ("bare word", "not json"),
        ("truncated object", "{\"Components\":"),
        ("trailing comma", "{\"Components\": [],}"),
    ] {
        let (status, parsed) = call(
            &router,
            "CreateDataset",
            &json!({
                "DatasetName": "bad-schema",
                "ClientToken": token(case),
                "DatasetSchema": { "InlineDataSchema": inline },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 for a non-JSON inline schema, got {status}: {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected ValidationException, got {parsed}"
        );
    }

    // Nothing was stored for any of those.
    let (status, parsed) = call(
        &router,
        "DescribeDataset",
        &json!({ "DatasetName": "bad-schema" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a refused create must store nothing, got {status}: {parsed}"
    );
}

/// The ingestion-job store is bounded: a dataset already holding the ceiling refuses a
/// further start with the modelled `ServiceQuotaExceededException`, so a caller looping on
/// start cannot grow the store without limit.
#[tokio::test]
async fn ingestion_jobs_are_capped_per_dataset() {
    let (router, engine) = router_with_engine();
    let dataset = "capped";
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/LookoutRole");
    ok_call(
        &router,
        "CreateDataset",
        &json!({ "DatasetName": dataset, "ClientToken": token("capped") }),
    )
    .await;

    // Fill the dataset's job list to the ceiling directly: the API caps starts to one per
    // transition span per dataset, so reaching the ceiling through it would take minutes.
    // The ceiling is on the COUNT the dataset holds, which is what this seeds.
    {
        let store = engine.state::<Store>();
        let mut guard = store.lock();
        let record = guard
            .datasets
            .get_mut(dataset)
            .expect("the dataset was just created");
        for index in 0..state::MAX_INGESTION_JOBS_PER_DATASET {
            record.ingestion_job_ids.push(format!("{index:032x}"));
        }
    }

    let (status, parsed) = call(
        &router,
        "StartDataIngestionJob",
        &json!({
            "DatasetName": dataset,
            "IngestionInputConfiguration": { "S3InputConfiguration": { "Bucket": "sensor-data" } },
            "RoleArn": role_arn,
            "ClientToken": token("capped-ingest"),
        }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case at the per-dataset ceiling: expected ServiceQuotaExceededException's 402, \
         got {status}: {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ServiceQuotaExceededException",
        "case at the per-dataset ceiling: expected ServiceQuotaExceededException, got {parsed}"
    );
}

/// Delete removes the dataset and the ingestion jobs that belonged to it, and frees the
/// name for a later create.
#[tokio::test]
async fn delete_removes_the_dataset_and_its_ingestion_jobs() {
    let router = router();
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/LookoutRole");

    // Rows: a dataset that never ingested, and one that did.
    for (case, ingest) in [("no ingestion", false), ("after ingestion", true)] {
        let dataset = "deletable";
        ok_call(
            &router,
            "CreateDataset",
            &json!({ "DatasetName": dataset, "ClientToken": token(case) }),
        )
        .await;

        let job_id = if ingest {
            let started = ok_call(
                &router,
                "StartDataIngestionJob",
                &json!({
                    "DatasetName": dataset,
                    "IngestionInputConfiguration": {
                        "S3InputConfiguration": { "Bucket": "sensor-data" }
                    },
                    "RoleArn": role_arn,
                    "ClientToken": token(&format!("{case}-ingest")),
                }),
            )
            .await;
            Some(text(&started, "JobId", case))
        } else {
            None
        };

        let (status, deleted) = call(
            &router,
            "DeleteDataset",
            &json!({ "DatasetName": dataset }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected DeleteDataset 200, got {status}: {deleted}"
        );
        assert!(
            deleted.is_null() || deleted == json!({}),
            "case {case}: DeleteDataset answers an empty body, got {deleted}"
        );

        let (status, parsed) = call(
            &router,
            "DescribeDataset",
            &json!({ "DatasetName": dataset }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: a deleted dataset must 404, got {status}: {parsed}"
        );

        let listed = ok_call(&router, "ListDatasets", &json!({})).await;
        assert!(
            !summary_names(&listed).contains(&dataset.to_string()),
            "case {case}: a deleted dataset must leave the listing, got {listed}"
        );

        if let Some(job_id) = job_id {
            let (status, parsed) = call(
                &router,
                "DescribeDataIngestionJob",
                &json!({ "JobId": job_id }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case}: the deleted dataset's job must 404, got {status}: {parsed}"
            );
        }

        // Deleting again is a 404: the record really is gone.
        let (status, parsed) = call(
            &router,
            "DeleteDataset",
            &json!({ "DatasetName": dataset }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: deleting twice must 404, got {status}: {parsed}"
        );
    }

    // The name is free again, and a fresh create mints a NEW ARN rather than reviving the
    // old record.
    let recreated = ok_call(
        &router,
        "CreateDataset",
        &json!({ "DatasetName": "deletable", "ClientToken": token("recreate") }),
    )
    .await;
    assert_eq!(
        recreated.get("Status").and_then(Value::as_str),
        Some(state::DATASET_STATUS_CREATED),
        "case recreate: a recreated dataset starts at {}, got {recreated}",
        state::DATASET_STATUS_CREATED
    );
}

/// Every operation `AWSLookoutEquipmentFrontendService` binds, in the order the service
/// shape lists them. Written out by hand from OPERATIONS.md, which carries one
/// `## <Operation>` heading per bound operation; the same list comes from
/// `smithy_query(query="list_operations")` over this crate's `smithy-model/`.
const BOUND_OPERATIONS: [&str; 49] = [
    "CreateDataset",
    "CreateInferenceScheduler",
    "CreateLabel",
    "CreateLabelGroup",
    "CreateModel",
    "CreateRetrainingScheduler",
    "DeleteDataset",
    "DeleteInferenceScheduler",
    "DeleteLabel",
    "DeleteLabelGroup",
    "DeleteModel",
    "DeleteResourcePolicy",
    "DeleteRetrainingScheduler",
    "DescribeDataIngestionJob",
    "DescribeDataset",
    "DescribeInferenceScheduler",
    "DescribeLabel",
    "DescribeLabelGroup",
    "DescribeModel",
    "DescribeModelVersion",
    "DescribeResourcePolicy",
    "DescribeRetrainingScheduler",
    "ImportDataset",
    "ImportModelVersion",
    "ListDataIngestionJobs",
    "ListDatasets",
    "ListInferenceEvents",
    "ListInferenceExecutions",
    "ListInferenceSchedulers",
    "ListLabelGroups",
    "ListLabels",
    "ListModels",
    "ListModelVersions",
    "ListRetrainingSchedulers",
    "ListSensorStatistics",
    "ListTagsForResource",
    "PutResourcePolicy",
    "StartDataIngestionJob",
    "StartInferenceScheduler",
    "StartRetrainingScheduler",
    "StopInferenceScheduler",
    "StopRetrainingScheduler",
    "TagResource",
    "UntagResource",
    "UpdateActiveModelVersion",
    "UpdateInferenceScheduler",
    "UpdateLabelGroup",
    "UpdateModel",
    "UpdateRetrainingScheduler",
];

/// A target name the model declares no operation for — the control row of the table
/// below. Without it, a check that every bound operation is routed could pass while
/// nothing was routed at all.
const UNBOUND_OPERATION: &str = "DescribeEquipmentHealth";

/// The `message` member of an answer, or the empty string when it carries none.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether the answer came from one of this service's own operations, rather than from
/// the framework refusing a request that named no modelled operation.
///
/// A bound operation answers 200, one of its modelled errors, or — while it is still a
/// scaffold — a 501 that calls itself scaffolded. A name the service binds no operation
/// for never reaches a handler at all, and the refusal it earns does not claim to be a
/// scaffold.
fn reached_an_operation(status: u16, body: &Value) -> bool {
    status != 501 || message(body).contains("scaffolded")
}

/// Every operation the model binds is routed to that operation's own handler.
///
/// The model carries AWS's end-of-support `@deprecated` trait on all 49 operations.
/// Deprecation is documentation: it unbinds nothing and changes no wire contract, so
/// every name below must still resolve to its handler — and the one name the model binds
/// nothing for must still be refused by routing.
#[tokio::test]
async fn every_operation_the_model_binds_is_routed_to_its_own_handler() {
    let router = router();

    for operation in BOUND_OPERATIONS {
        let (status, body) = call(&router, operation, &json!({})).await;
        assert!(
            reached_an_operation(status, &body),
            "case {operation}: the model binds this operation, so the request must reach \
             its handler, got {status}: {body}"
        );
    }

    let (status, body) = call(&router, UNBOUND_OPERATION, &json!({})).await;
    assert!(
        !reached_an_operation(status, &body),
        "case {UNBOUND_OPERATION}: a name the model binds no operation for must be refused \
         by routing, got {status}: {body}"
    );
}

/// The same rule one level down: an operation AWS marks deprecated is served exactly as
/// it was before, so the six operations implemented in this crate still answer their
/// modelled output from the persisted record.
#[tokio::test]
async fn the_implemented_operations_answer_their_modeled_output() {
    let router = router();
    let case = "deprecated-but-served";
    let dataset = "deprecated-but-served";
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/LookoutRole");

    let created = ok_call(
        &router,
        "CreateDataset",
        &json!({ "DatasetName": dataset, "ClientToken": token(case) }),
    )
    .await;
    let dataset_arn = text(&created, "DatasetArn", case);
    assert!(
        is_dataset_arn(&dataset_arn, dataset),
        "case {case}: expected the dataset's own ARN, got {dataset_arn:?}"
    );

    let described = ok_call(&router, "DescribeDataset", &json!({ "DatasetName": dataset })).await;
    assert_eq!(
        described.get("DatasetArn").and_then(Value::as_str),
        Some(dataset_arn.as_str()),
        "case {case}: DescribeDataset must answer the stored ARN, got {described}"
    );

    let started = ok_call(
        &router,
        "StartDataIngestionJob",
        &json!({
            "DatasetName": dataset,
            "IngestionInputConfiguration": { "S3InputConfiguration": { "Bucket": "sensor-data" } },
            "RoleArn": role_arn,
            "ClientToken": token(&format!("{case}-ingest")),
        }),
    )
    .await;
    let job_id = text(&started, "JobId", case);

    let job = ok_call(
        &router,
        "DescribeDataIngestionJob",
        &json!({ "JobId": job_id }),
    )
    .await;
    assert_eq!(
        job.get("DatasetArn").and_then(Value::as_str),
        Some(dataset_arn.as_str()),
        "case {case}: the job must report its parent dataset's ARN, got {job}"
    );

    let listed = ok_call(&router, "ListDatasets", &json!({})).await;
    assert!(
        summary_names(&listed).contains(&dataset.to_string()),
        "case {case}: the created dataset must appear in the listing, got {listed}"
    );

    ok_call(&router, "DeleteDataset", &json!({ "DatasetName": dataset })).await;
    let (status, body) = call(&router, "DescribeDataset", &json!({ "DatasetName": dataset })).await;
    assert_eq!(
        status, 404,
        "case {case}: a deleted dataset must 404, got {status}: {body}"
    );
}
