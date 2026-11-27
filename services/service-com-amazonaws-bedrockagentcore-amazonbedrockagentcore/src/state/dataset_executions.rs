// SPDX-License-Identifier: CC0-1.0

//! The DATASET RUN a batch evaluation carries out before it evaluates anything: which
//! version of which dataset its scenarios come from, and which agent runtime endpoint
//! each scenario is sent to.
//!
//! WHY THE CONFIGURATION IS RESOLVED ONCE. `StartBatchEvaluation`'s
//! `datasetExecutionConfig` is a REQUEST: it names a dataset and may leave the version
//! open, and it names an endpoint without saying which agent runtime version sits behind
//! it. `GetBatchEvaluation` reports `datasetExecutionDetails`, where both are settled —
//! "if the request didn't specify a version, this is the latest published version when
//! the batch evaluation started" and "the version of the agent runtime that the endpoint
//! pointed to when the batch evaluation started" (`smithy_query(query="shape",
//! shape="DatasetDetails")` and `shape="AgentCoreRuntimeInvocationDetails"`). So the
//! configuration is resolved by the call that STARTS the job, and the resolved details
//! are what the job holds and what every later read of it reports; a version published
//! afterwards does not change what the job ran.
//!
//! A dataset and an agent runtime endpoint are both the AgentCore CONTROL plane's
//! resources — this plane creates neither — so each is resolved against the component
//! that OWNS it, exactly as the runtime plane resolves an endpoint
//! ([`super::resolve_runtime_endpoint`]). What the model documents of the pair is
//! enforced here, in one place, so no two callers of this resolver can admit different
//! configurations:
//!
//! * the dataset "must use the predefined evaluation schema, and the version that runs
//!   must contain between 1 and 1,000 scenarios"
//!   (`smithy_query(query="shape", shape="DatasetExecutionConfig")`, member `dataset`);
//! * "You can't run the `DRAFT` version" (`shape="DatasetSource"`, member
//!   `datasetVersion`);
//! * "The service requires the endpoint and its agent runtime version to be ready"
//!   (`shape="AgentCoreRuntimeInvocation"`, member `agentRuntimeEndpointArn`).
//!
//! Each refusal names the member at fault, which is what the caller has to change.

use std::time::SystemTime;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as control_plane;

use crate::ops::types::{
    AgentCoreRuntimeInvocationDetails, AgentInvocationConfig, AgentInvocationDetails,
    DatasetDetails, DatasetExecutionConfig, DatasetExecutionDetails,
};

/// The control plane's own store, as this resolver reads it.
type ControlStore =
    control_plane::state::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState;

/// The `datasetExecutionConfig.dataset.datasetId` member path a refusal names.
pub const DATASET_EXECUTION_DATASET_ID: &str = "datasetExecutionConfig.dataset.datasetId";

/// The `datasetExecutionConfig.dataset.datasetVersion` member path a refusal names.
pub const DATASET_EXECUTION_DATASET_VERSION: &str =
    "datasetExecutionConfig.dataset.datasetVersion";

/// The `datasetExecutionConfig.agentInvocation.agentCoreRuntime.agentRuntimeEndpointArn`
/// member path a refusal names.
pub const DATASET_EXECUTION_ENDPOINT_ARN: &str =
    "datasetExecutionConfig.agentInvocation.agentCoreRuntime.agentRuntimeEndpointArn";

/// The sentence a call is refused with when the component that owns datasets and agent
/// runtimes cannot be reached, which is this emulator's fault and not the request's.
pub const DATASET_EXECUTION_STORE_UNAVAILABLE: &str =
    "the shared engine does not carry the AgentCore control plane's state";

/// The request payload template a dataset run uses for each turn when the request names
/// none: "If you don't specify a template, the service uses `{\"prompt\":
/// \"{{turn.input}}\"}`" (`smithy_query(query="doc", shape="AgentCoreRuntimeInvocation",
/// member="requestTemplate")`).
pub const DEFAULT_REQUEST_TEMPLATE: &str = "{\"prompt\": \"{{turn.input}}\"}";

