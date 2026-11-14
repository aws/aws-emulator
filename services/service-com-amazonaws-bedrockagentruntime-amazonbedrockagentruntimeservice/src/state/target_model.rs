// SPDX-License-Identifier: CC0-1.0

//! The MODEL a prompt optimization is asked to optimize for, and what this emulator can
//! honestly answer about it.
//!
//! `OptimizePrompt` is an inference surface: AWS runs a model over the submitted prompt and
//! streams back a rewritten one. NO LANGUAGE MODEL RUNS IN THIS EMULATOR, so this module
//! does the two things that CAN be done faithfully and nothing else:
//!
//! * it resolves `targetModelId` against the base-model catalogue the Bedrock Runtime data
//!   plane of this same emulator serves inference from, so the model a prompt is optimized
//!   for is one this emulator really publishes and an identifier naming no such model is
//!   refused. The catalogue is not copied here: it is read in the plane that owns it, so
//!   the two planes cannot disagree about which models exist;
//! * it words the text the stream carries, which SAYS that no model ran — the same
//!   sentence that plane answers every inference call with, so one emulator states it once
//!   — extended with facts about the request that was actually served. Nothing here
//!   rewrites, echoes or synthesises a prompt.

use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state as runtime_state;

/// Why a target model cannot be optimized for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRefusal {
    /// The catalogue is not on this engine, so nothing can be said about the model —
    /// answered with a modelled server error, never with a client fault.
    StoreUnavailable,
    /// The identifier names no model this emulator can optimize a prompt for, stated in the
    /// data plane's own words for that same identifier.
    Unusable(String),
}

/// The model `target_model_id` names, as the Bedrock Runtime data plane's catalogue holds
/// it.
///
/// A reference identifies a BASE MODEL this emulator publishes, spelled one of the two ways
/// AWS names one: the model id exactly as the catalogue spells it, or that model's own
/// published ARN in the region the request was signed for
/// (`arn:<partition>:bedrock:<region>::foundation-model/<id>` — a foundation model is
/// published by the service, so its ARN carries no account). Anything else names no model a
/// prompt can be optimized for and is [`ModelRefusal::Unusable`]:
///
/// * an ARN of an account-scoped resource (a custom model, a provisioned throughput, a
///   prompt), which this emulator holds none of;
/// * an ARN in another partition, region or account, which names a resource this request
///   cannot reach — an ARN identifies one resource in one place;
/// * a cross-region inference-profile id, which names a routing profile rather than the
///   base model this call optimizes for;
/// * a model past its end of life, which serves no inference at all, reported in the words
///   the data plane reports it in.
///
/// Both of `resolve_model_for_inference`'s rejections are `Unusable` here, because
/// `OptimizePrompt` declares no `ResourceNotFoundException` — its only client-fault error
/// for a model reference it cannot use is `ValidationException`.
///
/// READ-ONLY: the catalogue is published, not stored per account, so nothing is written.
pub fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    target_model_id: &str,
) -> Result<String, ModelRefusal> {
    let Some(runtime) = runtime_state::service_state(engine) else {
        return Err(ModelRefusal::StoreUnavailable);
    };
    let unreadable = || ModelRefusal::Unusable(invalid_identifier_message());
    let named = if target_model_id.starts_with("arn:") {
        match model_arn_resource(region, target_model_id) {
            Ok(model_id) if !model_id.is_empty() => model_id,
            Ok(_) => return Err(unreadable()),
            Err(ArnProblem::RegionOrPartition) => {
                return Err(ModelRefusal::Unusable(
                    INCORRECT_ARN_REGION_OR_PARTITION.to_string(),
                ))
            }
            Err(ArnProblem::Unreadable) => return Err(unreadable()),
        }
    } else {
        target_model_id
    };
    let guard = runtime.lock();
    match guard.resolve_model_for_inference(named) {
        // The catalogue spells the model the way the reference named it, so the reference
        // names that base model.
        Ok(entry) if entry.model_id == named => Ok(entry.model_id.clone()),
        // A reference that resolves to a model it does not SPELL is an inference-profile
        // id: it names a routing profile, not the base model.
        Ok(_) => Err(unreadable()),
        Err(runtime_state::ModelRejection::Invalid(message)
        | runtime_state::ModelRejection::NotFound(message)) => {
            Err(ModelRefusal::Unusable(invalid_model_id(&message)))
        }
    }
}

/// Why an ARN names no model this call can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnProblem {
    /// The ARN names another partition, or another region than the request was made in —
    /// an ARN identifies one resource in one place, and this call reaches the models of the
    /// region it was signed for.
    RegionOrPartition,
    /// The ARN is not one of a model published by this service at all: another service's,
    /// an account's own resource, or not a foundation model.
    Unreadable,
}

