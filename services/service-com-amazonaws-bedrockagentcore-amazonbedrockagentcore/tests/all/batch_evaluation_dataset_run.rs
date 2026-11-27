// SPDX-License-Identifier: CC0-1.0

//! The DATASET RUN a batch evaluation carries out before it evaluates anything: what
//! `StartBatchEvaluation`'s `datasetExecutionConfig` settles, and what
//! `GetBatchEvaluation` reports for it.
//!
//! THE RULE: a start names the run loosely — a dataset whose version may be left open,
//! and an endpoint without the agent runtime version behind it — while a read of the job
//! reports `datasetExecutionDetails`, where both are settled. So the configuration is
//! RESOLVED once, by the call that starts the job, against the component that owns each
//! resource, and those resolved details are what every later read reports: the dataset
//! version that runs (the one the request named, else the latest version published when
//! the job started), the agent runtime version the endpoint pointed at, and the request
//! template each turn is sent under (the one the request named, else the documented
//! default `{"prompt": "{{turn.input}}"}`). A dataset, a version or an endpoint this
//! account cannot run is refused 400 `ValidationException` naming the member at fault,
//! and no job is started for it.
//!
//! The rows span what the rule generalises over: whether the request names the dataset
//! version or leaves it open, whether it names a request template, how many scenarios the
//! version that runs holds, which resource of the pair the request got wrong and in which
//! way (not held, not published, not runnable, not ready), and which companion member the
//! model documents a dataset run cannot be asked for without.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with — the dataset and the agent runtime through the AgentCore CONTROL plane's own
//! API, which is what owns them — so what is asserted is the body a client parses.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;
use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as control;

/// The dataset schema whose scenarios a batch evaluation can run: "The dataset must use
/// the predefined evaluation schema" (source: smithy_query query="shape"
/// shape="DatasetExecutionConfig").
const PREDEFINED_SCHEMA: &str = "AGENTCORE_EVALUATION_PREDEFINED_V1";

/// A dataset schema a batch evaluation cannot run the scenarios of, from the same enum
/// (source: smithy_query query="shape" shape="DatasetSchemaType" against the control
/// plane's model).
const THIRD_PARTY_SCHEMA: &str = "THIRD_PARTY_EVALUATION_V1";

/// The request template a turn is sent under when the request names none: "If you don't
/// specify a template, the service uses `{\"prompt\": \"{{turn.input}}\"}`" (source:
/// smithy_query query="doc" shape="AgentCoreRuntimeInvocation" member="requestTemplate").
const DEFAULT_REQUEST_TEMPLATE: &str = "{\"prompt\": \"{{turn.input}}\"}";

/// A request template a caller supplies instead.
const NAMED_REQUEST_TEMPLATE: &str = "{\"message\": \"{{turn.input}}\", \"stream\": false}";

/// The endpoint the control plane establishes with every agent runtime, which is the one
/// these fixtures address.
const DEFAULT_ENDPOINT: &str = "DEFAULT";

/// A role of ANOTHER account, so the create of the fixture runtime needs no IAM role of
/// this emulator's own: another account's IAM is not this emulator's to ask.
const FOREIGN_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/agentcore-batch-eval";

/// An agent runtime endpoint ARN, well formed, for a runtime nothing created.
const ENDPOINT_ARN_OF_NO_RUNTIME: &str = "arn:aws:bedrock-agentcore:us-east-1:000000000000:\
     runtime/ghost-1a2b3c4d5e/runtime-endpoint/DEFAULT";

/// The modelled error code every refusal here answers with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `ValidationExceptionReason` a member-level failure carries (source: smithy_query
/// query="shape" shape="ValidationExceptionReason").
const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The emulator, with request authentication off so a test signs nothing, plus the shared
/// engine: the AgentCore CONTROL plane is mounted beside this service because it owns the
/// datasets and the agent runtimes a dataset run names.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(control::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body).
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

/// One member of a JSON object as a string, or `""`.
fn text(body: &Value, path: &[&str]) -> String {
    let mut at = body;
    for name in path {
        at = at.get(name).unwrap_or(&Value::Null);
    }
    at.as_str().unwrap_or_default().to_string()
}

