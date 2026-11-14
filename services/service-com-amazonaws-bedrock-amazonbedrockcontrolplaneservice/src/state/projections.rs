// SPDX-License-Identifier: CC0-1.0

//! The ARN builders, the identifier readers and the record-to-response projections this
//! service's operations share, plus the cursor every paginated listing of this service
//! pages with.

use super::*;
mod cursor;
pub use cursor::*;

/// The ARN of a custom model deployment in `region`:
/// `arn:aws:bedrock:<region>:<account>:custom-model-deployment/<id>`, as
/// `CustomModelDeploymentArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model-deployment/[a-z0-9]{12}$`)
/// spells it.
#[must_use]
pub fn custom_model_deployment_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "bedrock",
        region,
        &format!("{CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE}/{id}"),
    )
}

/// The ARN resource type a custom model deployment is named by.
pub const CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE: &str = "custom-model-deployment";

/// What this service answers as its `ValidationException` message for a
/// `CustomModelDeploymentIdentifier` that is not the ARN of a deployment and resolves to
/// none.
///
/// RECORDED behaviour, not a modelled rule: the corpus `simparity-behavioral-ga-prod-v1`
/// records Amazon Bedrock answering 400 `ValidationException` with exactly this sentence
/// for a bare deployment name this account holds no deployment under. A CONSTANT,
/// interpolating no part of the request: it reports that the identifier is not usable, and
/// nothing about which name was asked for, so the message cannot be used to probe the
/// store.
pub const CUSTOM_MODEL_DEPLOYMENT_IDENTIFIER_INVALID: &str =
    "The provided custom model deployment identifier is invalid.";

/// A minted resource id: `^[a-z0-9]+$`, the store's mint `ordinal` then the sub-second
/// part of `now`, each in [`ID_HALF_WIDTH`] base-36 digits.
pub(super) fn minted_id(ordinal: u64, now: SystemTime) -> String {
    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos().into());
    format!(
        "{}{}",
        base36(ordinal, ID_HALF_WIDTH),
        base36(nanos, ID_HALF_WIDTH)
    )
}

/// A minted UUIDv4-shaped resource id: the lowercase hyphenated form
/// `AutomatedReasoningPolicyBuildWorkflowId` spells.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2` crate)
/// and hex-rendered (the `hex` crate), the way this repo's other services mint a
/// UUID-shaped id (see
/// `services/service-com-amazonaws-wafv2-awswaf-20190729/src/state.rs`): this store's mint
/// `ordinal`, so two ids of one store can never collide; `now` in nanoseconds; and the
/// OS-seeded randomness a `RandomState` draws, so an id is not a guessable sequence
/// number. RFC 4122 §4.4's version nibble and variant bits are then set, which is what the
/// `4` and the `[89ab]` in that pattern require. An id names a resource; a credential
/// would come from `ctx.credential_store()`, never here.
pub(super) fn minted_uuid(ordinal: u64, now: SystemTime) -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::collections::hash_map::RandomState;
    use ::std::hash::BuildHasher;

    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(ordinal).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(ordinal.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..]
    )
}

/// What `UpdateMarketplaceModelEndpoint`, `RegisterMarketplaceModelEndpoint` and
/// `DeregisterMarketplaceModelEndpoint` answer as the `ResourceNotFoundException` message
/// for a Bedrock Marketplace model endpoint ARN this account holds no endpoint for.
///
/// RECORDED behaviour, not a modelled sentence (the model's own doc for the error reads
/// "The specified resource Amazon Resource Name (ARN) was not found. …"): the corpus
/// `simparity-behavioral-ga-prod-v1` records Amazon Bedrock answering 404
/// `ResourceNotFoundException` with exactly this sentence for an `endpointArn` that names
/// no endpoint. A CONSTANT, interpolating no part of the request — it reports that the
/// resource is not there and nothing about which ARN was asked for, so the message cannot
/// be used to probe the store.
///
/// Scoped to the operations recorded answering `ResourceNotFoundException` for such an ARN
/// — the WRITES that address an endpoint by it. The same corpus records
/// `GetMarketplaceModelEndpoint` refusing the SAME ARN with 400 `ValidationException`
/// instead, so the code and the sentence are per-operation; and this service's guardrail
/// and prompt router handlers keep the sentences they already word, because no recording
/// here shows this one for those resources.
pub const MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE: &str = "Bedrock Resource not found.";

/// The endpoint NAME inside an `endpointArn`: the resource id of a
/// `.../endpoint/<name>` ARN, or the whole value when it is not one.
///
/// Built like [`guardrail_id_of`], and for the same reason: a member that carries a
/// resource's ARN is read back to the id the resource is named by.
pub fn marketplace_endpoint_name_of(endpoint_arn: &str) -> &str {
    match endpoint_arn.rsplit_once(":endpoint/") {
        Some((_, name)) => name,
        None => endpoint_arn,
    }
}

/// Every Bedrock Marketplace model this emulator's Amazon Bedrock publishes, by the HUB
/// CONTENT NAME the Amazon SageMaker public hub holds it under — the name a
/// `ModelSourceIdentifier` addresses a model by.
///
/// A PUBLISHED catalog, the way [`FOUNDATION_MODELS`] is: Amazon Bedrock Marketplace offers
/// the models of the AWS-managed `SageMakerPublicHub`, and no request creates one, so this
/// is the set a `modelSourceIdentifier` can name — nothing a caller adds to or removes
/// from. The names are the hub content names AWS publishes there, and the catalog is a
/// SUBSET of them, spanning several providers and name forms, because the rule — a name the
/// public hub does not publish names no model — not the catalog's size is what this
/// emulator has to get right.
///
/// In `name` order, so a reader sees the set rather than a history of additions.
pub const PUBLISHED_MARKETPLACE_MODELS: &[&str] = &[
    "deepseek-llm-r1-distill-qwen-7b",
    "huggingface-llm-falcon-7b-instruct-bf16",
    "huggingface-llm-gemma-7b-instruct",
    "huggingface-llm-mistral-7b-instruct",
    "meta-textgeneration-llama-2-7b-f",
    "meta-textgeneration-llama-3-70b-instruct",
    "meta-textgeneration-llama-3-8b-instruct",
];

/// The hub content NAME inside a `ModelSourceIdentifier`: the segment after the ARN's
/// `.../Model/`, up to the content VERSION the identifier may carry after it — or the whole
/// value when it carries no such segment.
///
/// `ModelSourceIdentifier`'s pattern is
/// `arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*`
/// (`smithy_query(query="shape",shape="ModelSourceIdentifier")`), and a SageMaker hub
/// content is addressed by `<hub>/<type>/<name>/<version>`, so what follows `Model/` is the
/// content's name and then its version. Built like [`marketplace_endpoint_name_of`]: a
/// member carrying a resource's ARN is read back to the name the resource is held under.
pub fn marketplace_model_source_name_of(model_source_identifier: &str) -> &str {
    let after_type = match model_source_identifier.rsplit_once("/Model/") {
        Some((_, rest)) => rest,
        None => model_source_identifier,
    };
    match after_type.split_once('/') {
        Some((name, _version)) => name,
        None => after_type,
    }
}

/// Whether the Amazon SageMaker public hub publishes a Bedrock Marketplace model under this
/// hub content name ([`PUBLISHED_MARKETPLACE_MODELS`]).
#[must_use]
pub fn is_published_marketplace_model(hub_content_name: &str) -> bool {
    PUBLISHED_MARKETPLACE_MODELS.contains(&hub_content_name)
}

/// What a request naming a Bedrock Marketplace model the public hub does not publish is
/// refused with: `Hub content with name <name> does not exist.`
///
/// RECORDED behaviour, not a modelled sentence: the corpus records Amazon Bedrock answering
/// `RegisterMarketplaceModelEndpoint` 404 `ResourceNotFoundException` with exactly this
/// sentence for a well-formed `modelSourceIdentifier` whose hub content name the public hub
/// holds nothing under — and it is the model source, not the endpoint, that such a request
/// is refused on. BUILT FROM THE REQUEST: it names the hub content the identifier named
/// ([`marketplace_model_source_name_of`]), so the rule is "name the content the identifier
/// names", not one recorded name.
pub fn marketplace_hub_content_not_found(hub_content_name: &str) -> String {
    format!("Hub content with name {hub_content_name} does not exist.")
}

