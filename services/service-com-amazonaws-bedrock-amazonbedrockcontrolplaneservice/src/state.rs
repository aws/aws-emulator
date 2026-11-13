// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrock-amazonbedrockcontrolplaneservice service.
//!
//! Today this holds the guardrail resource: the control plane's self-contained
//! CRUD resource, owned by `ops/{create,get,list,update,delete}_guardrail.rs` and
//! `ops/create_guardrail_version.rs`. Every one of those handlers reads and writes
//! THIS struct through [`service_state`], so a guardrail created by one request is
//! the guardrail every later request sees.
//!
//! It also holds the account's model invocation logging configuration — the single
//! settings record `ops/{get,put,delete}_model_invocation_logging_configuration.rs`
//! share, so the configuration one request stores is the configuration the next
//! request reads back.
//!
//! It also holds the account's enforced guardrail configuration — the single record
//! `ops/put_enforced_guardrail_configuration.rs` installs and
//! `ops/list_enforced_guardrails_configuration.rs` reports, so the configuration one
//! request stores is the configuration the next request lists.
//!
//! And it holds the prompt routers `ops/create_prompt_router.rs` mints,
//! `ops/get_prompt_router.rs` reads back, `ops/list_prompt_routers.rs` reports and
//! `ops/delete_prompt_router.rs` removes, so a router created by one request is the
//! router every later request sees and a router one request deleted is gone for every
//! later one.
//!
//! And it holds the model import jobs `ops/create_model_import_job.rs` mints and
//! `ops/get_model_import_job.rs` reads back, together with the instant each job's
//! non-terminal `status` is owed its move to `Completed`. Those same records hold the
//! models those jobs import, which `ops/get_imported_model.rs` reads back once the
//! import has completed — the job that imports a model is where that model's identity,
//! data source and key live, so there is only ever one copy of either.
//!
//! And it holds the evaluation jobs `ops/create_evaluation_job.rs` mints and
//! `ops/get_evaluation_job.rs` reads back, together with the instant each job's
//! non-terminal status is owed its move to a terminal one and the instant each job
//! `ops/batch_delete_evaluation_job.rs` accepted for deletion is owed its removal. Each of
//! those records also holds what the job's WORK established — whether its run is still owed,
//! the terminal status that run decided and the sentences a job that could not be run
//! reports ([`evaluation_run`]) — because a job reported as having run and the result
//! objects under its output prefix are two sides of one run.
//!
//! And it holds the advanced prompt optimization jobs
//! `ops/create_advanced_prompt_optimization_job.rs` mints,
//! `ops/get_advanced_prompt_optimization_job.rs` reads back and
//! `ops/stop_advanced_prompt_optimization_job.rs` winds down, together with the instant
//! each job's non-terminal `jobStatus` is owed its move to a terminal one.
//!
//! And it holds the model customization jobs `ops/create_model_customization_job.rs`
//! mints, `ops/get_model_customization_job.rs` reads back and
//! `ops/stop_model_customization_job.rs` winds down, together with the instant each job's
//! non-terminal `status` is owed its move to a terminal one. A job that reaches
//! `Completed` is what brings the custom model it was customizing into being, so the
//! completion registers that model in the custom models below rather than holding a second
//! copy of it: the model a job produced is the model `ops/get_custom_model.rs` and
//! `ops/list_custom_models.rs` report.
//!
//! And it holds the model copy jobs `ops/create_model_copy_job.rs` submits and
//! `ops/get_model_copy_job.rs` reads back, together with the instant each job's
//! non-terminal `status` is owed its move to `Completed`. A job that completes is what
//! brings the COPIED model into being, so the completion registers that model in the
//! custom models below rather than holding a second copy of it: the model a copy produced
//! is the model `ops/get_custom_model.rs` and `ops/list_custom_models.rs` report.
//!
//! And it holds the Bedrock Marketplace model endpoints
//! `ops/create_marketplace_model_endpoint.rs` mints,
//! `ops/update_marketplace_model_endpoint.rs` reconfigures,
//! `ops/register_marketplace_model_endpoint.rs` registers with Amazon Bedrock,
//! `ops/get_marketplace_model_endpoint.rs` reads back and
//! `ops/deregister_marketplace_model_endpoint.rs` drops, together with the instant each
//! endpoint's non-terminal `endpointStatus` is owed its move to `InService`.
//!
//! And it holds the Provisioned Throughputs `ops/create_provisioned_model_throughput.rs`
//! mints, `ops/get_provisioned_model_throughput.rs` reads back,
//! `ops/list_provisioned_model_throughputs.rs` reports and
//! `ops/delete_provisioned_model_throughput.rs` removes, together with the instant each
//! one's non-terminal `status` is owed its move to `InService`.
//!
//! And it holds the custom models `ops/create_custom_model.rs` mints,
//! `ops/get_custom_model.rs` reads back, `ops/list_custom_models.rs` reports and
//! `ops/delete_custom_model.rs` removes, together with the instant each one's non-terminal
//! `modelStatus` is owed its move to `Active`.
//!
//! And it holds the custom model deployments `ops/create_custom_model_deployment.rs`
//! mints, `ops/get_custom_model_deployment.rs` reads back and
//! `ops/list_custom_model_deployments.rs` reports, together with the instant each
//! deployment's non-terminal `status` is owed its move to `Active`.
//!
//! And it holds the Automated Reasoning policies `ops/create_automated_reasoning_policy.rs`
//! mints, `ops/update_automated_reasoning_policy.rs` rewrites and
//! `ops/get_automated_reasoning_policy.rs` reads back, each with the draft definition it
//! was last written with and the hash of that definition, so the hash the write answered
//! is the hash the read reports — and, beside that draft, every numbered version
//! `ops/create_automated_reasoning_policy_version.rs` cuts from it, which the same read
//! reports when the ARN it is given carries that version's number, unchanged by any later
//! write to the draft. `ops/list_automated_reasoning_policies.rs` reports those same
//! revisions as summaries — every policy's draft, or the revisions of the one policy an ARN
//! filter names — so a listing reports what a read of each revision finds.
//! `ops/export_automated_reasoning_policy_version.rs` reads the
//! DEFINITION of whichever of those revisions the ARN names, so what a revision was written
//! with is what the export reports.
//! `ops/delete_automated_reasoning_policy.rs` removes whichever of those revisions the ARN
//! names — one numbered version, or the policy with every artifact of it held below — so a
//! revision one request deleted is a revision no later request finds.
//!
//! And it holds the build workflows of those policies —
//! `ops/start_automated_reasoning_policy_build_workflow.rs` starts them,
//! `ops/cancel_automated_reasoning_policy_build_workflow.rs` stops them and
//! `ops/get_automated_reasoning_policy_build_workflow.rs` reads them back — each with the
//! document the start named and the instant each status of the build's progression —
//! `PREPROCESSING`, `BUILDING`, `TESTING` and then `COMPLETED` — is owed at, or the instant
//! `CANCELLED` is owed once a cancel has asked for it. Each of those records also holds the
//! ANNOTATION SET of its workflow — the corrections to the policy's rules, variables and
//! types the start carried, which `ops/get_automated_reasoning_policy_annotations.rs` reads
//! back with the hash of that set ([`annotations`]) — so the annotations one request
//! carried are the annotations every later read of that workflow reports.
//!
//! And it holds the Automated Reasoning policy tests
//! `ops/create_automated_reasoning_policy_test_case.rs` mints,
//! `ops/get_automated_reasoning_policy_test_case.rs` reads back and
//! `ops/delete_automated_reasoning_policy_test_case.rs` removes, each keyed by the policy it
//! was created under, so a test created by one request is the test every later request
//! sees — and a test one request deleted is a test no later request finds.
//!
//! And it holds the runs of those tests —
//! `ops/start_automated_reasoning_policy_test_workflow.rs` schedules them against a built
//! build workflow and `ops/get_automated_reasoning_policy_test_result.rs` reads each one
//! back — each keyed by the policy, the build workflow and the test it runs, with the
//! instant its non-terminal `testRunStatus` is owed its move to `COMPLETED`.
//!
//! And it holds the account's acknowledged use case for model access — the single form
//! `ops/put_use_case_for_model_access.rs` submits and `ops/get_use_case_for_model_access.rs`
//! reads back, so the form one request submits is the form the next request gets.
//!
//! And it holds the account's data retention setting — the single record
//! `ops/put_account_data_retention.rs` stores and `ops/get_account_data_retention.rs` reads
//! back, so the mode one request sets is the mode the next request reports.
//!
//! And it holds the application inference profiles `ops/create_inference_profile.rs`
//! mints, `ops/get_inference_profile.rs` reads back,
//! `ops/list_inference_profiles.rs` reports and `ops/delete_inference_profile.rs`
//! removes, each holding the foundation models that profile tracks metrics and costs for.
//!
//! And it holds the batch inference jobs `ops/create_model_invocation_job.rs` submits,
//! `ops/get_model_invocation_job.rs` reads back, `ops/list_model_invocation_jobs.rs`
//! reports and `ops/stop_model_invocation_job.rs` winds down, together with the instant
//! each job's non-terminal `status` is owed its next move and the instant the job would
//! time out. Each of those records also holds what the job's WORK established — the records
//! read out of its input object and the tally of the run that invoked the model over them
//! ([`model_invocation_run`]) — because the counts the two reads report and the results
//! stored under the job's output prefix are two sides of one run.
//!
//! And it holds the published foundation model catalog — the base models
//! `ops/list_foundation_models.rs` reports and `ops/get_foundation_model.rs` resolves one
//! row of — which is seeded with the store and never written, because no operation of this
//! service creates a foundation model. The cross-region inference profiles Amazon Bedrock
//! publishes over those models ([`system_defined_inference_profiles`], which
//! `ops/list_inference_profiles.rs` reports beside the account's own profiles) are
//! published the same way, so they are constants rather than part of the store at all.
//!
//! And it holds the resource policies `ops/put_resource_policy.rs` attaches,
//! `ops/get_resource_policy.rs` reports and `ops/delete_resource_policy.rs` takes away,
//! keyed by the [`TaggedResource`] each one is
//! attached to — one of the resources above, resolved out of the very same maps, so a
//! policy is attached to a RESOURCE rather than to one spelling of its ARN.
//!
//! Tagging adds NO store of its own: `ops/tag_resource.rs` writes, and
//! `ops/list_tags_for_resource.rs` reports, the `tags` list of the record that already
//! holds the resource a `resourceARN` names — the same list the create that minted the
//! resource filled — so there is only ever one copy of a resource's tags, whichever
//! operation reads them. [`TaggedResource`] is what a `resourceARN` resolves to, and
//! resolving it is what keeps every tagging operation addressing the same record.
//!
//! The record shapes, and the conversions between the `*PolicyConfig` shapes a
//! request carries and the `*Policy` shapes a response carries, live here rather
//! than in one operation's file because six operations share them (a file under
//! `src/ops/` is claimed by the build as an operation, so a shared helper cannot
//! live there).
//!
//! This module is a DIRECTORY: the store struct, the guardrail family, the quotas and the
//! shared helpers stay in this file, and the families large enough to read on their own —
//! the stored records of the model, endpoint and job resources ([`resource_records`]), the
//! Automated Reasoning policy family ([`automated_reasoning_policies`]), the foundation
//! model catalogue ([`foundation_models`]) and the store's own lookups
//! ([`store_access`]) — live in their own files beside it. Every one of them is
//! re-exported here, so each name is still reached as `crate::state::…` and the handlers
//! address ONE module.