/// The wire error code of a refusal (`__type` on `restJson1`).
fn error_code(body: &Value) -> String {
    text(body, &["__type"])
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Make every transition the CONTROL plane's clock owes a dataset or an agent runtime
/// endpoint already due, so this suite waits for no span it is not testing. The test
/// reaching into the store the handlers share, not an emulator behaviour.
fn settle_control_plane(engine: &engine::InMemoryEngine) {
    let already_due = SystemTime::now() - Duration::from_secs(3_600);
    let state = control::state::service_state(engine).expect("the control plane's state");
    let mut store = state.lock();
    for dataset in store.datasets.values_mut() {
        dataset.settles_at = already_due;
    }
    for runtime in store.agent_runtimes.values_mut() {
        for endpoint in runtime.endpoints.values_mut() {
            endpoint.settles_at = already_due;
        }
    }
}

/// Hold the transition of the endpoints of `agent_runtime_id` open, so the endpoint is
/// deterministically still on its way to `READY` however fast the host runs. The test
/// reaching into the shared store again, for the same reason.
fn hold_endpoint_transition(engine: &engine::InMemoryEngine, agent_runtime_id: &str) {
    let not_yet_due = SystemTime::now() + Duration::from_secs(3_600);
    let state = control::state::service_state(engine).expect("the control plane's state");
    let mut store = state.lock();
    let runtime = store
        .agent_runtimes
        .get_mut(agent_runtime_id)
        .expect("the runtime that was just created");
    for endpoint in runtime.endpoints.values_mut() {
        endpoint.settles_at = not_yet_due;
    }
}

/// How many batch evaluation jobs this emulator holds — the check that a refused start
/// wrote nothing.
fn batch_evaluation_count(engine: &engine::InMemoryEngine) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store.batch_evaluations.len()
}

/// `CreateDataset` on the CONTROL plane: `POST /datasets`, with `scenarios` inline
/// examples. Every example of an AgentCore evaluation dataset carries a `scenario_id`.
async fn create_dataset(
    router: &MultiServiceRouter,
    name: &str,
    schema_type: &str,
    scenarios: usize,
) -> String {
    let examples: Vec<Value> = (0..scenarios)
        .map(|at| json!({"scenario_id": format!("scenario-{at}"), "input": "hello"}))
        .collect();
    let (status, body) = send(
        router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": name,
            "schemaType": schema_type,
            "source": {"inlineExamples": {"examples": examples}},
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: CreateDataset {name} must be accepted, got {status} with {body}"
    );
    text(&body, &["datasetId"])
}