/// What `GetMarketplaceModelEndpoint` answers as its `ValidationException` message for an
/// `endpointArn` this account holds no endpoint for: `Could not find endpoint "<name>".`
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST rather than a constant: the
/// corpus `simparity-behavioral-ga-prod-v1` records Amazon Bedrock naming the endpoint the
/// request asked for — the resource id of the ARN the member carried
/// ([`marketplace_endpoint_name_of`]) — in exactly this sentence, quoted and closed with a
/// full stop. So the rule is "name the endpoint the ARN names", not one recorded name.
pub fn marketplace_endpoint_not_found_for_read(endpoint_arn: &str) -> String {
    format!(
        "Could not find endpoint \"{}\".",
        marketplace_endpoint_name_of(endpoint_arn)
    )
}

/// The `guardrailId` inside a `guardrailIdentifier`: the bare id, or the resource
/// half of a `.../guardrail/<id>` ARN.
pub fn guardrail_id_of(identifier: &str) -> &str {
    match identifier.rsplit_once(":guardrail/") {
        Some((_, id)) => id,
        None => identifier,
    }
}

/// The ARN of a guardrail in `region`:
/// `arn:aws:bedrock:<region>:<account>:guardrail/<id>`.
pub fn guardrail_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("guardrail/{id}"))
}

/// The ARN of a prompt router in `region`:
/// `arn:aws:bedrock:<region>:<account>:default-prompt-router/<id>`.
///
/// `default-prompt-router` is the resource type `PromptRouterArn` admits — its pattern
/// is `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:default-prompt-router/[a-zA-Z0-9-:.]+$`
/// (`smithy_query(query="shape",shape="PromptRouterArn")`) — so every router this
/// emulator mints is addressable through the members that carry that shape, whatever
/// `PromptRouterType` the router itself reports. `TaggableResourcesArn` spells the same
/// resource `prompt-router/[a-z0-9]{12}`; `PromptRouterArn` is the shape every
/// prompt-router operation carries, so it decides the form, and a minted id is 12
/// base-36 digits so it satisfies the other spelling's id form too.
pub fn prompt_router_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("default-prompt-router/{id}"))
}

/// One stored prompt router as the `PromptRouterSummary` a listing carries.
///
/// Lives here rather than in `ops/list_prompt_routers.rs` because every member it carries
/// is also a member `GetPromptRouter` reports, and the two must agree about them: a listed
/// summary is the same router the read of that one router finds. It is also the shape the
/// published routers are built in ([`default_prompt_routers`]), so a caller-created router
/// and a published one are reported through ONE mapping.
#[must_use]
pub fn prompt_router_summary(router: &PromptRouter) -> PromptRouterSummary {
    PromptRouterSummary {
        prompt_router_name: router.name.clone(),
        routing_criteria: router.routing_criteria.clone(),
        description: router.description.clone(),
        created_at: Some(date_time(router.created_at)),
        updated_at: Some(date_time(router.updated_at)),
        prompt_router_arn: router.arn.clone(),
        models: router.models.clone(),
        fallback_model: router.fallback_model.clone(),
        status: router.status.to_string(),
        r#type: router.router_type.to_string(),
    }
}

/// The ARN of an application inference profile in `region`:
/// `arn:aws:bedrock:<region>:<account>:application-inference-profile/<id>`.
///
/// `application-inference-profile` is the resource type `InferenceProfileArn` admits for a
/// profile a caller created — its pattern is
/// `^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{0,20}):(|[0-9]{12}):(inference-profile|application-inference-profile)/[a-zA-Z0-9-:.]+$`
/// (`smithy_query(query="shape",shape="InferenceProfileSummary")`) — while
/// `inference-profile` is the type the system-defined profiles Amazon Bedrock itself
/// publishes carry, and no operation here mints one of those.
pub fn application_inference_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "bedrock",
        region,
        &format!("application-inference-profile/{id}"),
    )
}

/// The ARN of a Bedrock Marketplace model endpoint named `name` in `region`:
/// `arn:aws:sagemaker:<region>:<account>:endpoint/<name>`.
///
/// A Bedrock Marketplace model endpoint IS a SageMaker endpoint — `EndpointConfig`'s only
/// member is `sageMaker` (`smithy_query(query="shape",shape="EndpointConfig")`),
/// `SageMakerEndpoint` carries the instance count, instance type and execution role
/// SageMaker hosts it with, and `RegisterMarketplaceModelEndpoint` takes the ARN of an
/// endpoint that already exists — so the `endpointArn` every marketplace endpoint
/// operation carries is that SageMaker endpoint's ARN, whose resource id is the endpoint
/// name. `Arn` declares no pattern (`@length(max: 2048)` only), so nothing in the model
/// The `AutomatedReasoningPolicyArn` of the policy `id` names in `region`:
/// `arn:aws:bedrock:<region>:<account>:automated-reasoning-policy/<id>`, which is the
/// form `AutomatedReasoningPolicyArn` spells without its optional version suffix.
#[must_use]
pub fn automated_reasoning_policy_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("automated-reasoning-policy/{id}"),
    )
}

/// The versioned `AutomatedReasoningPolicyArn` of version `version` of the policy
/// `policy_arn` names: the base ARN with the `:<version>` suffix
/// `AutomatedReasoningPolicyArn` admits after it.
#[must_use]
pub fn automated_reasoning_policy_version_arn(policy_arn: &str, version: u32) -> String {
    format!("{policy_arn}:{version}")
}

/// An `AutomatedReasoningPolicyArn` split into the base ARN and the version its optional
/// `:<version>` suffix names.
///
/// The suffix is the LAST colon-separated field, and only when it spells a number the
/// shape admits there (`([1-9][0-9]{0,11})`); anything else is part of the base ARN, so an
/// ARN carrying no suffix is returned whole.
#[must_use]
pub fn automated_reasoning_policy_arn_parts(policy_arn: &str) -> (&str, Option<u32>) {
    match policy_arn.rsplit_once(':') {
        Some((base, suffix)) => match version_number(suffix) {
            Some(number) => (base, Some(number)),
            None => (policy_arn, None),
        },
        None => (policy_arn, None),
    }
}

/// The `AutomatedReasoningPolicyTestCase` a stored test is reported as.
///
/// ONE mapping, so the read of a single test and a listing of a policy's tests report the
/// same test the same way.
#[must_use]
pub fn automated_reasoning_policy_test_case(
    record: &AutomatedReasoningPolicyTestCaseRecord,
) -> AutomatedReasoningPolicyTestCase {
    AutomatedReasoningPolicyTestCase {
        test_case_id: record.id.clone(),
        guard_content: record.guard_content.clone(),
        query_content: record.query_content.clone(),
        expected_aggregated_findings_result: Some(
            record.expected_aggregated_findings_result.clone(),
        ),
        confidence_threshold: record.confidence_threshold,
        created_at: date_time(record.created_at),
        updated_at: date_time(record.updated_at),
    }
}

/// The `AutomatedReasoningPolicyTestResult` a stored run of a stored test is reported as.
///
/// ONE mapping, so the read of a single result and a listing of a build workflow's results
/// report the same run the same way. The test it ran is reported through
/// [`automated_reasoning_policy_test_case`], so a result carries exactly the test a read of
/// that test reports.
///
/// `testFindings`, `aggregatedTestFindingsResult` and `testRunResult` are reported only
/// once something has checked the test's content against the policy's logic. Nothing in
/// this emulator does, so a run reports its lifecycle — the status it holds and when it
/// last changed — and none of the three: every one of them is an OPTIONAL member
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyTestResult")`), so a result
/// that carries no finding is one the shape admits, and inventing a finding or a verdict
/// here would report an outcome no check produced.
#[must_use]
pub fn automated_reasoning_policy_test_result(
    run: &AutomatedReasoningPolicyTestRunRecord,
    test_case: &AutomatedReasoningPolicyTestCaseRecord,
) -> AutomatedReasoningPolicyTestResult {
    AutomatedReasoningPolicyTestResult {
        test_case: automated_reasoning_policy_test_case(test_case),
        policy_arn: run.policy_arn.clone(),
        test_run_status: run.status.to_string(),
        test_findings: None,
        test_run_result: None,
        aggregated_test_findings_result: None,
        updated_at: date_time(run.updated_at),
    }
}

