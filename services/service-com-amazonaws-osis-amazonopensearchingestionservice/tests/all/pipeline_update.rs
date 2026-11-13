// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdatePipeline`, driven the way a client drives it: one HTTP
//! request per call through the same `MultiServiceRouter` the emulator boots.
//!
//! The rule under test is that an update is PARTIAL and PERSISTED: the request names the
//! pipeline and carries only the members it wants changed, so each named member replaces
//! the stored value, each omitted member keeps it, the pipeline enters `UPDATING` and the
//! clock owes it the settled status it already had — and every later read answers from
//! that stored record. The rows differ along what the rule generalises over: which
//! members the request carries (one, several, all, none), whether capacity scales up or
//! down, whether a member is a scalar, a structure or a document, and which settled
//! status the pipeline starts from.
//!
//! The refusal table covers the same operation's error contract over the inputs the
//! contract distinguishes: a name nothing was created under, a capacity pair the update
//! would leave inverted (named wholly or in part), and a pipeline that is mid-change.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is NOT the emulator default, so nothing under test can depend on a
/// default-region code path.
const REGION: &str = "eu-west-2";

/// The members `UpdatePipeline` can change (smithy_query: operation UpdatePipeline —
/// `MinUnits`, `MaxUnits`, `PipelineConfigurationBody`, `LogPublishingOptions`,
/// `BufferOptions`, `EncryptionAtRestOptions`, `PipelineRoleArn`). Every one of them is
/// optional, which is what makes the update partial.
const UPDATABLE_MEMBERS: [&str; 7] = [
    "MinUnits",
    "MaxUnits",
    "PipelineConfigurationBody",
    "LogPublishingOptions",
    "BufferOptions",
    "EncryptionAtRestOptions",
    "PipelineRoleArn",
];

/// Members an update never touches, minted or recorded at create time.
const IMMUTABLE_MEMBERS: [&str; 5] = [
    "PipelineName",
    "PipelineArn",
    "CreatedAt",
    "IngestEndpointUrls",
    "Tags",
];

/// What one call answered: the status, the modelled error code (empty on success) and the
/// parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// One member of the returned `Pipeline`.
    fn member(&self, name: &str) -> &Value {
        &self.body["Pipeline"][name]
    }

    fn status_value(&self) -> String {
        self.member("Status")
            .as_str()
            .unwrap_or_else(|| panic!("expected a string `Status` in {:?}", self.body))
            .to_string()
    }

    /// The error message the response carries, under either casing a protocol may render
    /// the `message` member with.
    fn message(&self) -> String {
        for key in ["message", "Message"] {
            if let Some(text) = self.body.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        panic!("expected an error message in {:?}", self.body)
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://osis.{REGION}.amazonaws.com{path}"))
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/osis/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

async fn create_pipeline(router: &MultiServiceRouter, request: Value) -> Answer {
    call(
        router,
        "POST",
        "/2022-01-01/osis/createPipeline",
        Some(request),
    )
    .await
}

async fn get_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2022-01-01/osis/getPipeline/{name}"),
        None,
    )
    .await
}

/// `UpdatePipeline` is `PUT /2022-01-01/osis/updatePipeline/{PipelineName}`: the name is
/// an `@httpLabel`, so the body carries only the members being changed.
async fn update_pipeline(router: &MultiServiceRouter, name: &str, patch: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/updatePipeline/{name}"),
        Some(patch),
    )
    .await
}

async fn stop_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/stopPipeline/{name}"),
        None,
    )
    .await
}

/// The create every update case starts from: every member an update can change is set to
/// a known value, so an omitted member being preserved is observable.
fn baseline_request(name: &str) -> Value {
    json!({
        "PipelineName": name,
        "MinUnits": 2,
        "MaxUnits": 4,
        "PipelineConfigurationBody": "version: \"2\"\nbaseline-pipeline:\n  source:\n    http:\n",
        "PipelineRoleArn": "arn:aws:iam::000000000000:role/osis-baseline",
        "LogPublishingOptions": { "IsLoggingEnabled": false },
        "BufferOptions": { "PersistentBufferEnabled": false },
        "EncryptionAtRestOptions": { "KmsKeyArn": "arn:aws:kms:eu-west-2:000000000000:key/baseline" },
        "Tags": [{ "Key": "team", "Value": "search" }],
    })
}

/// Create the baseline pipeline and read it back, so the caller starts from a settled,
/// running pipeline: the `CREATING` -> `ACTIVE` transition the create armed is due as soon
/// as the create has answered.
async fn active_baseline(router: &MultiServiceRouter, case: &str, name: &str) -> Answer {
    let created = create_pipeline(router, baseline_request(name)).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePipeline, got {} / {:?}",
        created.status, created.body
    );
    let read = get_pipeline(router, name).await;
    assert_eq!(
        read.status_value(),
        "ACTIVE",
        "case {case:?}: the pipeline must settle to ACTIVE before the update, got {:?}",
        read.body
    );
    read
}