/// The sentence a model ARN of another region or partition is refused with, as AWS words it
/// for this error class.
const INCORRECT_ARN_REGION_OR_PARTITION: &str = "Invalid ARN format: Incorrect region/partition.";

/// The model id a `foundation-model` ARN names, when that ARN names a model of THIS service
/// in `region`.
///
/// Read through the ONE ARN parse this tree has (`service_common::arn`), so this service
/// cannot disagree with any other about what an ARN is. A foundation model is published by
/// the service rather than created in an account, so its ARN carries an EMPTY account
/// segment — `arn:<partition>:bedrock:<region>::foundation-model/<id>`, exactly as the
/// modelled `targetModelId` pattern spells it. The partition and the region are told apart
/// from the other fields because AWS words their refusal its own way
/// ([`INCORRECT_ARN_REGION_OR_PARTITION`]).
fn model_arn_resource<'reference>(
    region: &str,
    reference: &'reference str,
) -> Result<&'reference str, ArnProblem> {
    let parsed = ::service_common::arn::parse(reference).ok_or(ArnProblem::Unreadable)?;
    if parsed.partition != emulator_partition() || parsed.region != region {
        return Err(ArnProblem::RegionOrPartition);
    }
    if parsed.vendor != crate::state::ARN_NAMESPACE || !parsed.account.is_empty() {
        return Err(ArnProblem::Unreadable);
    }
    ::service_common::arn::resource_id_of_type(reference, FOUNDATION_MODEL_RESOURCE_TYPE)
        .ok_or(ArnProblem::Unreadable)
}

/// The resource type a foundation model's ARN addresses.
const FOUNDATION_MODEL_RESOURCE_TYPE: &str = "foundation-model";

/// The partition every ARN this emulator mints carries, read off one it mints — through the
/// same parse — rather than spelled here, so this check and the ARNs the emulator hands out
/// cannot disagree.
fn emulator_partition() -> String {
    let minted = ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        crate::state::DEFAULT_REGION,
        "x",
    );
    ::service_common::arn::parse(&minted)
        .map(|arn| arn.partition.to_string())
        .unwrap_or_default()
}

/// The sentence a reference this service cannot read as a model identifier is refused
/// with: what is wrong with the model id, stated after the sentence the model documents
/// for this operation's `ValidationException`, which is how AWS words a refusal of this
/// class ([`VALIDATION_FAILED`]).
fn invalid_identifier_message() -> String {
    invalid_model_id(runtime_state::INVALID_MODEL_IDENTIFIER_MESSAGE)
}

/// `detail` stated as the model-id failure it is, behind the sentence the model documents
/// for this operation's `ValidationException`.
fn invalid_model_id(detail: &str) -> String {
    format!("{VALIDATION_FAILED} Invalid model ID: {detail}")
}

/// The sentence the model documents for this operation's `ValidationException`
/// (`smithy_query(query="shape",shape="ValidationException")`), which AWS states first and
/// then says what was invalid.
const VALIDATION_FAILED: &str =
    "Input validation failed. Check your request parameters and retry the request.";

/// The resource an identity-based policy for a `bedrock:` action on a MODEL names: the
/// model the call is made against — the model a prompt is optimized for
/// (`bedrock:OptimizePrompt`), and the model an inline agent orchestrates with
/// (`bedrock:InvokeInlineAgent`, `crate::ops::invoke_inline_agent`).
///
/// An identifier that is already an ARN IS that resource; a bare model id names the
/// foundation model of that id in the region the caller signed for, built by the data
/// plane's own ARN helper so both planes name a model the same way. Built without reading
/// the catalogue, so a decision made with it is made before — and independently of —
/// whether the model exists.
pub fn model_resource_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target_model_id: &str,
) -> String {
    if target_model_id.starts_with("arn:") {
        return target_model_id.to_string();
    }
    runtime_state::foundation_model_arn(&crate::state::request_region(ctx), target_model_id)
}

/// The message the `analyzePromptEvent` of a served optimization carries: that the prompt
/// was not analysed by a model, because none ran.
pub fn analysis_notice(model_id: &str) -> String {
    format!(
        "{} {model_id} analyzed no prompt.",
        runtime_state::generation::EMULATED_COMPLETION_FLOOR
    )
}

/// The text the `optimizedPromptEvent` of a served optimization carries.
///
/// It states that no model ran, which model the prompt was submitted against, and how long
/// the submitted prompt was — facts about the request that was really served. The
/// submitted prompt itself is NOT part of it: a prompt handed back unchanged would read as
/// an optimization that had happened.
pub fn optimized_prompt_notice(model_id: &str, submitted_prompt: &str) -> String {
    format!(
        "{} {model_id} optimized no prompt: the prompt submitted for optimization carried \
         {} character(s), and no rewriting of it was generated.",
        runtime_state::generation::EMULATED_COMPLETION_FLOOR,
        submitted_prompt.chars().count()
    )
}