/// The `definitionHash` of an Automated Reasoning policy definition: 128 digits of
/// `[0-9a-z]`, as `AutomatedReasoningPolicyHash` requires.
///
/// A function of the DEFINITION alone, so two policies whose definitions agree report one
/// hash and any change to a definition changes it — which is what the member documents it
/// for ("used as a concurrency token for creating policy versions"). The digest is SHA-512
/// hex, whose 128 lowercase hex digits are exactly the width and alphabet the shape
/// admits, taken over a canonical rendering of the definition rather than over the request
/// bytes, so the same definition written two ways hashes alike.
///
/// An ABSENT definition hashes as the empty definition the shape's own defaults spell: a
/// policy created without one still reports a hash, and reports the same one until a
/// definition is written to it.
#[must_use]
pub fn policy_definition_hash(definition: Option<&AutomatedReasoningPolicyDefinition>) -> String {
    use ::sha2::{Digest, Sha512};

    let canonical = canonical_policy_definition(definition).to_string();
    ::hex::encode(Sha512::digest(canonical.as_bytes()))
}

/// A policy definition as its CONTENT: the format version this service writes definitions
/// in ([`POLICY_DEFINITION_FORMAT_VERSION`]), and each of its three lists, an absent one
/// read as the empty list.
///
/// So a revision that holds NO definition, and one whose definition carries no types,
/// variables or rules, are one definition — which is what makes [`policy_definition_hash`]
/// a function of the definition's content alone, and what
/// `ops/export_automated_reasoning_policy_version.rs` exports, so the definition that
/// operation reports always hashes to the `definitionHash` the same revision reports.
///
/// `version` names the FORMAT the definition document is written in rather than anything
/// about the policy, so it is this service's own format version wherever a definition is
/// reported: a format token a request carried is a statement about the document that
/// request sent, not about the one read back, and this service writes exactly one format.
/// A definition therefore reads back in the format it is stored in whatever token reached
/// it, and two definitions differing only in that token are one definition to the hash.
#[must_use]
pub fn policy_definition_content(
    definition: Option<&AutomatedReasoningPolicyDefinition>,
) -> AutomatedReasoningPolicyDefinition {
    AutomatedReasoningPolicyDefinition {
        version: POLICY_DEFINITION_FORMAT_VERSION.to_string(),
        types: Some(
            definition
                .and_then(|definition| definition.types.clone())
                .unwrap_or_default(),
        ),
        variables: Some(
            definition
                .and_then(|definition| definition.variables.clone())
                .unwrap_or_default(),
        ),
        rules: Some(
            definition
                .and_then(|definition| definition.rules.clone())
                .unwrap_or_default(),
        ),
    }
}

/// A policy definition as the one JSON value [`policy_definition_hash`] digests: every
/// member of every element of [`policy_definition_content`] in a fixed order, so the
/// rendering depends on the definition's CONTENT and on nothing else.
pub(super) fn canonical_policy_definition(
    definition: Option<&AutomatedReasoningPolicyDefinition>,
) -> ::serde_json::Value {
    let content = policy_definition_content(definition);
    let types: Vec<::serde_json::Value> = content
        .types
        .iter()
        .flatten()
        .map(|declared| {
            ::serde_json::json!({
                "name": declared.name,
                "description": declared.description,
                "values": declared
                    .values
                    .iter()
                    .map(|value| ::serde_json::json!({
                        "value": value.value,
                        "description": value.description,
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let variables: Vec<::serde_json::Value> = content
        .variables
        .iter()
        .flatten()
        .map(|variable| {
            ::serde_json::json!({
                "name": variable.name,
                "type": variable.r#type,
                "description": variable.description,
            })
        })
        .collect();
    let rules: Vec<::serde_json::Value> = content
        .rules
        .iter()
        .flatten()
        .map(|rule| {
            ::serde_json::json!({
                "id": rule.id,
                "expression": rule.expression,
                "alternateExpression": rule.alternate_expression,
            })
        })
        .collect();
    ::serde_json::json!({
        "version": content.version,
        "types": types,
        "variables": variables,
        "rules": rules,
    })
}

/// narrows it further.
pub fn marketplace_model_endpoint_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("sagemaker", region, &format!("endpoint/{name}"))
}

/// The `MarketplaceModelEndpoint` a response reports for a stored endpoint.
///
/// The ARN of a model import job in `region`:
/// `arn:aws:bedrock:<region>:<account>:model-import-job/<id>`, as `ModelImportJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-import-job/[a-z0-9]{12}$`)
/// spells it.
pub fn model_import_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("model-import-job/{id}"))
}

/// The ARN of an imported model in `region`:
/// `arn:aws:bedrock:<region>:<account>:imported-model/<id>`, as `ImportedModelArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:imported-model/[a-z0-9]{12}$`)
/// spells it.
pub fn imported_model_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("imported-model/{id}"))
}

/// The ARN of an evaluation job in `region`:
/// `arn:aws:bedrock:<region>:<account>:evaluation-job/<id>`, as `EvaluationJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12}$`)
/// spells it.
pub fn evaluation_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("evaluation-job/{id}"))
}

/// The ARN of an advanced prompt optimization job in `region`:
/// `arn:aws:bedrock:<region>:<account>:advanced-prompt-optimization-job/<id>`, as
/// `AdvancedPromptOptimizationJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/[a-z0-9]{12}$`)
/// spells it.
pub fn advanced_prompt_optimization_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "bedrock",
        region,
        &format!("advanced-prompt-optimization-job/{id}"),
    )
}

/// The `EvaluationJobType` of a job configured by `config`.
///
/// `EvaluationConfig` is a union of `automated` ("an automated evaluation job that
/// computes metrics") and `human` ("an evaluation job that uses human workers")
/// (`smithy_query(query="shape",shape="EvaluationConfig")`), and `EvaluationJobType` is
/// `[Human, Automated]` — so the variant the request carried IS the job's type, and the
/// type is never read off the request's own wording.
///
/// Lives here rather than in one operation's file because `CreateEvaluationJob` derives it
/// and `GetEvaluationJob` reports it, so both must agree about it.
#[must_use]
pub fn evaluation_job_type_of(config: &EvaluationConfig) -> &'static str {
    match config {
        EvaluationConfig::Automated(_) => EVALUATION_JOB_TYPE_AUTOMATED,
        EvaluationConfig::Human(_) => EVALUATION_JOB_TYPE_HUMAN,
    }
}

