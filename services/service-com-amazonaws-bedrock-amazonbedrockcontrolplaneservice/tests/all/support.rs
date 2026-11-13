// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives the emulator through.
//!
//! Extracted out of `guardrails.rs` so the other suites exercise the SAME dispatch path
//! rather than a second copy of it: one shared engine, one registered service, requests
//! signed the way an SDK signs them and dispatched in-process through the
//! `MultiServiceRouter` the emulator boots.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

/// The region every request in these suites signs for; it is what the minted ARNs must
/// carry.
pub const REGION: &str = "us-west-2";

/// The canonical emulator account id every ARN carries.
pub const ACCOUNT: &str = "000000000000";

/// One emulator answer: the HTTP status, the `content-type` it was serialized under, the
/// parsed JSON body, and the wire error code when it is a failure.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Value,
    pub error: Option<String>,
}

impl Reply {
    /// The body member `name`, or `Value::Null` when the response omits it.
    pub fn get(&self, name: &str) -> Value {
        self.body.get(name).cloned().unwrap_or(Value::Null)
    }

    /// The body member `name` as a string; panics naming the member when it is absent.
    pub fn text(&self, name: &str) -> String {
        self.get(name)
            .as_str()
            .unwrap_or_else(|| panic!("response has no string member {name}: {:?}", self.body))
            .to_string()
    }

    /// The instant the body member `name` carries, in epoch seconds.
    ///
    /// Every `Timestamp` member of this service's guardrail responses is modelled
    /// `@timestampFormat("date-time")`, so the wire form is an RFC 3339 STRING and this
    /// panics on anything else — including on the epoch NUMBER the protocol default would
    /// have produced.
    pub fn instant(&self, name: &str) -> f64 {
        let value = self.get(name);
        let text = value.as_str().unwrap_or_else(|| {
            panic!(
                "member {name} is modelled @timestampFormat(\"date-time\"), so it must be an \
                 RFC 3339 string, got {value}"
            )
        });
        date_time_seconds(name, text)
    }
}

/// An RFC 3339 date-time as epoch seconds, read with the same codec the wire uses
/// (`aws_smithy_types::date_time::Format::DateTime`).
pub fn date_time_seconds(case: &str, text: &str) -> f64 {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case}: {text:?} does not parse as an RFC 3339 date-time: {error}")
    });
    parsed.as_secs_f64()
}

/// An RFC 3339 `date-time` as whole milliseconds since the epoch.
///
/// Beside [`date_time_seconds`] because every suite whose rule is about a `lastUpdatedAt`
/// concurrency token reads a reported instant and sends one back at THIS resolution — the
/// finest a client that carries milliseconds can echo — and one rendering of the pair is
/// what keeps those suites reading and writing the same wire form.
pub fn epoch_millis(case: &str, text: &str) -> i64 {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: {text:?} does not parse as an RFC 3339 date-time: {error}")
    });
    parsed.secs() * 1_000 + i64::from(parsed.subsec_nanos() / 1_000_000)
}

/// `millis` since the epoch as the `date-time` text a client sends.
pub fn at_millis(case: &str, millis: i64) -> String {
    ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {millis} does not render: {error}"))
}