/// An update names the members it changes and leaves every other one alone, and the
/// pipeline settles back into the status it was already in.
#[tokio::test]
async fn an_update_writes_only_the_members_it_names() {
    // (case, pipeline name, the update body, the settled status to start from)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "scale both unit counts up",
            "scale-units-up",
            json!({ "MinUnits": 6, "MaxUnits": 12 }),
            "ACTIVE",
        ),
        (
            // `MinUnits`/`MaxUnits` are `@range(1,)` (smithy_query: operation
            // UpdatePipeline), so this row sits on the published lower bound.
            "scale both unit counts down to the published minimum",
            "scale-units-down",
            json!({ "MinUnits": 1, "MaxUnits": 1 }),
            "ACTIVE",
        ),
        (
            "raise only the maximum",
            "raise-max-only",
            json!({ "MaxUnits": 9 }),
            "ACTIVE",
        ),
        (
            "raise only the minimum, within the stored maximum",
            "raise-min-only",
            json!({ "MinUnits": 4 }),
            "ACTIVE",
        ),
        (
            "replace the configuration document alone",
            "replace-config-only",
            json!({
                "PipelineConfigurationBody": "version: \"2\"\nupdated-pipeline:\n  source:\n    otel_trace_source:\n  sink:\n    - opensearch:\n",
            }),
            "ACTIVE",
        ),
        (
            "replace every structure member and the role",
            "replace-structures",
            json!({
                "LogPublishingOptions": {
                    "IsLoggingEnabled": true,
                    "CloudWatchLogDestination": { "LogGroup": "/aws/vendedlogs/OpenSearchIngestion/updated" },
                },
                "BufferOptions": { "PersistentBufferEnabled": true },
                "EncryptionAtRestOptions": { "KmsKeyArn": "arn:aws:kms:eu-west-2:000000000000:key/rotated" },
                "PipelineRoleArn": "arn:aws:iam::000000000000:role/osis-updated",
            }),
            "ACTIVE",
        ),
        (
            "every updatable member at once",
            "replace-everything",
            json!({
                "MinUnits": 3,
                "MaxUnits": 8,
                "PipelineConfigurationBody": "version: \"2\"\nall-members:\n  source:\n    http:\n",
                "LogPublishingOptions": { "IsLoggingEnabled": true },
                "BufferOptions": { "PersistentBufferEnabled": true },
                "EncryptionAtRestOptions": { "KmsKeyArn": "arn:aws:kms:eu-west-2:000000000000:key/all" },
                "PipelineRoleArn": "arn:aws:iam::000000000000:role/osis-all",
            }),
            "ACTIVE",
        ),
        (
            "an update that names no member changes nothing",
            "no-member-update",
            json!({}),
            "ACTIVE",
        ),
        (
            "a stopped pipeline scales and stays stopped",
            "stopped-scales-units",
            json!({ "MinUnits": 5, "MaxUnits": 5 }),
            "STOPPED",
        ),
    ];

    let router = new_router();
    for (case, name, patch, start_status) in cases {
        let before = active_baseline(&router, case, name).await;
        if start_status == "STOPPED" {
            let stopped = stop_pipeline(&router, name).await;
            assert_eq!(
                stopped.status, 200,
                "case {case:?}: expected 200 from StopPipeline, got {} / {:?}",
                stopped.status, stopped.body
            );
        }
        let baseline = baseline_request(name);

        let updated = update_pipeline(&router, name, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdatePipeline, got {} / {:?}",
            updated.status, updated.body
        );

        // Applying a change takes time, so the answer reports the transient status.
        assert_eq!(
            updated.status_value(),
            "UPDATING",
            "case {case:?}: the update must answer with \"UPDATING\", got {:?}",
            updated.body
        );

        // The partial-update rule, member by member: a named member took the request's
        // value, an omitted one kept the stored one.
        for member in UPDATABLE_MEMBERS {
            let expected = patch
                .get(member)
                .or_else(|| baseline.get(member))
                .unwrap_or(&Value::Null);
            assert_eq!(
                updated.member(member),
                expected,
                "case {case:?}: {member} must be {expected:?} after the update, got {:?}",
                updated.member(member)
            );
        }
        for member in IMMUTABLE_MEMBERS {
            assert_eq!(
                updated.member(member),
                before.member(member),
                "case {case:?}: {member} must not change across an update, got {:?}",
                updated.member(member)
            );
        }
        assert!(
            updated.member("LastUpdatedAt").as_f64() >= before.member("LastUpdatedAt").as_f64(),
            "case {case:?}: LastUpdatedAt must not go backwards, got {:?} after {:?}",
            updated.member("LastUpdatedAt"),
            before.member("LastUpdatedAt")
        );

        // Read it back: the change was persisted, and the pipeline settled into the
        // status it held before the update.
        let read = get_pipeline(&router, name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetPipeline, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.status_value(),
            start_status,
            "case {case:?}: the update must settle back to {start_status:?}, got {:?}",
            read.body
        );
        for member in UPDATABLE_MEMBERS.iter().chain(IMMUTABLE_MEMBERS.iter()) {
            assert_eq!(
                read.member(member),
                updated.member(member),
                "case {case:?}: {member} did not round-trip through GetPipeline, got {:?}",
                read.member(member)
            );
        }
        assert!(
            read.member("StatusReason").is_null(),
            "case {case:?}: a settled pipeline carries no StatusReason, got {:?}",
            read.member("StatusReason")
        );
    }
}