/// The `EvaluationSummary` `ListEvaluationJobs` reports for a stored job.
///
/// Every member comes off the stored record, so a listing and a read of the same job agree:
/// the identity and status the job holds now, the task types its `datasetMetricConfigs`
/// name, and the models or RAG sources its `inferenceConfig` names — reported both in the
/// `inferenceConfigSummary` and, for the models, in the deprecated `modelIdentifiers` member
/// AWS still populates ("Inference identifiers should be retrieved from the
/// inferenceConfigSummary", `smithy_query(query="shape",shape="EvaluationSummary")`), since
/// a client reading either must see the same configuration.
#[must_use]
pub fn evaluation_summary(record: &EvaluationJobRecord) -> EvaluationSummary {
    let (bedrock_models, precomputed_sources) = match &record.inference_config {
        EvaluationInferenceConfig::Models(models) => models.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut bedrock, mut precomputed), model| {
                match model {
                    EvaluationModelConfig::bedrockModel(model) => {
                        bedrock.push(model.model_identifier.clone());
                    }
                    EvaluationModelConfig::precomputedInferenceSource(source) => {
                        precomputed.push(source.inference_source_identifier.clone());
                    }
                }
                (bedrock, precomputed)
            },
        ),
        EvaluationInferenceConfig::ragConfigs(_) => (Vec::new(), Vec::new()),
    };
    let (knowledge_bases, precomputed_rag) = match &record.inference_config {
        EvaluationInferenceConfig::ragConfigs(configs) => configs.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut bases, mut precomputed), config| {
                match config {
                    RAGConfig::knowledgeBaseConfig(KnowledgeBaseConfig::retrieveConfig(
                        retrieve,
                    )) => bases.push(retrieve.knowledge_base_id.clone()),
                    RAGConfig::knowledgeBaseConfig(
                        KnowledgeBaseConfig::retrieveAndGenerateConfig(generate),
                    ) => bases.extend(
                        generate
                            .knowledge_base_configuration
                            .as_ref()
                            .map(|configuration| configuration.knowledge_base_id.clone()),
                    ),
                    RAGConfig::precomputedRagSourceConfig(
                        EvaluationPrecomputedRagSourceConfig::retrieveSourceConfig(source),
                    ) => precomputed.push(source.rag_source_identifier.clone()),
                    RAGConfig::precomputedRagSourceConfig(
                        EvaluationPrecomputedRagSourceConfig::retrieveAndGenerateSourceConfig(
                            source,
                        ),
                    ) => precomputed.push(source.rag_source_identifier.clone()),
                }
                (bases, precomputed)
            },
        ),
        EvaluationInferenceConfig::Models(_) => (Vec::new(), Vec::new()),
    };
    let automated = match &record.evaluation_config {
        EvaluationConfig::Automated(automated) => Some(automated),
        EvaluationConfig::Human(_) => None,
    };
    // `EvaluatorModelIdentifiers` is `@length(0..1)`, as the one `bedrockEvaluatorModels`
    // entry a configuration may name is.
    let evaluator_models = |models: Option<&EvaluatorModelConfig>| {
        models
            .map(|config| {
                let EvaluatorModelConfig::bedrockEvaluatorModels(models) = config;
                models
                    .iter()
                    .map(|model| model.model_identifier.clone())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default()
    };
    EvaluationSummary {
        job_arn: record.arn.clone(),
        job_name: record.name.clone(),
        status: record.status.to_string(),
        creation_time: date_time(record.created_at),
        job_type: record.job_type.to_string(),
        evaluation_task_types: evaluation_task_types(&record.evaluation_config),
        model_identifiers: Some(bedrock_models.clone()),
        rag_identifiers: Some(knowledge_bases.clone()),
        evaluator_model_identifiers: Some(evaluator_models(
            automated.and_then(|automated| automated.evaluator_model_config.as_ref()),
        )),
        custom_metrics_evaluator_model_identifiers: Some(
            automated
                .and_then(|automated| automated.custom_metric_config.as_ref())
                .map(|config| {
                    config
                        .evaluator_model_config
                        .bedrock_evaluator_models
                        .iter()
                        .map(|model| model.model_identifier.clone())
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default(),
        ),
        inference_config_summary: Some(EvaluationInferenceConfigSummary {
            model_config_summary: Some(EvaluationModelConfigSummary {
                bedrock_model_identifiers: Some(bedrock_models),
                precomputed_inference_source_identifiers: Some(precomputed_sources),
            }),
            rag_config_summary: Some(EvaluationRagConfigSummary {
                bedrock_knowledge_base_identifiers: Some(knowledge_bases),
                precomputed_rag_source_identifiers: Some(precomputed_rag),
            }),
        }),
        application_type: record.application_type.clone(),
    }
}

/// The `evaluationTaskTypes` of a job: the `taskType` of each `datasetMetricConfigs` entry
/// its configuration carries, each reported ONCE and in the order the entries name them.
///
/// `EvaluationTaskTypes` is `@length(1..5)`, as `EvaluationDatasetMetricConfigs` is, and
/// both configuration variants carry that list — so the task types of a job are the task
/// types its entries configure, whether workers or this service compute its metrics.
#[must_use]
pub fn evaluation_task_types(config: &EvaluationConfig) -> Vec<String> {
    let entries = match config {
        EvaluationConfig::Automated(automated) => &automated.dataset_metric_configs,
        EvaluationConfig::Human(human) => &human.dataset_metric_configs,
    };
    let mut task_types: Vec<String> = Vec::new();
    for entry in entries {
        if !task_types.contains(&entry.task_type) {
            task_types.push(entry.task_type.clone());
        }
    }
    task_types
}

/// The ARN of a Provisioned Throughput in `region`:
/// `arn:aws:bedrock:<region>:<account>:provisioned-model/<id>`, as `ProvisionedModelArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}$`)
/// spells it.
pub fn provisioned_model_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("provisioned-model/{id}"))
}

/// The base model id inside a `ModelIdentifier`: the `modelId` of a
/// `.../foundation-model/<modelId>` ARN, or the whole value when it is not one.
///
/// `ModelIdentifier` admits a model ARN or a bare model id
/// (`smithy_query(query="operation",operation="CreateProvisionedModelThroughput")`), so
/// both spellings name the same base model — built like [`guardrail_id_of`], and for the
/// same reason.
#[must_use]
pub fn base_model_id_of(model_identifier: &str) -> &str {
    match model_identifier.rsplit_once(":foundation-model/") {
        Some((_, model_id)) => model_id,
        None => model_identifier,
    }
}

/// Whether `model` is offered with Provisioned Throughput, which is what decides whether
/// a Provisioned Throughput can be created over it.
///
/// `InferenceType` is `[ON_DEMAND, PROVISIONED]` and
/// [`FoundationModel::inference_types_supported`] reports the ones that model is offered
/// under, so this reads the catalog rather than any property of the request.
#[must_use]
pub fn supports_provisioned_throughput(model: &FoundationModel) -> bool {
    model
        .inference_types_supported
        .contains(&INFERENCE_TYPE_PROVISIONED)
}

/// The base models offered with Provisioned Throughput and NO commitment term.
///
/// Whether a purchase may omit `commitmentDuration` is a property of the MODEL, not of the
/// account: that member's own documentation sends a caller to "Supported Regions and models
/// for Provisioned Throughput" for "which base models support no commitment"
/// (`smithy_query(query="doc",shape="CreateProvisionedModelThroughputRequest",member="commitmentDuration")`,
/// which also states that custom models support every level of commitment). The base models
/// that page lists a no-commitment purchase for are the earlier generations — the Amazon
/// Titan family, the Cohere models, Mixtral and SDXL — so a later model is offered with
/// Provisioned Throughput only under a term, and a no-commitment purchase over one of those
/// is refused rather than drawn against the no-commitment model-unit quota
/// ([`granted_model_units`]).
///
/// Kept as a list of ids rather than a column of [`FOUNDATION_MODELS`] because it is a
/// different published table from the per-model attributes those rows carry. Every id here
/// is a row of that catalog whose `inferenceTypesSupported` carries
/// [`INFERENCE_TYPE_PROVISIONED`], which is what
/// [`supports_no_commitment_provisioned_throughput`] holds it to.
pub const NO_COMMITMENT_PROVISIONED_MODELS: &[&str] = &[
    "amazon.titan-embed-image-v1",
    "amazon.titan-embed-text-v2:0",
    "amazon.titan-image-generator-v1",
    "amazon.titan-text-lite-v1",
    "cohere.command-light-text-v14",
    "cohere.command-text-v14",
    "cohere.embed-english-v3",
    "mistral.mixtral-8x7b-instruct-v0:1",
    "stability.stable-diffusion-xl-v1",
];

/// Whether a Provisioned Throughput over `model` can be bought with NO commitment term
/// ([`NO_COMMITMENT_PROVISIONED_MODELS`]).
///
/// A model that is offered with no Provisioned Throughput at all is offered with none
/// without a commitment either, so this answers for the catalog row as a whole rather than
/// for the id alone.
#[must_use]
pub fn supports_no_commitment_provisioned_throughput(model: &FoundationModel) -> bool {
    supports_provisioned_throughput(model)
        && NO_COMMITMENT_PROVISIONED_MODELS.contains(&model.model_id)
}

/// The `ProvisionedModelSummary` a listing reports for a stored Provisioned Throughput.
///
/// Lives here rather than in `ops/list_provisioned_model_throughputs.rs` because every
/// member it carries is also a member `GetProvisionedModelThroughput` reports, and the two
/// must agree about them: a listed summary is the same record the read of that one
/// Provisioned Throughput finds.
pub fn provisioned_model_summary(
    record: &ProvisionedModelThroughputRecord,
) -> ProvisionedModelSummary {
    ProvisionedModelSummary {
        provisioned_model_name: record.name.clone(),
        provisioned_model_arn: record.arn.clone(),
        model_arn: record.model_arn.clone(),
        desired_model_arn: record.desired_model_arn.clone(),
        foundation_model_arn: record.foundation_model_arn.clone(),
        model_units: record.model_units,
        desired_model_units: record.desired_model_units,
        status: record.status.to_string(),
        commitment_duration: record.commitment_duration.clone(),
        commitment_expiration_time: record.commitment_expiration_time.map(date_time),
        creation_time: date_time(record.created_at),
        last_modified_time: date_time(record.last_modified_at),
    }
}

/// The `CustomModelDeploymentSummary` a listing reports for a stored custom model
/// deployment.
///
/// Lives here rather than in `ops/list_custom_model_deployments.rs` because every member it
/// carries is also a member `GetCustomModelDeployment` reports, and the two must agree
/// about them: a listed summary is the same record the read of that one deployment finds.
#[must_use]
pub fn custom_model_deployment_summary(
    record: &CustomModelDeploymentRecord,
) -> CustomModelDeploymentSummary {
    CustomModelDeploymentSummary {
        custom_model_deployment_arn: record.arn.clone(),
        custom_model_deployment_name: record.name.clone(),
        model_arn: record.model_arn.clone(),
        created_at: date_time(record.created_at),
        status: record.status.to_string(),
        last_updated_at: record.last_updated_at.map(date_time),
        // `failureMessage` is reported "if the deployment status is FAILED". Nothing here
        // makes an accepted deployment fail, so there is none to report.
        failure_message: None,
    }
}

/// The ARN resource type a custom model carries: the field `CustomModelArn` spells before
/// the base-model segment and the model's own id
/// (`smithy_query(query="operation",operation="GetCustomModel")`).
///
/// Stated once because two layers read it — the ARN this service builds for one, and the
/// test that tells an ARN from the bare NAME a `ModelIdentifier` also admits
/// ([`names_arn_of`]).
pub const CUSTOM_MODEL_RESOURCE_TYPE: &str = "custom-model";

/// The ARN resource type an imported model carries, as `ImportedModelArn` spells it before
/// the model's own id (`smithy_query(query="operation",operation="GetImportedModel")`).
pub const IMPORTED_MODEL_RESOURCE_TYPE: &str = "imported-model";

/// The ARN resource types an inference profile carries: the one a profile Amazon Bedrock
/// publishes is spelled with, and the one a profile this account created is, as
/// `InferenceProfileArn` spells them
/// (`^arn:…:(inference-profile|application-inference-profile)/[a-zA-Z0-9-:.]+$`).
pub const INFERENCE_PROFILE_RESOURCE_TYPE: &str = "inference-profile";

/// See [`INFERENCE_PROFILE_RESOURCE_TYPE`].
pub const APPLICATION_INFERENCE_PROFILE_RESOURCE_TYPE: &str = "application-inference-profile";

/// Whether `value` is an ARN of this service's own namespace naming a resource of
/// `resource_type`, rather than one of the other spellings a member admits.
///
/// The question a member whose `@pattern` admits an ARN **or** a bare name has to answer
/// before anything else: which of the two a caller sent. The parse is
/// `service_common::arn`'s one parse, so no two readings of "is this an ARN of ours"
/// disagree.
#[must_use]
pub fn names_arn_of(value: &str, resource_type: &str) -> bool {
    ::service_common::arn::parse_of(value, ARN_NAMESPACE)
        .is_some_and(|arn| arn.resource_type() == resource_type)
}

/// The ARN of a custom model in `region`:
/// `arn:aws:bedrock:<region>:<account>:custom-model/<base>/<id>`, as `CustomModelArn`
/// (`^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported|…)/[a-z0-9]{12}$`)
/// spells it.
///
/// `base` is the base-model segment that pattern carries before the id:
/// [`CUSTOM_MODEL_IMPORTED_BASE`] for a model whose artifacts were brought in, and the
/// base foundation model's `provider.model` name for one customized from a foundation
/// model.
pub fn custom_model_arn(region: &str, base: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("custom-model/{base}/{id}"))
}