use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

/// The INFERENCE plane's account-enforced guardrail: the one home of the enforcement an
/// installed configuration puts in force, which this control plane writes on the put that
/// installs it ([`enforced_guardrail_enforcement`]) and the Bedrock Runtime emulator's
/// invocation surfaces read.
use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::enforced_guardrail as runtime_guardrail;

/// The INFERENCE plane's registry of the guardrails it can APPLY: the one home of
/// the evaluable part of each guardrail revision, which this control plane writes on
/// every request that changes one ([`guardrail_publication`]) and the
/// `ApplyGuardrail` and Converse surfaces read.
use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::guardrail_registry as runtime_registry;

use crate::ops::types::{
    AdvancedPromptOptimizationInputConfig, AdvancedPromptOptimizationOutputConfig,
    AutomatedReasoningPolicyAnnotation,
    AutomatedReasoningPolicyDefinition, AutomatedReasoningPolicyTestCase,
    AutomatedReasoningPolicyTestResult, CustomModelDataSource,
    CustomModelDeploymentSummary,
    CapabilitySupport,
    BatchSupport,
    CustomModelSummary, CustomizationConfig, EndpointConfig, EvaluationConfig,
    ConverseSupport, FeaturesSupport,
    ModelInvocationJobInputDataConfig, ModelInvocationJobOutputDataConfig,
    ModelInvocationJobSummary,
    EvaluationInferenceConfig, EvaluationOutputDataConfig, FoundationModelDetails,
    EvaluationInferenceConfigSummary, EvaluationModelConfigSummary, EvaluationModelConfig,
    EvaluationPrecomputedRagSourceConfig, EvaluationRagConfigSummary, EvaluationSummary,
    EvaluatorModelConfig, KnowledgeBaseConfig, RAGConfig,
    FoundationModelLifecycle, FoundationModelSummary, GuardrailAutomatedReasoningPolicy,
    GuardrailAutomatedReasoningPolicyConfig,
    GuardrailContentFilter, GuardrailContentFilterConfig, GuardrailContentFiltersTier,
    GuardrailContentFiltersTierConfig, GuardrailContentPolicy, GuardrailContentPolicyConfig,
    GuardrailContextualGroundingFilter, GuardrailContextualGroundingFilterConfig,
    GuardrailContextualGroundingPolicy, GuardrailContextualGroundingPolicyConfig,
    GuardrailContextualGroundingTier, GuardrailContextualGroundingTierConfig,
    GuardrailCrossRegionConfig, GuardrailCrossRegionDetails, GuardrailManagedWords,
    GuardrailManagedWordsConfig, GuardrailPiiEntity, GuardrailPiiEntityConfig, GuardrailRegex,
    GuardrailRegexConfig, GuardrailSensitiveInformationPolicy,
    GuardrailSensitiveInformationPolicyConfig, GuardrailTopic, GuardrailTopicConfig,
    GuardrailTopicPolicy, GuardrailTopicPolicyConfig, GuardrailTopicsTier,
    GuardrailTopicsTierConfig, GuardrailWord, GuardrailWordConfig, GuardrailWordPolicy,
    GuardrailWordPolicyConfig, InferenceProfileModel, InferenceProfileSummary, LegalTerm,
    LoggingConfig,
    MarketplaceModelEndpoint,
    ModelConfiguration, ModelCopyJobSummary, ModelDataSource, ModelEnforcement, Offer,
    OutputDataConfig,
    PricingTerm, PromptRouterSummary, PromptRouterTargetModel,
    ProvisionedModelSummary, RoutingCriteria, SelectiveContentGuarding, SupportTerm, Tag,
    TermDetails, TrainingDataConfig,
    ValidationDataConfig, VpcConfig,
};