/// The test's own wall clock in epoch seconds, beside [`date_time_seconds`] so a suite that
/// bounds a response's instant against the clock reads it the same way every other suite
/// does: a stamp the emulator reports for a write must fall between the clock before the
/// call and the clock after it.
pub fn wall_clock_seconds() -> f64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// An identifier as an SDK writes it into a path label: an ARN carries `:` and `/`, and
/// both are percent-encoded so the whole identifier stays ONE path segment.
pub fn label(identifier: &str) -> String {
    identifier
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// The `FoundationModelArn` of the base model `model_id` in `region`: a foundation model is
/// published by AWS rather than owned by the account reading it, so the ARN carries an
/// EMPTY account field
/// (`smithy_query(query="shape",shape="FoundationModelDetails")`).
///
/// Here rather than in one suite because three of them compare a response's ARN against
/// it, and the spelling they compare against must be ONE spelling.
pub fn foundation_model_arn(region: &str, model_id: &str) -> String {
    format!("arn:aws:bedrock:{region}::foundation-model/{model_id}")
}

/// The base `modelId` a published `inference-profile` ARN routes to: the
/// `inferenceProfileId` behind its geography prefix (`us.<modelId>`).
///
/// Here rather than in one suite because both the published-catalogue suites read a
/// profile ARN this way, and how a published profile names its model must be ONE reading.
pub fn profile_model_id(profile_arn: &str) -> String {
    profile_arn
        .rsplit_once("inference-profile/")
        .and_then(|(_, id)| id.split_once('.'))
        .map(|(_, model_id)| model_id.to_string())
        .unwrap_or_default()
}

/// The WHOLE projection of the published catalog row `model` that a listing
/// (`ListFoundationModels`) and a read (`GetFoundationModel`) must both report for it in
/// `region`: the ten members `FoundationModelSummary`/`FoundationModelDetails` declare
/// (`smithy_query(query="shape",shape="FoundationModelSummary")`) and the capability
/// members a catalog listing reports beside them.
///
/// Computed HERE from the row's own published attributes rather than by asking the service
/// for them, so it is an independent statement of the rule a summary must satisfy:
///
/// * The capability metadata is published for a model that TAKES PART IN A CONVERSATION —
///   one that produces TEXT — and for no other: the summary of a model that answers with a
///   vector or an image carries the ten modelled members and `guardrailsSupported`, and
///   none of `modelFamily`, `inferenceAPIsSupported`, `featuresSupported`,
///   `explicitPromptCaching`, `intelligentPromptRouting`, `latencyOptimizationSupported`,
///   `batchSupported` or `converse`.
/// * `modelFamily` is the row's family.
/// * `inferenceAPIsSupported` carries `invokeModel` for every offered model, `converse` for
///   one that produces TEXT, and the two OpenAI-compatible endpoints for a model that
///   provider publishes.
/// * `guardrailsSupported` holds of a model that produces TEXT — a guardrail decides about
///   text and about nothing else, and is applied beside the invocation rather than built
///   into the model's integration, so nothing else is needed of the model.
/// * `featuresSupported` names each Amazon Bedrock feature the row publishes support for,
///   beside `countTokens`, which is published for the family a token count is served for;
///   `explicitPromptCaching`, `intelligentPromptRouting`, `latencyOptimizationSupported`
///   and `batchSupported` each report that one feature — each is a published per-model
///   fact, so a model no feature's supported-models table lists supports none of them. A
///   capability is reported as the RECORD naming what is supported, and batch inference's
///   record answers for the base model, for another region and for a customization of the
///   model, that last one only where the model accepts a customization at all.
/// * `converse` reports what a conversation may carry to the model and ask of it.
///
/// Here rather than in one suite because the listing suite, the read suite and the
/// capability suite all compare a response against it, and the projection they compare
/// against must be ONE projection.
pub fn expected_foundation_model_projection(
    model: &bedrock::state::FoundationModel,
    region: &str,
) -> Value {
    let conversational = model.output_modalities.contains(&"TEXT");
    let published = |feature: &str| model.features_supported.contains(&feature);
    let takes = |modality: &str| model.input_modalities.contains(&modality);

    let mut apis: Vec<&str> = Vec::new();
    if conversational {
        apis.push("converse");
    }
    apis.push("invokeModel");
    if model.provider_name == "OpenAI" {
        apis.push("openAiChatCompletions");
        apis.push("openAiResponses");
    }

    let mut lifecycle = json!({
        "status": model.lifecycle_status,
        // Every offered model has launched, so its projection carries its launch instant.
        "startOfLifeTime": model.start_of_life_time,
    });
    // Both ends of the legacy window AWS publishes, for a deprecated model and for no
    // other; `publicExtendedAccessTime` is published for none.
    if let Some(window) = model.legacy_window {
        lifecycle["legacyTime"] = json!(window.entered_at);
        lifecycle["endOfLifeTime"] = json!(window.end_of_life_at);
    }

    let mut projection = json!({
        "modelArn": foundation_model_arn(region, model.model_id),
        "modelId": model.model_id,
        "modelName": model.model_name,
        "providerName": model.provider_name,
        "inputModalities": model.input_modalities,
        "outputModalities": model.output_modalities,
        "responseStreamingSupported": model.response_streaming_supported,
        "customizationsSupported": model.customizations_supported,
        "inferenceTypesSupported": model.inference_types_supported,
        "modelLifecycle": lifecycle,
        "guardrailsSupported": conversational,
    });
    if !conversational {
        return projection;
    }

    // The formats a turn may carry TO the model: one modality's format set when the model
    // takes that modality, and empty when it does not.
    let formats = |carried: bool, values: &'static [&'static str]| {
        if carried {
            values.to_vec()
        } else {
            Vec::new()
        }
    };
    let mut converse = json!({
        "supported": true,
        "streamingSupported": model.response_streaming_supported,
        "systemRoleSupported": true,
        "reasoningSupported": { "isSupported": expected_reasoning_supported(model) },
        "stopSequencesDefault": Vec::<&str>::new(),
        "userDocumentTypesSupported": formats(takes("TEXT"), DOCUMENT_FORMATS),
        "userImageTypesSupported": formats(takes("IMAGE"), IMAGE_FORMATS),
        // No modality the model declares is video, so no row takes one.
        "userVideoTypesSupported": Vec::<&str>::new(),
    });
    if let Some(ceiling) = expected_max_output_tokens(model) {
        converse["maxTokensMaximum"] = json!(ceiling);
    }

    projection["modelFamily"] = json!(model.model_family);
    projection["inferenceAPIsSupported"] = json!(apis);
    projection["featuresSupported"] = json!({
        "batchInference": published("BATCH_INFERENCE"),
        "countTokens": expected_count_tokens_supported(model),
        "intelligentPromptRouting": published("INTELLIGENT_PROMPT_ROUTING"),
        "latencyOptimization": published("LATENCY_OPTIMIZATION"),
        "promptCaching": published("PROMPT_CACHING"),
        // A prompt optimization job rewrites a prompt template for the models it targets,
        // so it needs of a model only that a prompt is what the model is given.
        "promptOptimization": conversational,
    });
    projection["explicitPromptCaching"] = json!({ "isSupported": published("PROMPT_CACHING") });
    projection["intelligentPromptRouting"] =
        json!({ "isSupported": published("INTELLIGENT_PROMPT_ROUTING") });
    projection["latencyOptimizationSupported"] = json!(published("LATENCY_OPTIMIZATION"));
    projection["batchSupported"] = json!({
        "baseModelSupported": published("BATCH_INFERENCE"),
        "crossRegionSupported": published("BATCH_INFERENCE"),
        "customModelSupported":
            published("BATCH_INFERENCE") && !model.customizations_supported.is_empty(),
    });
    projection["converse"] = converse;
    projection
}