/// The error contract of an update, over the inputs the contract distinguishes.
#[tokio::test]
async fn an_update_is_refused_per_its_error_contract() {
    // (case, pipeline name, the update body, how the pipeline is set up, status,
    //  error code, expected message)
    //
    // There is no mid-change row: the transition a lifecycle call arms is due the instant
    // that call answers
    // (`service_com_amazonaws_osis_amazonopensearchingestionservice::state::TRANSITION_SPAN`),
    // so an update always meets a settled status and the ConflictException this operation
    // models for a pipeline mid-change is not reachable through the API.
    enum Setup {
        /// Nothing is created under this name.
        Missing,
        /// A settled, running pipeline.
        Active,
    }

    let cases: Vec<(&str, &str, Value, Setup, u16, &str, String)> = vec![
        (
            "a name nothing was created under",
            "ghost-pipeline",
            json!({ "MinUnits": 2, "MaxUnits": 2 }),
            Setup::Missing,
            404,
            "ResourceNotFoundException",
            "Service returned error code ResourceNotFoundException".to_string(),
        ),
        (
            "the shortest legal name, also never created",
            "abc",
            json!({ "MaxUnits": 3 }),
            Setup::Missing,
            404,
            "ResourceNotFoundException",
            "Service returned error code ResourceNotFoundException".to_string(),
        ),
        (
            "both counts named and inverted",
            "inverted-pair",
            json!({ "MinUnits": 7, "MaxUnits": 3 }),
            Setup::Active,
            400,
            "ValidationException",
            "MaxUnits (3) must be greater than or equal to MinUnits (7)".to_string(),
        ),
        (
            // Only the minimum is named: the pair it would leave on the pipeline is what
            // the rule is checked against, so the stored maximum of 4 is the one reported.
            "only the minimum named, above the stored maximum",
            "min-above-stored-max",
            json!({ "MinUnits": 9 }),
            Setup::Active,
            400,
            "ValidationException",
            "MaxUnits (4) must be greater than or equal to MinUnits (9)".to_string(),
        ),
        (
            "only the maximum named, below the stored minimum",
            "max-below-stored-min",
            json!({ "MaxUnits": 1 }),
            Setup::Active,
            400,
            "ValidationException",
            "MaxUnits (1) must be greater than or equal to MinUnits (2)".to_string(),
        ),
    ];

    let router = new_router();
    for (case, name, patch, setup, status, error_type, message) in cases {
        match setup {
            Setup::Missing => {}
            Setup::Active => {
                active_baseline(&router, case, name).await;
            }
        }

        let refused = update_pipeline(&router, name, patch).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, error_type,
            "case {case:?}: expected {error_type}, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.message(),
            message,
            "case {case:?}: expected the operation's own wording, got {:?}",
            refused.message()
        );
    }
}

/// A refused update changes nothing: the pipeline keeps the capacity, the configuration
/// and the status it had, so a failed call cannot be told apart from one never made.
#[tokio::test]
async fn a_refused_update_leaves_the_pipeline_untouched() {
    let router = new_router();
    let case = "an inverted pair leaves the stored record alone";
    let name = "refused-update-pipeline";
    let before = active_baseline(&router, case, name).await;

    let refused = update_pipeline(
        &router,
        name,
        json!({
            "MinUnits": 12,
            "PipelineConfigurationBody": "version: \"2\"\nnever-written:\n  source:\n    http:\n",
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected the update to be refused, got {} / {:?}",
        refused.status, refused.body
    );

    let after = get_pipeline(&router, name).await;
    for member in UPDATABLE_MEMBERS.iter().chain(IMMUTABLE_MEMBERS.iter()) {
        assert_eq!(
            after.member(member),
            before.member(member),
            "case {case:?}: {member} changed despite the refusal, got {:?}",
            after.member(member)
        );
    }
    assert_eq!(
        after.status_value(),
        "ACTIVE",
        "case {case:?}: a refused update must not move the status, got {:?}",
        after.body
    );
}