/// The WORK a batch inference job performs: the records it reads out of its input object,
/// the model invocation it performs over each one and the result records it writes under
/// its output prefix. It lives beside this module rather than in it because it reaches
/// another emulated service (object storage, and the inference surface), while the record
/// and its lifecycle live here.
pub mod model_invocation_run;

/// The WORK an automated evaluation job performs: the prompts it reads out of its
/// configured datasets, the inference it performs over each one, the metrics it scores that
/// inference with and the result records it writes under its output prefix. Beside this
/// module for the reason [`model_invocation_run`] is: it reaches another emulated service
/// (object storage, and the inference surface), while the record and its lifecycle live
/// here.
pub mod evaluation_run;

/// The prompt datasets an evaluation job runs over: the built-in ones a job names by name,
/// and the reading of a caller's own one out of object storage. Beside
/// [`evaluation_run`], which is its only caller, for the same reason.
pub mod evaluation_datasets;

/// Whether the Amazon S3 location a model source names satisfies the Amazon-managed-bucket
/// pre-condition the member carrying it states. Beside this module for the reason
/// [`model_invocation_run`] is: deciding it means asking another emulated service (object
/// storage) whose bucket a name is, while the records the decision guards live here.
pub mod model_source;

/// Whether the destinations a model invocation logging configuration names are ones Amazon
/// Bedrock could deliver into: the Amazon S3 buckets in the Region whose logging it
/// configures, and the CloudWatch Logs group with the role it would deliver under. Beside
/// this module for the reason [`model_source`] is: deciding it means asking the services that
/// OWN those resources (object storage, CloudWatch Logs, IAM), while the configuration the
/// decision guards lives here.
pub mod logging_destination;
pub use logging_destination::*;