/// The document formats a turn of a conversation may carry to a model that takes TEXT
/// (`DocumentFormat` in the Amazon Bedrock Runtime model).
const DOCUMENT_FORMATS: &[&str] = &[
    "pdf", "csv", "doc", "docx", "xls", "xlsx", "html", "txt", "md",
];

/// The image formats a turn may carry to a model that takes IMAGE (`ImageFormat`).
const IMAGE_FORMATS: &[&str] = &["png", "jpeg", "gif", "webp"];

/// Whether the provider publishes extended reasoning for the row, stated HERE rather than
/// read off the service: the suites assert that the summary reports what the catalogue
/// publishes, so the published set has to be stated independently of the one the handler
/// reads, or a wrong row in it is never caught.
///
/// The reasoning lines of each provider the catalogue carries: the Anthropic Claude lines
/// from Claude 4 on, which publish extended thinking, and the line each other provider
/// publishes as its reasoning model.
pub fn expected_reasoning_supported(model: &bedrock::state::FoundationModel) -> bool {
    const PUBLISHED_REASONING_MODELS: &[&str] = &[
        "anthropic.claude-fable-5",
        "anthropic.claude-fable-5-1",
        "anthropic.claude-haiku-4-5-20251001-v1:0",
        "anthropic.claude-opus-4-1-20250805-v1:0",
        "anthropic.claude-opus-4-5-20251101-v1:0",
        "anthropic.claude-opus-4-6-v1",
        "anthropic.claude-opus-4-7",
        "anthropic.claude-opus-4-8",
        "anthropic.claude-opus-5",
        "anthropic.claude-opus-5-5",
        "anthropic.claude-sonnet-4-20250514-v1:0",
        "anthropic.claude-sonnet-4-5-20250929-v1:0",
        "anthropic.claude-sonnet-4-6",
        "anthropic.claude-sonnet-5",
        "anthropic.claude-sonnet-5-5",
        "deepseek.r1-v1:0",
        "mistral.magistral-small-2509",
        "moonshot.kimi-k2-thinking",
        "nvidia.nemotron-super-3-120b",
        "qwen.qwen3-235b-a22b-2507-v1:0",
        "qwen.qwen3-32b-v1:0",
        "qwen.qwen3-next-80b-a3b",
    ];
    PUBLISHED_REASONING_MODELS.contains(&model.model_id)
}