/// The base-model segment a `CustomModelArn` embeds: the text between that ARN's
/// `custom-model/` resource type and the custom model's own id, which is either
/// [`CUSTOM_MODEL_IMPORTED_BASE`] or the id of the foundation model the custom model was
/// customized from, with the version qualifiers AWS appends to it.
///
/// `None` for a value that is not a `custom-model` ARN of this service's namespace at all;
/// such a value is refused by the member's own modelled constraints before any handler
/// sees it.
///
/// A TOTAL read of the value: the shared six-field ARN parse
/// (`service_common::arn::parse_of`, which answers `None` for anything that is not an ARN
/// of this service's namespace), then `strip_prefix` and `rsplit_once`, each answering
/// `None` rather than panicking. Nothing is indexed, sliced or unwrapped, so no value a
/// caller can send reaches a panic here.
///
/// Lives here because every operation that takes a custom model's ARN as a MODEL SOURCE
/// reads the same segment out of it and refuses the same way
/// ([`invalid_base_model_message`]): `ops/create_custom_model_deployment.rs` and
/// `ops/create_model_copy_job.rs` today.
#[must_use]
pub fn custom_model_base_segment(model_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(model_arn, ARN_NAMESPACE)?;
    let rest = arn.resource.strip_prefix("custom-model/")?;
    let (segment, id) = rest.rsplit_once('/')?;
    (!segment.is_empty() && !id.is_empty()).then_some(segment)
}

/// What this service answers as its `ValidationException` message for a `CustomModelArn`
/// whose base-model segment names no foundation model the published catalog carries.
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST rather than a constant: the
/// corpus records Amazon Bedrock naming the base model the submitted ARN embedded
/// ([`custom_model_base_segment`]), quoted, in exactly this sentence. So the rule is "name
/// the base model the ARN embeds", not one recorded model id.
#[must_use]
pub fn invalid_base_model_message(base_model_segment: &str) -> String {
    format!(
        "The foundation model '{base_model_segment}' for custom model is not a valid \
         foundation model"
    )
}

/// The ARN of a model copy job in `region`:
/// `arn:aws:bedrock:<region>:<account>:model-copy-job/<id>`, as `ModelCopyJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-copy-job/[a-z0-9]{12}$`)
/// spells it.
#[must_use]
pub fn model_copy_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("model-copy-job/{id}"))
}

/// One stored model copy job as the `ModelCopyJobSummary` a listing carries.
///
/// Lives here rather than in `ops/list_model_copy_jobs.rs` because every member it carries
/// is also a member `GetModelCopyJob` reports, and the two must agree about them: a listed
/// summary is the same job the read of that one job finds.
///
/// `failureMessage` is reported for a copy that FAILED; nothing here makes an accepted copy
/// fail, so there is none to report. `targetModelTags` admits zero entries, so a copy
/// carrying no tags reports the empty list rather than omitting the member.
#[must_use]
pub fn model_copy_job_summary(job: &ModelCopyJobRecord) -> ModelCopyJobSummary {
    ModelCopyJobSummary {
        job_arn: job.arn.clone(),
        status: job.status.to_string(),
        creation_time: date_time(job.created_at),
        target_model_arn: job.target_model_arn.clone(),
        target_model_name: Some(job.target_model_name.clone()),
        source_account_id: job.source_account_id.clone(),
        source_model_arn: job.source_model_arn.clone(),
        target_model_kms_key_arn: job.target_model_kms_key_arn.clone(),
        target_model_tags: Some(job.target_model_tags.clone()),
        failure_message: None,
        source_model_name: job.source_model_name.clone(),
    }
}

/// The ARN of a model customization job in `region`:
/// `arn:aws:bedrock:<region>:<account>:model-customization-job/<base model>/<id>`, as
/// `ModelCustomizationJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-customization-job/[a-z0-9-]{1,63}[.]{1}…/[a-z0-9]{12}$`)
/// spells it.
///
/// `base_model_id` is the base-model segment that pattern carries before the id — the
/// `modelId` of the catalog row the request's `baseModelIdentifier` resolved to, exactly as
/// the custom model the job produces carries it in [`custom_model_arn`].
pub fn model_customization_job_arn(region: &str, base_model_id: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "bedrock",
        region,
        &format!("model-customization-job/{base_model_id}/{id}"),
    )
}

/// The `CustomizationType` a create request's `customizationType` names, or
/// [`CUSTOMIZATION_TYPE_FINE_TUNING`] when it names none; `None` for a value the modelled
/// enum does not carry.
///
/// Resolved against `crate::ops::types::CUSTOMIZATION_TYPE_VALUES` — the enum's own value
/// list — so a stored record carries one of the modelled values rather than whatever text
/// arrived.
#[must_use]
pub fn customization_type_of(requested: Option<&str>) -> Option<&'static str> {
    let Some(requested) = requested else {
        return Some(CUSTOMIZATION_TYPE_FINE_TUNING);
    };
    crate::ops::types::CUSTOMIZATION_TYPE_VALUES
        .iter()
        .find(|value| **value == requested)
        .copied()
}

/// Whether `model` is published as supporting the customization `customization_type` names,
/// which is what decides whether a customization job can be created over it.
///
/// [`FoundationModel::customizations_supported`] is the `ModelCustomizationList`
/// `ListFoundationModels` reports for that model, each value out of `ModelCustomization`
/// (`crate::ops::types::MODEL_CUSTOMIZATION_VALUES`) — so this reads the published catalog
/// rather than any property of the request, exactly as
/// [`supports_provisioned_throughput`] does for a Provisioned Throughput.
#[must_use]
pub fn supports_customization(model: &FoundationModel, customization_type: &str) -> bool {
    model
        .customizations_supported
        .contains(&customization_type)
}