/// The dataset schema a batch evaluation can run the scenarios of: "The dataset must use
/// the predefined evaluation schema", which is `DatasetSchemaType`'s
/// `AGENTCORE_EVALUATION_PREDEFINED_V1` — "Dataset with pre-written inputs per
/// conversation turn" (`smithy_query(query="shape", shape="DatasetSchemaType",
/// model_path="services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`).
pub const PREDEFINED_EVALUATION_SCHEMA: &str = "AGENTCORE_EVALUATION_PREDEFINED_V1";

/// The fewest scenarios a dataset version that runs may hold: "the version that runs must
/// contain between 1 and 1,000 scenarios".
pub const MIN_RUN_SCENARIOS: usize = 1;

/// The most scenarios a dataset version that runs may hold; see [`MIN_RUN_SCENARIOS`].
pub const MAX_RUN_SCENARIOS: usize = 1_000;

/// Why a dataset run a `StartBatchEvaluation` asked for cannot be set up, with the
/// sentence it is refused with. Which modelled error carries the sentence is the
/// operation's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatasetExecutionUnavailable {
    /// The request named a dataset, a version or an endpoint this account cannot run: the
    /// modelled `ValidationException`, over the member the refusal names.
    Refused {
        /// The member path at fault, for the message and the `fieldList` beside it.
        member: &'static str,
        message: String,
    },
    /// The component that owns datasets and agent runtimes could not be reached: a
    /// modelled `InternalServerException`.
    StoreUnavailable(String),
}

/// Refuse the run over `member`.
fn refused(member: &'static str, message: String) -> DatasetExecutionUnavailable {
    DatasetExecutionUnavailable::Refused { member, message }
}

/// Resolve the dataset run `requested` into the details a later read of the job reports,
/// read off the component that OWNS datasets and agent runtime endpoints.
///
/// Reading the owner's store materialises the dataset and runtime removals the clock has
/// already made due, which were owed at this instant whether or not anybody called;
/// nothing else about the owner's state is changed.
pub fn resolve_dataset_execution<Eng: Send + Sync + 'static>(
    engine: &Eng,
    requested: &DatasetExecutionConfig,
    now: SystemTime,
) -> Result<DatasetExecutionDetails, DatasetExecutionUnavailable> {
    let Some(control) = control_plane::state::service_state(engine) else {
        return Err(DatasetExecutionUnavailable::StoreUnavailable(
            DATASET_EXECUTION_STORE_UNAVAILABLE.to_string(),
        ));
    };
    let mut store = control.lock();
    store.expire_deleted_datasets(now);
    store.expire_deleted_agent_runtimes(now);

    let dataset = resolve_dataset(&store, requested)?;
    let agent_invocation = resolve_agent_invocation(&store, requested, now)?;
    Ok(DatasetExecutionDetails {
        dataset,
        agent_invocation,
    })
}