/// The ceiling on generated tokens the catalogue publishes for the row, if it publishes one
/// — stated HERE for the same reason as the reasoning set above.
///
/// The figures each provider publishes for the model ("Inference parameters for foundation
/// models" in the Amazon Bedrock User Guide), which are the figures this emulator's
/// inference plane caps a generated answer at for the same model; a model whose provider
/// publishes no ceiling is in no row, and no ceiling is reported for it.
pub fn expected_max_output_tokens(model: &bedrock::state::FoundationModel) -> Option<i32> {
    const PUBLISHED_MAX_OUTPUT_TOKENS: &[(&str, i32)] = &[
        ("amazon.nova-lite-v1:0", 5120),
        ("amazon.nova-micro-v1:0", 5120),
        ("amazon.nova-pro-v1:0", 5120),
        ("amazon.titan-text-lite-v1", 4096),
        ("amazon.titan-text-premier-v1:0", 3072),
        ("anthropic.claude-3-5-sonnet-20240620-v1:0", 8192),
        ("anthropic.claude-3-5-sonnet-20241022-v2:0", 8192),
        ("anthropic.claude-3-haiku-20240307-v1:0", 4096),
        ("anthropic.claude-3-sonnet-20240229-v1:0", 4096),
        ("cohere.command-light-text-v14", 4096),
        ("cohere.command-text-v14", 4096),
        ("meta.llama3-1-70b-instruct-v1:0", 2048),
        ("meta.llama3-1-8b-instruct-v1:0", 2048),
        ("meta.llama3-3-70b-instruct-v1:0", 2048),
        ("meta.llama3-70b-instruct-v1:0", 2048),
        ("meta.llama3-8b-instruct-v1:0", 2048),
        ("mistral.mistral-7b-instruct-v0:2", 8192),
        ("mistral.mistral-large-2402-v1:0", 8192),
        ("mistral.mistral-large-2407-v1:0", 8192),
        ("mistral.mixtral-8x7b-instruct-v0:1", 4096),
    ];
    PUBLISHED_MAX_OUTPUT_TOKENS
        .iter()
        .find(|(model_id, _)| *model_id == model.model_id)
        .map(|(_, ceiling)| *ceiling)
}

/// Whether a token count is served for the row: a count is the MODEL's own tokenizer's, so
/// it is served for the one provider's family this emulator's inference plane carries a
/// tokenizer for.
pub fn expected_count_tokens_supported(model: &bedrock::state::FoundationModel) -> bool {
    model.provider_name == "Anthropic"
}

/// The bare error code out of an `__type` / `x-amzn-errortype` value, which may carry a
/// namespace or a URL beside it.
pub fn error_code(raw: &str) -> &str {
    let without_url = raw.split(':').next().unwrap_or(raw);
    without_url.rsplit('#').next().unwrap_or(without_url)
}

/// Store the build workflow an admitted `StartAutomatedReasoningPolicyBuildWorkflow` would
/// have written `created_ago` ago, and hand back its `buildWorkflowId`.
///
/// The record is built out of the state helpers the start itself uses
/// ([`bedrock::state::mint_build_workflow_id`] and the status constants), so a seeded
/// workflow is the workflow a start would have left — and seeding is what lets a row fix the
/// instant a workflow was started, and so the order a listing must walk, or the status a
/// request must find it in, without sleeping for either. `tweak` is where a row differs from
/// that baseline.
///
/// Here rather than in one suite because two of them seed this record, and ONE rendering of
/// it is what keeps them from drifting apart when
/// [`bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord`] gains a field.
pub fn seed_build_workflow(
    emu: &Emulator,
    policy_arn: &str,
    workflow_type: &str,
    created_ago: Duration,
    tweak: impl FnOnce(&mut bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord),
) -> String {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let now = SystemTime::now();
    let created_at = now - created_ago;
    let mut store = state.lock();
    let id = store.mint_build_workflow_id(now);
    let mut record = bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord {
        policy_arn: policy_arn.to_string(),
        id: id.clone(),
        workflow_type: workflow_type.to_string(),
        status: bedrock::state::BUILD_WORKFLOW_STATUS_SCHEDULED,
        preprocessing_due_at: None,
        building_due_at: None,
        testing_due_at: None,
        completed_due_at: None,
        cancelled_due_at: None,
        document_name: None,
        document_content_type: None,
        document_description: None,
        annotations: Vec::new(),
        annotations_updated_at: created_at,
        created_at,
        updated_at: created_at,
    };
    // The whole progression the start arms, on the timeline of the instant this workflow is
    // seeded as having been started at — so a workflow seeded long enough ago is as terminal
    // as one a start left that long ago, and one seeded moments ago is as scheduled.
    arm_build_workflow(&mut record, created_at);
    tweak(&mut record);
    store
        .automated_reasoning_policy_build_workflows
        .insert((policy_arn.to_string(), id.clone()), record);
    id
}

