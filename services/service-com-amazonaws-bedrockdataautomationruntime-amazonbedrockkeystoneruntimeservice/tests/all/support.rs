// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! A submission names WHAT to extract by reference, and the referenced data-automation
//! project belongs to the build-time service rather than to this one, so a suite whose
//! submissions name a project has to ARRANGE that project in the owning service's store
//! before the submission can be accepted.
//!
//! [`seed_project`] is that arrangement, for a suite that is about something else and
//! names its project by a FIXED ARN (the ARN a create mints is the store's to choose, so
//! a suite that wants a particular one writes the record). The suite that is about the
//! resolution rule itself (`control_plane_reference_resolution`) does not use this: it
//! creates every project and blueprint it references through the build-time service's own
//! `CreateDataAutomationProject` / `CreateBlueprint` over the wire.
//!
//! The second half of this module is the WIRE harness — [`Answer`], [`call`] and the
//! tag-key fixture around them — which the suites about the modelled `TagKey` constraint
//! share instead of each carrying its own copy of the dispatch, the credential scope and
//! the submission body.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state as build_time;
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

/// The `invocationId` inside a minted invocation ARN: the segment after the resource
/// prefix the ARN is built around (`state::invocation_arn`).
///
/// A job's output lives under a segment of its OWN id below the submitted output prefix,
/// so the id is what a reader's reported output location is checked against
/// ([`job_metadata_uri`]).
pub fn invocation_id(invocation_arn: &str) -> &str {
    invocation_arn.split('/').next_back().unwrap_or_default()
}

/// Where `GetDataAutomationStatus` reports the output of the job `invocation_arn` names,
/// for a submission whose `outputConfiguration.s3Uri` was `output_prefix`: that job's own
/// `job_metadata.json`, under its id below the prefix.
///
/// For the suites whose subject is something else and which only have to know that the
/// reader answers for the record their submission minted; the rule itself — including how
/// a prefix with and without a trailing separator are joined — is asserted row by row in
/// `job_output_location_and_outcome`.
pub fn job_metadata_uri(output_prefix: &str, invocation_arn: &str) -> String {
    format!(
        "{}/{}/job_metadata.json",
        output_prefix.trim_end_matches('/'),
        invocation_id(invocation_arn)
    )
}

/// Register one data-automation project, under `project_arn` and named `project_name`, in
/// the build-time service's store on `engine` — a project in its settled stage, as a
/// create that has already completed leaves it.
pub fn seed_project(engine: &engine::InMemoryEngine, project_arn: &str, project_name: &str) {
    let state = build_time::service_state(engine).expect("the build-time store on this engine");
    let created = build_time::now();
    let mut store = state.lock();
    store.projects.insert(
        project_arn.to_string(),
        build_time::DataAutomationProjectRecord {
            project_arn: project_arn.to_string(),
            project_name: project_name.to_string(),
            project_stage: build_time::DEFAULT_PROJECT_STAGE.to_string(),
            project_type: "ASYNC".to_string(),
            project_description: None,
            standard_output_configuration: None,
            custom_output_configuration: None,
            override_configuration: None,
            data_automation_library_configuration: None,
            creation_time: created,
            last_modified_time: created,
            kms_key_id: None,
            kms_encryption_context: None,
            completes_at: SystemTime::now(),
            tags: Vec::new(),
        },
    );
    store
        .project_names
        .insert(project_name.to_string(), project_arn.to_string());
}

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes on.
pub const SIGNING_NAME: &str = "bedrock";

/// The region the requests [`call`] builds sign for.
pub const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
pub const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// A legal `dataAutomationConfiguration.dataAutomationProjectArn` (pattern from
/// `smithy_query` shape `DataAutomationProjectArn`): what tells the service WHAT to
/// extract, which every accepted submission has to name. [`tag_key_router`] arranges it in
/// the store of the service that owns it.
pub const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/receipts";

/// The modelled `TagKey` pattern without the outer anchors a Smithy `@pattern` does not
/// need (`smithy_query` shape `TagKey`), as a refusal's requirement clause prints it.
pub const UNANCHORED_TAG_KEY_PATTERN: &str = r"(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*";

/// One wire answer: the HTTP status and the parsed JSON body.
pub struct Answer {
    /// The HTTP status the router answered with.
    pub status: u16,
    /// The response body, parsed, or `Value::Null` for an empty one.
    pub body: Value,
}

impl Answer {
    /// The awsJson error code: the body's `__type`, local name only.
    pub fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('#')
            .next_back()
            .unwrap_or_default()
    }

    /// The error message the body carries, or the empty string.
    pub fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Call one operation on `router` the way an SDK would: the awsJson1_1 target header, the
/// JSON body and a credential scope in this service's signing name.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// A router serving only this service over one shared engine, with [`PROJECT_ARN`]
/// arranged under `project_name` in the store of the service that OWNS data-automation
/// projects: a submission naming a project nobody created is refused before any tag rule
/// can be reached.
pub fn tag_key_router(project_name: &str) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_project(&engine, PROJECT_ARN, project_name);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(engine));
    router
}

/// A submission carrying every required member at its minimum, writing its output below
/// `output`, optionally with `tags`.
pub fn tag_key_submission(output: &str, tags: Option<Value>) -> Value {
    let mut body = json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/receipt.pdf"},
        "outputConfiguration": {"s3Uri": output},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
    });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    body
}

/// The tag keys the resource `arn` currently carries, read back through
/// `ListTagsForResource` and sorted.
pub async fn tag_keys(router: &MultiServiceRouter, arn: &str) -> Vec<String> {
    let answer = call(router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        answer.status, 200,
        "the resource's tags must read back, got {} / {}",
        answer.status, answer.body
    );
    let mut keys: Vec<String> = answer.body["tags"]
        .as_array()
        .expect("the answer must carry a tags list")
        .iter()
        .map(|tag| {
            tag["key"]
                .as_str()
                .expect("every reported tag has a key")
                .to_string()
        })
        .collect();
    keys.sort();
    keys
}

/// The refusal message a tag key outside the modelled pattern earns at input path `path`:
/// the `ValidationException` envelope around one clause naming the member and the
/// constraint it failed, with the offending value NOT echoed.
pub fn tag_key_refusal_message(path: &str) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {UNANCHORED_TAG_KEY_PATTERN}"
    )
}