/// The dataset version the run reads its scenarios from: the one the request named, or
/// the latest version published when the job started.
fn resolve_dataset(
    store: &ControlStore,
    requested: &DatasetExecutionConfig,
) -> Result<DatasetDetails, DatasetExecutionUnavailable> {
    let dataset_id = requested.dataset.dataset_id.as_str();
    let Some(held) = store.datasets.get(dataset_id) else {
        return Err(refused(
            DATASET_EXECUTION_DATASET_ID,
            "The specified dataset does not exist. Please verify the datasetId and try again."
                .to_string(),
        ));
    };
    if held.schema_type != PREDEFINED_EVALUATION_SCHEMA {
        return Err(refused(
            DATASET_EXECUTION_DATASET_ID,
            format!(
                "Dataset {dataset_id} uses schema {}. A batch evaluation can only run the \
                 scenarios of a dataset that uses {PREDEFINED_EVALUATION_SCHEMA}.",
                held.schema_type
            ),
        ));
    }

    let version = match requested.dataset.dataset_version.as_deref() {
        Some(named) if named == control_plane::state::DATASET_VERSION_DRAFT => {
            return Err(refused(
                DATASET_EXECUTION_DATASET_VERSION,
                format!(
                    "The {} version of a dataset cannot be run. Specify a published version of \
                     dataset {dataset_id}.",
                    control_plane::state::DATASET_VERSION_DRAFT
                ),
            ));
        }
        Some(named) => named
            .parse::<u32>()
            .ok()
            .and_then(|number| held.version(number))
            .ok_or_else(|| {
                refused(
                    DATASET_EXECUTION_DATASET_VERSION,
                    format!(
                        "Dataset {dataset_id} has no published version {named}. Please verify \
                         the datasetVersion and try again."
                    ),
                )
            })?,
        // "If you don't specify a version, the batch evaluation uses the latest published
        // version", which is the last one published from the working copy.
        None => held.versions().last().ok_or_else(|| {
            refused(
                DATASET_EXECUTION_DATASET_VERSION,
                format!(
                    "Dataset {dataset_id} has no published version to run. Publish a version of \
                     the dataset and run that version."
                ),
            )
        })?,
    };

    let scenarios = version.examples.len();
    if !(MIN_RUN_SCENARIOS..=MAX_RUN_SCENARIOS).contains(&scenarios) {
        return Err(refused(
            DATASET_EXECUTION_DATASET_VERSION,
            format!(
                "Version {} of dataset {dataset_id} contains {scenarios} scenarios. The version \
                 that runs must contain between {MIN_RUN_SCENARIOS} and {MAX_RUN_SCENARIOS} \
                 scenarios.",
                version.version
            ),
        ));
    }

    Ok(DatasetDetails {
        dataset_id: dataset_id.to_string(),
        dataset_version: version.version.to_string(),
    })
}

/// The agent the run sends each scenario to: the endpoint the request named, the agent
/// runtime version that endpoint points at, and the request template each turn is sent
/// under.
fn resolve_agent_invocation(
    store: &ControlStore,
    requested: &DatasetExecutionConfig,
    now: SystemTime,
) -> Result<AgentInvocationDetails, DatasetExecutionUnavailable> {
    let AgentInvocationConfig::agentCoreRuntime(invocation) = &requested.agent_invocation;
    let endpoint_arn = invocation.agent_runtime_endpoint_arn.as_str();
    // The stored endpoint's OWN ARN is what addresses it, so an ARN naming another
    // account's endpoint resolves to nothing, however well formed it is.
    let endpoint = store
        .agent_runtimes
        .values()
        .flat_map(|runtime| runtime.endpoints.values())
        .find(|endpoint| endpoint.endpoint_arn == endpoint_arn)
        .ok_or_else(|| {
            refused(
                DATASET_EXECUTION_ENDPOINT_ARN,
                "The specified agent runtime endpoint does not exist. Please verify the \
                 agentRuntimeEndpointArn and try again."
                    .to_string(),
            )
        })?;
    let status = endpoint.status_at(now);
    if status != control_plane::state::STATUS_READY {
        return Err(refused(
            DATASET_EXECUTION_ENDPOINT_ARN,
            format!(
                "Agent runtime endpoint {endpoint_arn} is {status}. A batch evaluation requires \
                 the endpoint and its agent runtime version to be {}.",
                control_plane::state::STATUS_READY
            ),
        ));
    }

    Ok(AgentInvocationDetails::agentCoreRuntime(
        AgentCoreRuntimeInvocationDetails {
            agent_runtime_endpoint_arn: endpoint.endpoint_arn.clone(),
            agent_runtime_version: endpoint.target_version_string(),
            request_template: Some(
                invocation
                    .request_template
                    .clone()
                    .unwrap_or_else(|| DEFAULT_REQUEST_TEMPLATE.to_string()),
            ),
        },
    ))
}