/// The ARN of a batch inference job in `region`:
/// `arn:aws:bedrock:<region>:<account>:model-invocation-job/<id>`, as
/// `ModelInvocationJobArn`
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-invocation-job/[a-z0-9]{12}$`)
/// spells it.
pub fn model_invocation_job_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("model-invocation-job/{id}"))
}

/// The `ModelInvocationType` a create request's `modelInvocationType` names; `None` for a
/// value the modelled enum does not carry.
///
/// Resolved against `crate::ops::types::MODEL_INVOCATION_TYPE_VALUES` — the enum's own
/// value list — so a stored record carries one of the modelled values rather than whatever
/// text arrived, exactly as [`customization_type_of`] does for a customization.
#[must_use]
pub fn model_invocation_type_of(requested: &str) -> Option<&'static str> {
    crate::ops::types::MODEL_INVOCATION_TYPE_VALUES
        .iter()
        .find(|value| **value == requested)
        .copied()
}

/// The `ModelInvocationJobSummary` a listing reports for a stored batch inference job.
///
/// Lives here rather than in `ops/list_model_invocation_jobs.rs` because every member it
/// carries is also a member `GetModelInvocationJob` reports, and the two must agree about
/// them: a listed summary is the same record the read of that one job finds.
#[must_use]
pub fn model_invocation_job_summary(record: &ModelInvocationJobRecord) -> ModelInvocationJobSummary {
    ModelInvocationJobSummary {
        job_arn: record.arn.clone(),
        job_name: record.name.clone(),
        model_id: record.model_id.clone(),
        client_request_token: record.client_request_token.clone(),
        role_arn: record.role_arn.clone(),
        status: Some(record.status.to_string()),
        // "If the batch inference job failed, this field contains a message describing why
        // the job failed": the sentence its validation or its run failed on
        // ([`ModelInvocationJobRecord::message`]), and nothing for a job that has not
        // failed.
        message: record.message.clone(),
        submit_time: date_time(record.submit_time),
        last_modified_time: Some(date_time(record.last_modified_at)),
        end_time: record.ended_at.map(date_time),
        input_data_config: record.input_data_config.clone(),
        output_data_config: record.output_data_config.clone(),
        vpc_config: record.vpc_config.clone(),
        timeout_duration_in_hours: record.timeout_duration_in_hours,
        job_expiration_time: record.job_expiration_time.map(date_time),
        model_invocation_type: Some(record.model_invocation_type.to_string()),
        // The record counts are counted BY the job's own work step: the records read out of
        // the input object the submission was pointed at ([`model_invocation_run`]) and the
        // tally of the run that invoked the model over them. A job nothing is known about
        // the input of has no count to report, which is the one case all four are absent in
        // — never a job that ran.
        total_record_count: record.total_record_count,
        processed_record_count: record
            .total_record_count
            .map(|_| record.processed_record_count),
        success_record_count: record
            .total_record_count
            .map(|_| record.success_record_count),
        error_record_count: record.total_record_count.map(|_| record.error_record_count),
    }
}

/// The `CustomModelSummary` a listing reports for a stored custom model.
///
/// Lives here rather than in `ops/list_custom_models.rs` because every member it carries
/// is also a member `GetCustomModel` reports, and the two must agree about them: a listed
/// summary is the same record the read of that one model finds.
pub fn custom_model_summary(record: &CustomModelRecord) -> CustomModelSummary {
    CustomModelSummary {
        model_arn: record.arn.clone(),
        model_name: record.name.clone(),
        creation_time: date_time(record.created_at),
        base_model_arn: record.base_model_arn.clone(),
        base_model_name: record.base_model_name.clone(),
        customization_type: Some(record.customization_type.to_string()),
        // "The unique identifier of the account that owns the model": every model this
        // store holds was created by the one account the emulator presents.
        owner_account_id: Some(::engine::emulator_account_id().to_string()),
        model_status: Some(record.status.to_string()),
    }
}

/// Lives here rather than in one operation's file because
/// `Create`/`Get`/`UpdateMarketplaceModelEndpoint` all report this shape, and they must
/// report it alike: what a create or update answers is exactly what the next read finds.
pub fn marketplace_model_endpoint_view(
    endpoint: &MarketplaceModelEndpointRecord,
) -> MarketplaceModelEndpoint {
    MarketplaceModelEndpoint {
        endpoint_arn: endpoint.arn.clone(),
        model_source_identifier: endpoint.model_source_identifier.clone(),
        status: Some(endpoint.status.to_string()),
        // `statusMessage` is "additional information about the overall status, if
        // available"; a registered endpoint has none to add.
        status_message: None,
        created_at: date_time(endpoint.created_at),
        updated_at: date_time(endpoint.updated_at),
        endpoint_config: endpoint.endpoint_config.clone(),
        endpoint_status: endpoint.endpoint_status.to_string(),
        // `endpointStatusMessage` likewise: no failure or detail to report for an endpoint
        // that is coming up or serving.
        endpoint_status_message: None,
    }
}

/// Whether `value` is a `responseQualityDifference` Amazon Bedrock accepts: a multiple
/// of [`RESPONSE_QUALITY_DIFFERENCE_STEP`].
///
/// The modelled `@range(0..100)` is enforced ahead of any handler, so this is only about
/// the granularity inside that span. An exact remainder test: the step and every multiple
/// of it below 100 are representable in binary floating point, so a value that is a
/// multiple is one exactly.
#[must_use]
pub fn is_accepted_response_quality_difference(value: f64) -> bool {
    value % RESPONSE_QUALITY_DIFFERENCE_STEP == 0.0
}

/// A `GuardrailVersion`/`GuardrailNumericalVersion` as a number, or `None` when it
/// is not a numbered version.
pub fn version_number(version: &str) -> Option<u32> {
    if version.is_empty() || version.starts_with('0') {
        return None;
    }
    version.parse::<u32>().ok()
}

/// The AWS region this request was signed for, defaulting to `us-east-1` for an
/// unsigned one, and folded back to the default when it is not region-shaped
/// (`GuardrailArn` admits `[a-z0-9-]{1,20}` there).
pub fn request_region(parts: &http::request::Parts) -> String {
    const DEFAULT_REGION: &str = "us-east-1";
    let region = ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region);
    match region {
        Some(region)
            if (1..=20).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
        {
            region
        }
        _ => DEFAULT_REGION.to_string(),
    }
}

// =============================================================================
// This service's own check of a caller-supplied ARN
// =============================================================================
//
// For SOME of its resources, Amazon Bedrock reads an ARN a caller submits ITSELF, ahead
// of the constraints the model declares for the member carrying it, and answers its own
// short sentence for one it will not accept — never the `N validation error(s) detected:
// Value '…' at '…' failed to satisfy constraint: …` envelope a constraint violation
// words. The two sentences below, and which resources they cover, are recorded in
// `simparity-wire-aws-v1`; which of them a request gets is decided here, and only here,
// so every member carrying an ARN of those resources answers alike.

/// This service's own ARN namespace: the third field of every ARN it owns.
///
/// `aws.api#service$arnNamespace` on this model is `bedrock`
/// (`smithy_query(query="service_info")`), so an ARN whose vendor field is another
/// service's — an IAM role, a KMS key, an S3 bucket — names no resource of this service
/// and is left to the constraints the model declares for it.
pub const ARN_NAMESPACE: &str = "bedrock";

/// What this service answers for an ARN of a resource it validates itself that names a
/// region other than the one the request addresses.
///
/// RECORDED behaviour, not a modelled rule: the model declares no per-region ARN check,
/// and the corpus records Amazon Bedrock answering 400 `ValidationException` with exactly
/// this sentence for an ARN of one of the resources in
/// [`SELF_VALIDATED_RESOURCE_TYPES`] from another region. A CONSTANT, interpolating no
/// part of the request: it reports that the ARN is not usable here, and nothing about
/// which member carried it or which expression that member declares.
pub const ARN_INVALID_FOR_SERVICE_REGION: &str =
    "The provided ARN is invalid for the service region";

/// What this service answers for a value submitted to a member whose modelled form admits
/// NOTHING BUT an ARN, when that value is not a well-formed ARN at all.
///
/// RECORDED behaviour: Amazon Bedrock reads such a member as an ARN itself, ahead of the
/// `@pattern` the model declares for it, and reports that the ARN cannot be read. A
/// CONSTANT, interpolating no part of the request: it carries neither the rejected value,
/// nor the member that carried it, nor the expression that member declares.
pub const ARN_INVALID: &str = "Provided ARN is invalid.";