/// `CreateDatasetVersion`: `POST /datasets/{datasetId}/versions`, publishing the working
/// copy as the next numbered version.
async fn publish_version(router: &MultiServiceRouter, dataset_id: &str) -> String {
    let (status, body) = send(
        router,
        "POST",
        &format!("/datasets/{dataset_id}/versions"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: CreateDatasetVersion of {dataset_id} must be accepted, got {status} with {body}"
    );
    text(&body, &["datasetVersion"])
}

/// Empty the DRAFT working copy of `dataset_id`, so the version published next holds no
/// scenario at all.
async fn empty_the_draft(router: &MultiServiceRouter, dataset_id: &str) {
    let (status, body) = send(
        router,
        "GET",
        &format!("/datasets/{dataset_id}/examples?datasetVersion=DRAFT"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: ListDatasetExamples of {dataset_id} must be served, got {status} with {body}"
    );
    let example_ids: Vec<Value> = body
        .get("examples")
        .and_then(Value::as_array)
        .map(|examples| {
            examples
                .iter()
                .map(|example| example.get("exampleId").cloned().unwrap_or(Value::Null))
                .collect()
        })
        .unwrap_or_default();
    let (status, body) = send(
        router,
        "POST",
        &format!("/datasets/{dataset_id}/examples/delete"),
        Some(json!({"exampleIds": example_ids})),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: DeleteDatasetExamples of {dataset_id} must be accepted, got {status} with {body}"
    );
}

/// `CreateAgentRuntime`: `PUT /runtimes/`, which establishes the `DEFAULT` endpoint with
/// it. Answers the `agentRuntimeId`.
async fn create_agent_runtime(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = send(
        router,
        "PUT",
        "/runtimes/",
        Some(json!({
            "agentRuntimeName": name,
            "agentRuntimeArtifact": {"containerConfiguration": {
                "containerUri": "public.ecr.aws/agentcore/demo:v1"
            }},
            "networkConfiguration": {"networkMode": "PUBLIC"},
            "roleArn": FOREIGN_ROLE_ARN,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: CreateAgentRuntime {name} must be accepted, got {status} with {body}"
    );
    text(&body, &["agentRuntimeId"])
}

/// `GetAgentRuntimeEndpoint`: the endpoint's own ARN, the version it points at and the
/// status it reports.
async fn read_endpoint(router: &MultiServiceRouter, agent_runtime_id: &str) -> (String, String, String) {
    let (status, body) = send(
        router,
        "GET",
        &format!("/runtimes/{agent_runtime_id}/runtime-endpoints/{DEFAULT_ENDPOINT}/"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: GetAgentRuntimeEndpoint of {agent_runtime_id} must be served, got {status} \
         with {body}"
    );
    (
        text(&body, &["agentRuntimeEndpointArn"]),
        text(&body, &["targetVersion"]),
        text(&body, &["status"]),
    )
}

/// `StartBatchEvaluation`: `POST /evaluations/batch-evaluate`.
async fn start_batch_evaluation(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    send(router, "POST", "/evaluations/batch-evaluate", Some(body)).await
}

/// `GetBatchEvaluation`: `GET /evaluations/batch-evaluate/{batchEvaluationId}`.
async fn get_batch_evaluation(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/evaluations/batch-evaluate/{id}"),
        None,
    )
    .await
}

/// A `cloudWatchLogs` data source, which is the one a dataset run must be accompanied by.
fn cloudwatch_source() -> Value {
    json!({"cloudWatchLogs": {
        "serviceNames": ["checkout-agent"],
        "logGroupNames": ["/aws/bedrock-agentcore/checkout"],
    }})
}

/// A `datasetExecutionConfig` naming `dataset_id`, optionally a version and optionally a
/// request template.
fn dataset_execution_config(
    dataset_id: &str,
    dataset_version: Option<&str>,
    endpoint_arn: &str,
    request_template: Option<&str>,
) -> Value {
    let mut dataset = json!({"datasetId": dataset_id});
    if let Some(version) = dataset_version {
        dataset["datasetVersion"] = json!(version);
    }
    let mut invocation = json!({"agentRuntimeEndpointArn": endpoint_arn});
    if let Some(template) = request_template {
        invocation["requestTemplate"] = json!(template);
    }
    json!({
        "dataset": dataset,
        "agentInvocation": {"agentCoreRuntime": invocation},
    })
}

/// A `StartBatchEvaluation` body that asks for the dataset run `run`, carrying everything
/// the model documents must accompany one.
fn start_with_run(name: &str, run: Value) -> Value {
    json!({
        "batchEvaluationName": name,
        "dataSourceConfig": cloudwatch_source(),
        "evaluators": [{"evaluatorId": "Builtin.Correctness"}],
        "datasetExecutionConfig": run,
    })
}

/// The resources every case here runs against, created through the CONTROL plane's own API
/// and with the transitions its clock owes them already due.
struct Fixtures {
    /// Three scenarios, with two published versions, so "the latest published version"
    /// is not also the only one.
    predefined: String,
    /// One scenario — the fewest a version that runs may hold.
    single_scenario: String,
    /// A published version holding no scenario at all, one past that bound.
    no_scenario: String,
    /// A dataset whose schema is not the predefined evaluation schema.
    third_party: String,
    /// The endpoint a run is sent to, and the agent runtime version it points at.
    endpoint_arn: String,
    endpoint_version: String,
    /// An endpoint whose transition to `READY` the clock still owes.
    pending_endpoint_arn: String,
}

async fn fixtures(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Fixtures {
    let predefined = create_dataset(router, "predefined_run", PREDEFINED_SCHEMA, 3).await;
    let single_scenario = create_dataset(router, "single_scenario", PREDEFINED_SCHEMA, 1).await;
    let no_scenario = create_dataset(router, "no_scenario", PREDEFINED_SCHEMA, 1).await;
    let third_party = create_dataset(router, "third_party", THIRD_PARTY_SCHEMA, 2).await;
    let runtime = create_agent_runtime(router, "batch_eval_agent").await;
    let pending_runtime = create_agent_runtime(router, "pending_agent").await;

    // Every create is asynchronous; the spans they owe are not what this suite is about.
    settle_control_plane(engine);

    assert_eq!(
        publish_version(router, &predefined).await,
        "1",
        "setup: the first published version of a dataset is version 1"
    );
    settle_control_plane(engine);
    assert_eq!(
        publish_version(router, &predefined).await,
        "2",
        "setup: the second published version of a dataset is version 2"
    );
    publish_version(router, &single_scenario).await;
    empty_the_draft(router, &no_scenario).await;
    settle_control_plane(engine);
    publish_version(router, &no_scenario).await;
    publish_version(router, &third_party).await;
    settle_control_plane(engine);

    let (endpoint_arn, endpoint_version, status) = read_endpoint(router, &runtime).await;
    assert_eq!(
        status, "READY",
        "setup: the fixture endpoint must be ready, got {status:?}"
    );
    hold_endpoint_transition(engine, &pending_runtime);
    let (pending_endpoint_arn, _, pending_status) = read_endpoint(router, &pending_runtime).await;
    assert_ne!(
        pending_status, "READY",
        "setup: the pending endpoint must not be ready yet, got {pending_status:?}"
    );

    Fixtures {
        predefined,
        single_scenario,
        no_scenario,
        third_party,
        endpoint_arn,
        endpoint_version,
        pending_endpoint_arn,
    }
}

/// One row of the accepted table: the run a start asks for, and what the job must report
/// for it.
struct AcceptedRow<'a> {
    case: &'a str,
    /// The dataset the run names.
    dataset_id: &'a str,
    /// The version the run names, or `None` to leave it open.
    version: Option<&'a str>,
    /// The request template the run names, or `None` to take the documented default.
    template: Option<&'a str>,
    /// The version that must run.
    expected_version: &'a str,
    /// The template each turn must be sent under.
    expected_template: &'a str,
}

/// The settled run a read of the job reports, member for member.
#[tokio::test]
async fn a_dataset_run_reads_back_settled_on_the_job() {
    let (router, engine) = emulator();
    let held = fixtures(&router, &engine).await;

    let rows: Vec<AcceptedRow<'_>> = vec![
        AcceptedRow {
            case: "no-version-named-runs-the-latest-published-version",
            dataset_id: held.predefined.as_str(),
            version: None,
            template: None,
            expected_version: "2",
            expected_template: DEFAULT_REQUEST_TEMPLATE,
        },
        AcceptedRow {
            case: "a-named-version-runs-that-version",
            dataset_id: held.predefined.as_str(),
            version: Some("1"),
            template: None,
            expected_version: "1",
            expected_template: DEFAULT_REQUEST_TEMPLATE,
        },
        AcceptedRow {
            case: "a-named-template-is-what-each-turn-is-sent-under",
            dataset_id: held.predefined.as_str(),
            version: Some("2"),
            template: Some(NAMED_REQUEST_TEMPLATE),
            expected_version: "2",
            expected_template: NAMED_REQUEST_TEMPLATE,
        },
        AcceptedRow {
            case: "one-scenario-is-inside-the-range-a-version-may-run",
            dataset_id: held.single_scenario.as_str(),
            version: None,
            template: None,
            expected_version: "1",
            expected_template: DEFAULT_REQUEST_TEMPLATE,
        },
    ];

    for (
        at,
        AcceptedRow {
            case,
            dataset_id,
            version,
            template,
            expected_version,
            expected_template,
        },
    ) in rows.into_iter().enumerate()
    {
        let run = dataset_execution_config(dataset_id, version, &held.endpoint_arn, template);
        let (status, started) =
            start_batch_evaluation(&router, start_with_run(&format!("run_{at}"), run)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the start must be accepted, got {status} with {started}"
        );
        let job = text(&started, &["batchEvaluationId"]);

        let (status, read) = get_batch_evaluation(&router, &job).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the job must read back, got {status} with {read}"
        );
        let details = read
            .get("datasetExecutionDetails")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            text(&details, &["dataset", "datasetId"]),
            dataset_id,
            "case {case:?}: expected the dataset the run named, got {details}"
        );
        assert_eq!(
            text(&details, &["dataset", "datasetVersion"]),
            expected_version,
            "case {case:?}: expected version {expected_version}, got {details}"
        );
        assert_eq!(
            text(&details, &["agentInvocation", "agentCoreRuntime", "agentRuntimeEndpointArn"]),
            held.endpoint_arn,
            "case {case:?}: expected the endpoint the run named, got {details}"
        );
        assert_eq!(
            text(&details, &["agentInvocation", "agentCoreRuntime", "agentRuntimeVersion"]),
            held.endpoint_version,
            "case {case:?}: expected the version the endpoint points at, got {details}"
        );
        assert_eq!(
            text(&details, &["agentInvocation", "agentCoreRuntime", "requestTemplate"]),
            expected_template,
            "case {case:?}: expected the template each turn is sent under, got {details}"
        );
    }

    // A start that asks for NO dataset run has none to report: the member is absent
    // rather than carrying a run nothing was asked for.
    let (status, started) = start_batch_evaluation(
        &router,
        json!({
            "batchEvaluationName": "no_dataset_run",
            "dataSourceConfig": cloudwatch_source(),
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "a start without a dataset run must be accepted, got {status} with {started}"
    );
    let (status, read) = get_batch_evaluation(&router, &text(&started, &["batchEvaluationId"])).await;
    assert_eq!(
        status, 200,
        "the job must read back, got {status} with {read}"
    );
    assert!(
        read.get("datasetExecutionDetails").is_none(),
        "a job started with no dataset run must report none, got {read}"
    );
}

/// A dataset, a version or an endpoint this account cannot run is refused over the member
/// at fault, and no job is started for it.
#[tokio::test]
async fn a_dataset_run_this_account_cannot_run_is_refused() {
    let (router, engine) = emulator();
    let held = fixtures(&router, &engine).await;
    let before = batch_evaluation_count(&engine);

    // (case, the dataset the run names, the version it names, the endpoint it names,
    //  the member path the refusal must name)
    let rows: Vec<(&str, &str, Option<&str>, &str, &str)> = vec![
        (
            "a dataset this account does not hold",
            "absent_dataset",
            None,
            held.endpoint_arn.as_str(),
            "datasetExecutionConfig.dataset.datasetId",
        ),
        (
            "a dataset whose schema is not the predefined evaluation schema",
            held.third_party.as_str(),
            None,
            held.endpoint_arn.as_str(),
            "datasetExecutionConfig.dataset.datasetId",
        ),
        (
            "the DRAFT working copy, which cannot be run",
            held.predefined.as_str(),
            Some("DRAFT"),
            held.endpoint_arn.as_str(),
            "datasetExecutionConfig.dataset.datasetVersion",
        ),
        (
            "a version the dataset never published",
            held.predefined.as_str(),
            Some("7"),
            held.endpoint_arn.as_str(),
            "datasetExecutionConfig.dataset.datasetVersion",
        ),
        (
            "a version holding no scenario at all",
            held.no_scenario.as_str(),
            None,
            held.endpoint_arn.as_str(),
            "datasetExecutionConfig.dataset.datasetVersion",
        ),
        (
            "an endpoint this account does not hold",
            held.predefined.as_str(),
            None,
            ENDPOINT_ARN_OF_NO_RUNTIME,
            "datasetExecutionConfig.agentInvocation.agentCoreRuntime.agentRuntimeEndpointArn",
        ),
        (
            "an endpoint that is not ready yet",
            held.predefined.as_str(),
            None,
            held.pending_endpoint_arn.as_str(),
            "datasetExecutionConfig.agentInvocation.agentCoreRuntime.agentRuntimeEndpointArn",
        ),
    ];

    for (at, (case, dataset_id, version, endpoint_arn, member)) in rows.into_iter().enumerate() {
        let run = dataset_execution_config(dataset_id, version, endpoint_arn, None);
        let (status, refused) =
            start_batch_evaluation(&router, start_with_run(&format!("refused_{at}"), run)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {refused}"
        );
        assert_eq!(
            text(&refused, &["reason"]),
            FIELD_VALIDATION_FAILED,
            "case {case:?}: expected the member-level classification, got {refused}"
        );
        let named: Vec<String> = refused
            .get("fieldList")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| text(field, &["name"]))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            named,
            vec![member.to_string()],
            "case {case:?}: expected the refusal to name {member}, got {refused}"
        );
        assert!(
            !text(&refused, &["message"]).is_empty(),
            "case {case:?}: the refusal must say what is wrong, got {refused}"
        );
    }

    assert_eq!(
        batch_evaluation_count(&engine),
        before,
        "a refused start must leave no job behind"
    );
}

/// A dataset run cannot be asked for without what the model documents must accompany it,
/// and each refusal names the companion member at fault.
#[tokio::test]
async fn a_dataset_run_is_refused_without_the_members_it_must_accompany() {
    let (router, engine) = emulator();
    let held = fixtures(&router, &engine).await;
    let before = batch_evaluation_count(&engine);
    let run = dataset_execution_config(&held.predefined, None, &held.endpoint_arn, None);

    // (case, the start body, the member path the refusal must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "no evaluators",
            json!({
                "batchEvaluationName": "no_evaluators",
                "dataSourceConfig": cloudwatch_source(),
                "datasetExecutionConfig": run.clone(),
            }),
            "evaluators",
        ),
        (
            "an empty evaluator list",
            json!({
                "batchEvaluationName": "empty_evaluators",
                "dataSourceConfig": cloudwatch_source(),
                "evaluators": [],
                "datasetExecutionConfig": run.clone(),
            }),
            "evaluators",
        ),
        (
            "a data source that is not cloudWatchLogs",
            json!({
                "batchEvaluationName": "online_source",
                "dataSourceConfig": {"onlineEvaluationConfigSource": {
                    "onlineEvaluationConfigArn":
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:\
                         online-evaluation-config/quality-0a1b2c3d4e"
                }},
                "evaluators": [{"evaluatorId": "Builtin.Correctness"}],
                "datasetExecutionConfig": run.clone(),
            }),
            "dataSourceConfig",
        ),
        (
            "insights beside the dataset run",
            json!({
                "batchEvaluationName": "with_insights",
                "dataSourceConfig": cloudwatch_source(),
                "evaluators": [{"evaluatorId": "Builtin.Correctness"}],
                "insights": [{"insightId": "Builtin.FailureAnalysis"}],
                "datasetExecutionConfig": run.clone(),
            }),
            "insights",
        ),
    ];

    for (case, body, member) in rows {
        let (status, refused) = start_batch_evaluation(&router, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {refused}"
        );
        let named: Vec<String> = refused
            .get("fieldList")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| text(field, &["name"]))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            named,
            vec![member.to_string()],
            "case {case:?}: expected the refusal to name {member}, got {refused}"
        );
    }

    assert_eq!(
        batch_evaluation_count(&engine),
        before,
        "a refused start must leave no job behind"
    );

    // The same request WITH everything the run must be accompanied by is accepted, so no
    // row above was refused for a reason of its own.
    let (status, started) = start_batch_evaluation(
        &router,
        start_with_run("accompanied_run", run),
    )
    .await;
    assert_eq!(
        status, 202,
        "a dataset run carrying its companions must be accepted, got {status} with {started}"
    );
}