/// Arm every status transition the clock owes `record`, as though its build had been started
/// at `start_time` — the very clock
/// `StartAutomatedReasoningPolicyBuildWorkflow` arms
/// ([`bedrock::state::AutomatedReasoningPolicyBuildWorkflowClock`]), so a seeded workflow
/// progresses exactly as a started one does.
pub fn arm_build_workflow(
    record: &mut bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord,
    start_time: SystemTime,
) {
    let clock = bedrock::state::AutomatedReasoningPolicyBuildWorkflowClock::armed_at(start_time);
    record.preprocessing_due_at = Some(clock.preprocessing_at);
    record.building_due_at = Some(clock.building_at);
    record.testing_due_at = Some(clock.testing_at);
    record.completed_due_at = Some(clock.completed_at);
}

/// Age the stored build workflow `(policy_arn, build_workflow_id)` names by `by`, as though
/// it had been started that much earlier: its own timeline is untouched and only how much of
/// it has elapsed changes, so a rule about a span of wall-clock time needs no sleep.
///
/// Here rather than in one suite because four of them carry a workflow past a stage of its
/// build, and ONE shift of its instants is what keeps them from drifting apart when
/// [`bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord`] gains another due instant.
pub fn age_build_workflow(emu: &Emulator, policy_arn: &str, build_workflow_id: &str, by: Duration) {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let record = store
        .automated_reasoning_policy_build_workflows
        .get_mut(&(policy_arn.to_string(), build_workflow_id.to_string()))
        .unwrap_or_else(|| {
            panic!("the store must hold the build workflow {build_workflow_id} of {policy_arn}")
        });
    record.created_at -= by;
    record.updated_at -= by;
    for due in [
        &mut record.preprocessing_due_at,
        &mut record.building_due_at,
        &mut record.testing_due_at,
        &mut record.completed_due_at,
        &mut record.cancelled_due_at,
    ] {
        if let Some(instant) = due.as_mut() {
            *instant -= by;
        }
    }
}

/// Age the stored model customization job `arn` names by `by`, as though it had been
/// submitted that much earlier: its own timeline is untouched and only how much of it has
/// elapsed changes, so a rule about a span of wall-clock time needs no sleep — neither the
/// job's own completion nor the custom model that completion brings into being.
///
/// Here rather than in one suite because two of them age this record (the job suite, for the
/// job's own status, and the custom-model deletion suite, for the model a completed job
/// produced), and ONE shift of a job's instants is what keeps them from drifting apart when
/// [`bedrock::state::ModelCustomizationJobRecord`] gains another due instant.
pub fn age_model_customization_job(emu: &Emulator, arn: &str, by: Duration) {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let job = store
        .model_customization_jobs
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the store must hold the job {arn}"));
    job.created_at -= by;
    job.last_modified_at -= by;
    for due in [&mut job.completed_due_at, &mut job.stopped_due_at] {
        if let Some(instant) = due.as_mut() {
            *instant -= by;
        }
    }
}

/// The emulator under test: one shared engine, one registered service, dispatched
/// in-process.
pub struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    /// The very engine the registered service holds, so a suite whose rule is about a span
    /// of WALL-CLOCK time can read the store the handlers write and age a record it holds
    /// instead of sleeping for that span. Requests still travel through
    /// [`Self::call_in_region`]; this is never a second store.
    pub engine: Arc<engine::InMemoryEngine>,
}

pub fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// The same emulator with the Bedrock Runtime DATA plane mounted beside this control plane
/// on the one shared engine, exactly as `core/router` assembles them.
///
/// The form a suite drives when its rule spans the two planes — an account-enforced
/// guardrail this control plane installs and that data plane applies — so the request that
/// installs a configuration and the invocation that is evaluated against it travel through
/// ONE emulator. The two services are told apart by the path space their own models
/// publish, so [`Emulator::call`] reaches either.
pub fn emulator_with_inference() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
}