/// The ARN resource types this service validates ITSELF — the ones the two sentences
/// above cover.
///
/// A CATALOG, read off the corpus, because nothing in the model marks the difference and
/// the corpus records BOTH answers for ARNs of the same shape and the same foreign
/// region:
///
/// * these six resources answer this service's own sentences —
///   `custom-model` (`CreateCustomModelDeployment.modelArn`,
///   `CreateModelCopyJob.sourceModelArn`), `model-copy-job` (`GetModelCopyJob.jobArn`),
///   `model-invocation-job` (`Get`/`StopModelInvocationJob.jobIdentifier`),
///   `provisioned-model` (`Get`/`Update`/`DeleteProvisionedModelThroughput
///   .provisionedModelId`), `model-customization-job`
///   (`Get`/`StopModelCustomizationJob.jobIdentifier`) and `model-import-job`
///   (`GetModelImportJob.jobIdentifier`), the last two recorded answering the region
///   sentence for a well-formed job ARN of another region where the lookup would
///   otherwise report the job missing;
/// * an `advanced-prompt-optimization-job` and an `evaluation-job` ARN from the SAME
///   foreign region, submitted by the same probe generator, are refused with the
///   framework's constraint envelope instead — recorded in
///   `issues/…-bedrock-validation-error-messages-render-model-regex-with.md`, which is
///   why this is a catalog of the resources bedrock checks itself rather than a rule over
///   every ARN. A resource type NOT listed here keeps the refusal the model's own
///   constraints word, which is what that recording shows.
///
/// Matched on the ARN's resource TYPE rather than on an operation or a member, so every
/// member of this service that carries one of these ARNs — the ones the recorded causes
/// name and the ones they do not (`GetCustomModel.modelIdentifier`,
/// `CreateProvisionedModelThroughput.modelId`, …) — is read the same way.
pub const SELF_VALIDATED_RESOURCE_TYPES: &[&str] = &[
    CUSTOM_MODEL_RESOURCE_TYPE,
    "model-copy-job",
    "model-customization-job",
    "model-import-job",
    "model-invocation-job",
    "provisioned-model",
];

/// Whether a declared `@pattern` spells the ARN of a resource this service validates
/// itself ([`SELF_VALIDATED_RESOURCE_TYPES`]).
///
/// Read off the expression rather than off a value, because it answers a question about
/// the MEMBER: which resource the member is declared to carry an ARN of. That is what
/// decides whether a value submitted to it is read as an ARN at all — an
/// `evaluation-job` identifier is spelled as an ARN too, and the corpus records the
/// framework's constraint envelope for a value outside its form, which is why this is the
/// same catalog and not "every ARN-valued member".
///
/// Matched on `:<type>/` — the resource field as every ARN pattern of this service spells
/// it, behind the account field's colon — so one resource type is never read as the prefix
/// of another (`:custom-model/` is not `:custom-model-deployment/`).
#[must_use]
pub fn pattern_names_self_validated_resource(pattern: &str) -> bool {
    SELF_VALIDATED_RESOURCE_TYPES
        .iter()
        .any(|resource_type| pattern.contains(&format!(":{resource_type}/")))
}

/// A caller-supplied ARN this service validates itself: one of its own namespace naming a
/// resource type in [`SELF_VALIDATED_RESOURCE_TYPES`], as the parsed ARN and the type it
/// names.
///
/// The parse is `service_common::arn`'s one parse — six fields, the first literally
/// `arn`, a non-empty partition, vendor and resource, the vendor compared exactly — so no
/// two services disagree about what an ARN is or which one is theirs.
pub(super) fn self_validated_arn(value: &str) -> Option<(::service_common::arn::Arn<'_>, &'static str)> {
    let arn = ::service_common::arn::parse_of(value, ARN_NAMESPACE)?;
    let resource_type = arn.resource_type();
    SELF_VALIDATED_RESOURCE_TYPES
        .iter()
        .find(|listed| **listed == resource_type)
        .map(|listed| (arn, *listed))
}

/// Whether `value` is an ARN of this service's own namespace naming a region other than
/// `region` — the region the request addresses, as
/// [`request_region`] reads it off the request's SigV4 credential scope.
///
/// Only an ARN of a resource this service validates itself
/// ([`SELF_VALIDATED_RESOURCE_TYPES`]), and only one whose region field is NOT empty: an
/// empty region names none to disagree with, which is how a partition-global resource is
/// written.
#[must_use]
pub fn arn_names_another_region(value: &str, region: &str) -> bool {
    self_validated_arn(value)
        .is_some_and(|(arn, _)| !arn.region.is_empty() && arn.region != region)
}

/// What this service answers for an ARN of its own namespace that is not a well-formed
/// ARN of the resource type IT names: `Invalid <resource type> ARN format`, the type
/// spelled in words.
///
/// The corpus records `Invalid custom model ARN format` for a `custom-model` ARN whose
/// resource id is not one `CustomModelArn` admits, so the sentence is built from the
/// resource type the ARN itself carries rather than from the member or the member's
/// expression — a `model-copy-job` ARN reports the model copy job.
///
/// `None` for an ARN of a resource this service does not validate itself, and for a value
/// that is no ARN of its namespace at all: those are values this check has nothing to say
/// about, so the refusal the model's own constraints word stands.
#[must_use]
pub fn invalid_arn_format_message(value: &str) -> Option<String> {
    let (_, resource_type) = self_validated_arn(value)?;
    // An ARN resource type is a hyphenated token (`custom-model`,
    // `model-invocation-job`); the recorded sentence spells it as words.
    let words = spelled_resource_type(resource_type);
    Some(format!("Invalid {words} ARN format"))
}

/// The principal that made this request, for the `createdBy` / `updatedBy` an enforced
/// guardrail configuration reports.
///
/// Derived from the request's OWN SigV4 access key id, so the two members report who
/// called rather than a fabricated identity; the emulator sees an access key id and no
/// role session, so the id is rendered as the IAM principal that holds it. That id is
/// caller-controlled text, so it is first reduced to the characters an access key id can
/// hold, and an unsigned request falls back to the account root — the member is a plain
/// `String` in the model, and this keeps it an ARN whatever the header said.
#[must_use]
pub fn caller_principal(parts: &http::request::Parts) -> String {
    let user_name: String = ::engine::extract_akid(&parts.headers)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    if user_name.is_empty() {
        ::engine::emulator_arn("iam", "", "root")
    } else {
        ::engine::emulator_arn("iam", "", &format!("user/{user_name}"))
    }
}

/// The `kmsKeyArn` a response reports for a request's `kmsKeyId`.
///
/// `kmsKeyId` admits a key ARN, a bare key id, or an alias, while `KmsKeyArn`
/// admits only `arn:aws:kms:<region>:<account>:key/<key id>`. A key ARN passes
/// through and a bare key id is completed into one; an ALIAS resolves to a key id
/// only inside KMS, so this reports no ARN for it rather than inventing one.
pub fn kms_key_arn(kms_key_id: Option<&str>, region: &str) -> Option<String> {
    let key_id = kms_key_id?;
    if key_id.starts_with("arn:") {
        return key_id.contains(":key/").then(|| key_id.to_string());
    }
    let is_key_id = key_id.len() == 36
        && key_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    is_key_id.then(|| ::engine::emulator_arn("kms", region, &format!("key/{key_id}")))
}