/// The foundation-model catalogue this emulator publishes, and the renderings the model
/// and inference profile surfaces report it through. Here rather than in this file so one
/// file holds one concern; every name it declares is re-exported below, so a handler still
/// reaches it as `crate::state::…`.
mod foundation_models;

/// The lookups of [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState`] — the
/// inherent `impl` the handlers reach the store through, beside the struct it belongs to.
mod store_access;

/// The Automated Reasoning policy family: its quotas, statuses, spans, refusal sentences
/// and stored records.
mod automated_reasoning_policies;

/// The annotation set of an Automated Reasoning policy build workflow: its canonical
/// rendering and the `annotationSetHash` that rendering is digested into.
mod annotations;
pub use annotations::*;

/// The stored record of each model, endpoint and job resource, with the lifecycle its own
/// clock owes it.
mod resource_records;

/// The quotas, status values, settle spans and refusal sentences this service answers
/// with.
mod limits;
pub use limits::*;

/// The stored record of each guardrail, prompt router and inference profile a request
/// creates.
mod records;
pub use records::*;

/// The account-level configuration this service enforces on an inference request, and the
/// pass-role denials a create answers.
mod enforcement;
pub use enforcement::*;

/// The projection of a stored guardrail onto the policies the inference plane applies,
/// written across on every request that changes one.
mod guardrail_publication;
pub use guardrail_publication::*;

/// The store itself: the state struct a handler reaches through [`service_state`], and the
/// tag writes made through it.
mod store;
pub use store::*;

/// The ARN builders, the identifier readers and the record-to-response projections the
/// operations share, with the listing cursor every paginated listing pages with.
mod projections;
pub use projections::*;

pub use automated_reasoning_policies::*;
pub use foundation_models::*;
pub use resource_records::*;