/// The same emulator with CloudWatch Logs and IAM mounted beside this control plane on the
/// one shared engine, exactly as `core/router` assembles them.
///
/// The form a suite drives when its rule spans this service and the services that OWN the
/// resources a request names — the log group a model invocation logging destination writes
/// into, the role it writes under — so the question this control plane puts over its
/// in-process callback reaches those very stores. Requests to the other two go through
/// [`Emulator::call_awsjson`] (CloudWatch Logs) and [`Emulator::engine`] (the IAM records a
/// suite seeds).
pub fn emulator_with_logs_and_iam() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        ::service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
}

impl Emulator {
    /// One request signed for [`REGION`], the region these suites address.
    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.call_in_region(REGION, method, path, body).await
    }

    /// One request signed for `region`.
    ///
    /// The region a request addresses travels in its SigV4 credential scope, and a
    /// service reads it from there (`crate::state::request_region`), so a suite whose
    /// rule is about that region — an ARN compared against it, an ARN minted with it —
    /// drives this form and the rest keep [`call`](Self::call).
    pub async fn call_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        self.call_with_headers(region, method, path, body, &[])
            .await
    }

    /// One request signed for `region`, carrying `headers` beside the ones every request
    /// here carries.
    ///
    /// The form a suite drives for an input member bound to a HEADER rather than to the
    /// body — an `@idempotencyToken` the model binds to `x-amz-client-token`, say — so
    /// such a member travels where the wire puts it.
    pub async fn call_with_headers(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://bedrock.{region}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/bedrock/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .header("content-type", "application/json");
        let request = headers
            .iter()
            .fold(request, |request, (name, value)| {
                request.header(*name, *value)
            })
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        reply_of(response)
    }

    /// One awsJson1.1 call to ANOTHER emulated service mounted beside this one — `POST /`
    /// with `target` in `x-amz-target`, which is how an RPC-protocol service addresses its
    /// operation.
    ///
    /// The form a suite drives to stage a resource in the service that OWNS it, so what this
    /// control plane then resolves over its in-process callback is a resource that service
    /// really holds. `signing_name` travels in the SigV4 credential scope, which is what
    /// routes the request to that service.
    pub async fn call_awsjson(&self, signing_name: &str, target: &str, body: Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("a serializable request body"),
            ))
            .expect("a well-formed request");
        reply_of(self.router.dispatch(request).await)
    }
}

/// One emulator answer, read off the HTTP response: the status, the `content-type`, the
/// parsed body and the wire error code, wherever the protocol put it (`x-amzn-errortype`, or
/// the body's `__type`).
///
/// One reading for every call above, so a suite driving this service and a suite driving a
/// service mounted beside it decode an answer the same way.
fn reply_of(response: http::Response<bytes::Bytes>) -> Reply {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| error_code(value).to_string());
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error = header_error.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(|value| error_code(value).to_string())
    });
    Reply {
        status,
        content_type,
        body,
        error,
    }
}

impl Emulator {

    pub async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/guardrails", Some(body)).await
    }

    pub async fn get(&self, identifier: &str, version: Option<&str>) -> Reply {
        let identifier = label(identifier);
        let path = match version {
            Some(version) => format!("/guardrails/{identifier}?guardrailVersion={version}"),
            None => format!("/guardrails/{identifier}"),
        };
        self.call("GET", &path, None).await
    }

    pub async fn list(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/guardrails".to_string()
        } else {
            format!("/guardrails?{query}")
        };
        self.call("GET", &path, None).await
    }

    pub async fn update(&self, identifier: &str, body: Value) -> Reply {
        let identifier = label(identifier);
        self.call("PUT", &format!("/guardrails/{identifier}"), Some(body))
            .await
    }

    pub async fn create_version(&self, identifier: &str, body: Value) -> Reply {
        let identifier = label(identifier);
        self.call("POST", &format!("/guardrails/{identifier}"), Some(body))
            .await
    }

    pub async fn delete(&self, identifier: &str, version: Option<&str>) -> Reply {
        let identifier = label(identifier);
        let path = match version {
            Some(version) => format!("/guardrails/{identifier}?guardrailVersion={version}"),
            None => format!("/guardrails/{identifier}"),
        };
        self.call("DELETE", &path, None).await
    }

    /// Create a guardrail with only the required members and hand back its id.
    pub async fn create_named(&self, name: &str) -> String {
        let reply = self
            .create(json!({
                "name": name,
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            }))
            .await;
        assert_eq!(
            reply.status, 202,
            "case {name}: CreateGuardrail must answer 202, got {} {:?}",
            reply.status, reply.body
        );
        reply.text("guardrailId")
    }
}