/// base-36 (`0-9a-z`) rendering of `value`, left-padded with `0` to `width`
/// digits.
pub(super) fn base36(mut value: u64, width: usize) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::with_capacity(width);
    loop {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    while out.len() < width {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ASCII")
}

/// How a policy block's members are reported, which both conversions below and the
/// response construction in `ops/get_guardrail.rs` follow:
///
/// * A LIST member whose `@length` admits zero entries is always PRESENT — the empty list
///   when the request configured none — because the wire form of "nothing configured" for
///   such a member is `[]`, not an absent key. A list whose `@length` has `min: 1`
///   (`GuardrailContentPolicy$filters`, `GuardrailWordPolicy$words`,
///   `GuardrailSensitiveInformationPolicy$piiEntities`,
///   `GuardrailContextualGroundingPolicy$filters`) cannot be reported empty, so it stays
///   absent until the request configures it.
/// * A TIER member is always present on a block that models one, carrying the tier the
///   request named or [`TIER_CLASSIC`] when it named none.
pub(super) fn topic_policy_of(config: &GuardrailTopicPolicyConfig) -> GuardrailTopicPolicy {
    GuardrailTopicPolicy {
        topics: config.topics_config.iter().map(topic_of).collect(),
        tier: Some(GuardrailTopicsTier {
            tier_name: config
                .tier_config
                .as_ref()
                .map_or_else(
                    || TIER_CLASSIC.to_string(),
                    |tier: &GuardrailTopicsTierConfig| tier.tier_name.clone(),
                ),
        }),
    }
}

pub(super) fn topic_of(config: &GuardrailTopicConfig) -> GuardrailTopic {
    GuardrailTopic {
        name: config.name.clone(),
        definition: config.definition.clone(),
        // `GuardrailTopicExamples` is `@length(min: 0)`, so a topic with no examples
        // carries the empty list.
        examples: Some(config.examples.clone().unwrap_or_default()),
        r#type: Some(config.r#type.clone()),
        input_action: config.input_action.clone(),
        output_action: config.output_action.clone(),
        input_enabled: config.input_enabled,
        output_enabled: config.output_enabled,
    }
}

pub(super) fn content_policy_of(config: &GuardrailContentPolicyConfig) -> GuardrailContentPolicy {
    GuardrailContentPolicy {
        filters: Some(
            config
                .filters_config
                .iter()
                .map(content_filter_of)
                .collect(),
        ),
        tier: Some(GuardrailContentFiltersTier {
            tier_name: config.tier_config.as_ref().map_or_else(
                || TIER_CLASSIC.to_string(),
                |tier: &GuardrailContentFiltersTierConfig| tier.tier_name.clone(),
            ),
        }),
    }
}

pub(super) fn content_filter_of(config: &GuardrailContentFilterConfig) -> GuardrailContentFilter {
    GuardrailContentFilter {
        r#type: config.r#type.clone(),
        input_strength: config.input_strength.clone(),
        output_strength: config.output_strength.clone(),
        input_modalities: config.input_modalities.clone(),
        output_modalities: config.output_modalities.clone(),
        input_action: config.input_action.clone(),
        output_action: config.output_action.clone(),
        input_enabled: config.input_enabled,
        output_enabled: config.output_enabled,
    }
}

pub(super) fn word_policy_of(config: &GuardrailWordPolicyConfig) -> GuardrailWordPolicy {
    GuardrailWordPolicy {
        words: config.words_config.as_ref().map(|words| {
            words
                .iter()
                .map(|word: &GuardrailWordConfig| GuardrailWord {
                    text: word.text.clone(),
                    input_action: word.input_action.clone(),
                    output_action: word.output_action.clone(),
                    input_enabled: word.input_enabled,
                    output_enabled: word.output_enabled,
                })
                .collect()
        }),
        managed_word_lists: Some(
            config
                .managed_word_lists_config
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|list: &GuardrailManagedWordsConfig| GuardrailManagedWords {
                    r#type: list.r#type.clone(),
                    input_action: list.input_action.clone(),
                    output_action: list.output_action.clone(),
                    input_enabled: list.input_enabled,
                    output_enabled: list.output_enabled,
                })
                .collect(),
        ),
    }
}

pub(super) fn sensitive_information_policy_of(
    config: &GuardrailSensitiveInformationPolicyConfig,
) -> GuardrailSensitiveInformationPolicy {
    GuardrailSensitiveInformationPolicy {
        pii_entities: config.pii_entities_config.as_ref().map(|entities| {
            entities
                .iter()
                .map(|entity: &GuardrailPiiEntityConfig| GuardrailPiiEntity {
                    r#type: entity.r#type.clone(),
                    action: entity.action.clone(),
                    input_action: entity.input_action.clone(),
                    output_action: entity.output_action.clone(),
                    input_enabled: entity.input_enabled,
                    output_enabled: entity.output_enabled,
                })
                .collect()
        }),
        regexes: Some(
            config
                .regexes_config
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|regex: &GuardrailRegexConfig| GuardrailRegex {
                    name: regex.name.clone(),
                    description: regex.description.clone(),
                    pattern: regex.pattern.clone(),
                    action: regex.action.clone(),
                    input_action: regex.input_action.clone(),
                    output_action: regex.output_action.clone(),
                    input_enabled: regex.input_enabled,
                    output_enabled: regex.output_enabled,
                })
                .collect(),
        ),
    }
}

pub(super) fn contextual_grounding_policy_of(
    config: &GuardrailContextualGroundingPolicyConfig,
) -> GuardrailContextualGroundingPolicy {
    GuardrailContextualGroundingPolicy {
        filters: config
            .filters_config
            .iter()
            .map(
                |filter: &GuardrailContextualGroundingFilterConfig| {
                    GuardrailContextualGroundingFilter {
                        r#type: filter.r#type.clone(),
                        threshold: filter.threshold,
                        action: filter.action.clone(),
                        enabled: filter.enabled,
                    }
                },
            )
            .collect(),
        tier: Some(GuardrailContextualGroundingTier {
            tier_name: config.tier_config.as_ref().map_or_else(
                || TIER_CLASSIC.to_string(),
                |tier: &GuardrailContextualGroundingTierConfig| tier.tier_name.clone(),
            ),
        }),
    }
}

pub(super) fn automated_reasoning_policy_of(
    config: &GuardrailAutomatedReasoningPolicyConfig,
) -> GuardrailAutomatedReasoningPolicy {
    GuardrailAutomatedReasoningPolicy {
        policies: config.policies.clone(),
        confidence_threshold: config.confidence_threshold,
    }
}

/// The cross-region details for a `guardrailProfileIdentifier`, which may be a
/// profile id or a profile ARN.
///
/// `GuardrailCrossRegionGuardrailProfileId` is `<prefix>.guardrail.v<n>`; an
/// identifier that is not one of those names no profile this emulator can report,
/// so it reports none rather than inventing a profile.
pub(super) fn cross_region_details_of(
    config: &GuardrailCrossRegionConfig,
    region: &str,
) -> Option<GuardrailCrossRegionDetails> {
    let identifier = config.guardrail_profile_identifier.as_str();
    let profile_id = match identifier.rsplit_once(":guardrail-profile/") {
        Some((_, id)) => id,
        None => identifier,
    };
    if !profile_id.contains(".guardrail.v") {
        return None;
    }
    Some(GuardrailCrossRegionDetails {
        guardrail_profile_id: Some(profile_id.to_string()),
        guardrail_profile_arn: Some(::engine::emulator_arn(
            "bedrock",
            region,
            &format!("guardrail-profile/{profile_id}"),
        )),
    })
}

// =============================================================================
// The published foundation model catalog
// =============================================================================

/// One stored application inference profile as the `InferenceProfileSummary` a listing
/// carries.
///
/// Lives here rather than in `ops/list_inference_profiles.rs` because every member it
/// carries is also a member `GetInferenceProfile` reports, and the two must agree about
/// them: a listed summary is the same profile the read of that one profile finds. It is
/// also the shape the published profiles are built in
/// ([`system_defined_inference_profiles`]), so a caller-created profile and a published one
/// are reported through ONE mapping.
#[must_use]
pub fn inference_profile_summary(profile: &InferenceProfileRecord) -> InferenceProfileSummary {
    InferenceProfileSummary {
        inference_profile_name: profile.name.clone(),
        description: profile.description.clone(),
        created_at: Some(date_time(profile.created_at)),
        updated_at: Some(date_time(profile.updated_at)),
        inference_profile_arn: profile.arn.clone(),
        models: profile.models.clone(),
        inference_profile_id: profile.id.clone(),
        status: profile.status.to_string(),
        r#type: profile.profile_type.to_string(),
    }
}

/// The system-defined inference profile Amazon Bedrock publishes in `region` under
/// `identifier`, if it publishes one.
///
/// `InferenceProfileIdentifier` admits the profile's own ARN or its bare
/// `inferenceProfileId` (`smithy_query(query="operation",operation="GetInferenceProfile")`),
/// so both spellings address the same published profile \u2014 resolved here the way
/// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::inference_profile_arn_of`]
/// resolves the two spellings of a profile a caller created. A published profile carries
/// the ARN of the region it is offered in, so one of another region names a profile of
/// another account-region.
#[must_use]
pub fn published_inference_profile(
    identifier: &str,
    region: &str,
) -> Option<InferenceProfileSummary> {
    system_defined_inference_profiles(region)
        .into_iter()
        .find(|published| {
            published.inference_profile_arn == identifier
                || published.inference_profile_id == identifier
        })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState>)
}
