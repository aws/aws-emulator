// SPDX-License-Identifier: CC0-1.0

//! The account-level configuration this service ENFORCES on an inference request — the
//! enforced guardrail configuration and the account's data retention — and the pass-role
//! denials a create answers when the role it is handed cannot be passed.

use super::*;

/// The account's enforced guardrail configuration: the guardrail revision every
/// invocation in this account is guarded by, as `PutEnforcedGuardrailConfiguration` last
/// installed it.
///
/// ONE record per account rather than a map, because the listing that reports it is
/// modelled `@length(min: 0, max: 1)`
/// (`smithy_query(query="operation",operation="ListEnforcedGuardrailsConfiguration")`):
/// an account holds at most one such configuration, so a further put REPLACES this record
/// instead of adding another.
///
/// The guardrail is stored as the id and the ARN of the guardrail the request's
/// `guardrailIdentifier` resolved to, rather than as the identifier's own spelling, so the
/// listing reports the same resource whichever spelling installed it.
#[derive(Debug, Clone)]
pub struct EnforcedGuardrailConfiguration {
    /// `configId` — the id this configuration is addressed by, minted by
    /// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::mint_enforced_guardrail_config_id`]
    /// when the first put installed it.
    pub config_id: String,
    /// `guardrailId` of the guardrail this configuration enforces.
    pub guardrail_id: String,
    /// `guardrailArn` of that same guardrail, as the stored guardrail carries it.
    pub guardrail_arn: String,
    /// `guardrailVersion` — the numbered version being enforced
    /// (`GuardrailNumericalVersion`, so never the DRAFT).
    pub guardrail_version: String,
    /// `selectiveContentGuarding`, as the put carried it (absent when it carried none).
    pub selective_content_guarding: Option<SelectiveContentGuarding>,
    /// `modelEnforcement`, as the put carried it (absent when it carried none).
    pub model_enforcement: Option<ModelEnforcement>,
    /// When the configuration was first installed; kept across a later put, which
    /// replaces the configuration rather than creating a new one.
    pub created_at: SystemTime,
    /// The principal that first installed it ([`caller_principal`]).
    pub created_by: String,
    /// When the last admitted put stored it.
    pub updated_at: SystemTime,
    /// The principal that made that put ([`caller_principal`]).
    pub updated_by: String,
}

/// `DataRetentionMode` — the account-wide mode in force while no put has set one.
///
/// The model documents this value as "The standard data handling for the model applies"
/// (`smithy_query(query="shape",shape="DataRetentionMode")`), which is the handling an
/// account that has never configured a mode gets. `inherit` is documented as "No data
/// retention mode is set at this scope", so it is a mode a caller ASKS for rather than the
/// standing answer for an account that asked for nothing.
pub const DATA_RETENTION_MODE_DEFAULT: &str = "default";

/// What `GetUseCaseForModelAccess` answers as its `ResourceNotFoundException` message while
/// the account has submitted no form
/// ([`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::use_case_for_model_access`]).
///
/// RECORDED behaviour: Amazon Bedrock tells the caller what to DO — fill the form out before
/// asking for access — in a wholly fixed sentence that carries nothing about the account, so
/// a wrapper surfacing the message verbatim shows the same guidance production does.
pub const USE_CASE_FOR_MODEL_ACCESS_NOT_SUBMITTED: &str =
    "You have not filled out the request form. Fill out the form before getting access.";

/// The account's data retention setting: the `mode` `PutAccountDataRetention` last stored,
/// and the instant that put stored it.
///
/// ONE record per account rather than a map, for the reason
/// [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::use_case_for_model_access`]
/// is one: the operations that write and read it carry no identifier at all, so there is a
/// single account-level setting rather than a resource a caller can hold more than one of,
/// and a further put REPLACES this record whole.
#[derive(Debug, Clone)]
pub struct AccountDataRetention {
    /// `mode` — one of `DataRetentionMode`'s values, as the admitted put carried it. Any
    /// other value is refused by the constraint walk before the handler runs, so what is
    /// stored here is always a modelled mode.
    pub mode: String,
    /// `updatedAt` — when the last admitted put stored this mode.
    pub updated_at: SystemTime,
}

/// What this control plane answers when a request's `configId` names no enforced guardrail
/// configuration this account holds — an id nothing minted, and any id at all while the
/// account holds no configuration — in the 404 `ResourceNotFoundException` both operations
/// addressed by that id model.
///
/// RECORDED behaviour, and a message with NO variable parts: Amazon Bedrock reports that no
/// configuration was found for the identifier the request carried without repeating that
/// identifier, so the wording is the whole requirement. ONE sentence for the whole class:
/// the resource and the error are the same whether the request addressing it is the delete
/// (`configId` in the path) or the put (`configId` in the body), so each states this
/// recorded sentence rather than a second wording of the same answer — and in particular
/// never one interpolating the submitted id, a shape no recording of this service's
/// not-found refusals carries.
pub const ENFORCED_GUARDRAIL_CONFIGURATION_NOT_FOUND: &str =
    "No default guardrail configuration found for the specified config identifier.";

/// Put the account's enforced guardrail configuration IN FORCE on the inference plane:
/// `config`, enforcing `revision`, becomes the guardrail every covered invocation of this
/// account is evaluated against. `None` withdraws whatever stood before it.
///
/// Answers whether the inference plane's store was reached: an emulator assembled without
/// the Bedrock Runtime data plane enforces nothing, and this control plane's own record of
/// the configuration is unaffected either way.
///
/// Call it with NO store of this service held, so two services' stores are never locked at
/// once.
pub fn enforce_guardrail_on_inference<Eng: Send + Sync + 'static>(
    engine: &Eng,
    installed: Option<(&EnforcedGuardrailConfiguration, &GuardrailRevision)>,
) -> bool {
    runtime_guardrail::install(
        engine,
        installed.map(|(config, revision)| enforced_guardrail_enforcement(config, revision)),
    )
}

/// The ENFORCEMENT an installed configuration is, as the inference plane applies it.
///
/// The configuration this control plane stores is the account's resource — its `configId`,
/// the principals and the instants a listing reports. What the INFERENCE plane needs is
/// something else: the policies to evaluate a request against. The two planes keep one
/// copy of that enforcement, in the inference plane's own store
/// (`PutEnforcedGuardrailConfiguration` installs it there through
/// [`enforce_guardrail_on_inference`]),
/// and this is the projection that fills it: the enforced revision's word policy and
/// blocked messaging, together with the model enforcement and selective-guarding modes the
/// put carried.
///
/// `revision` is the guardrail revision the configuration enforces — a NUMBERED version,
/// which is an immutable snapshot of the guardrail's policies, so the enforcement installed
/// from it stays the revision inference evaluates for as long as the configuration stands.
pub(super) fn enforced_guardrail_enforcement(
    config: &EnforcedGuardrailConfiguration,
    revision: &GuardrailRevision,
) -> runtime_guardrail::EnforcedGuardrail {
    let word_policy = revision.word_policy.as_ref();
    runtime_guardrail::EnforcedGuardrail {
        guardrail_id: config.guardrail_id.clone(),
        guardrail_version: config.guardrail_version.clone(),
        blocked_input_messaging: revision.blocked_input_messaging.clone(),
        blocked_outputs_messaging: revision.blocked_outputs_messaging.clone(),
        words: word_policy
            .and_then(|policy| policy.words.as_ref())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|word| {
                runtime_guardrail::BlockedWord::new(
                    word.text.clone(),
                    word.input_action.as_deref(),
                    word.input_enabled,
                    word.output_action.as_deref(),
                    word.output_enabled,
                )
            })
            .collect(),
        managed_word_lists: word_policy
            .and_then(|policy| policy.managed_word_lists.as_ref())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|list| {
                runtime_guardrail::ManagedWordList::new(
                    list.r#type.clone(),
                    list.input_action.as_deref(),
                    list.input_enabled,
                    list.output_action.as_deref(),
                    list.output_enabled,
                )
            })
            .collect(),
        // The content policy's filters, each with the strength it filters its
        // category at on each side: a guardrail blocks on ANY policy it
        // configures, so an enforcement installed from a revision that configures
        // content filters is evaluated against them as much as against its words.
        content_filters: revision
            .content_policy
            .as_ref()
            .and_then(|policy| policy.filters.as_ref())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|filter| {
                runtime_guardrail::ContentFilter::new(
                    filter.r#type.clone(),
                    filter.input_strength.clone(),
                    filter.input_action.as_deref(),
                    filter.input_enabled,
                    filter.output_strength.clone(),
                    filter.output_action.as_deref(),
                    filter.output_enabled,
                )
            })
            .collect(),
        // A configuration carrying no `modelEnforcement` is enforced on every model, which
        // the inference plane reads from an EMPTY included list; one that carries it names
        // the models to enforce on and the ones to exclude.
        included_models: config
            .model_enforcement
            .as_ref()
            .map(|enforcement| enforcement.included_models.clone())
            .unwrap_or_default(),
        excluded_models: config
            .model_enforcement
            .as_ref()
            .map(|enforcement| enforcement.excluded_models.clone())
            .unwrap_or_default(),
        system_guarded_selectively: config
            .selective_content_guarding
            .as_ref()
            .and_then(|guarding| guarding.system.as_deref())
            .is_some_and(|mode| mode == runtime_guardrail::GUARDING_SELECTIVE),
        messages_guarded_selectively: config
            .selective_content_guarding
            .as_ref()
            .and_then(|guarding| guarding.messages.as_deref())
            .is_some_and(|mode| mode == runtime_guardrail::GUARDING_SELECTIVE),
    }
}

/// Amazon Bedrock's answer when it is asked to take on a role the caller may not hand it:
/// the `iam:PassRole` the request would make is one this account does not hold.
///
/// The one sentence, with no variable part: the denial names neither the role nor the
/// destination, so there is nothing of the request to fill in. It is spelled the way the
/// rest of this tree records AWS spelling the same denial. It is answered with the modelled
/// `AccessDeniedException` (`@httpError(403)`).
pub const CROSS_ACCOUNT_PASS_ROLE_DENIED: &str = "Cross-account pass role is not allowed.";

/// Why Amazon Bedrock may not be passed the role a model invocation logging `config` names,
/// or `None` when every role it names may be passed — which is the ordinary case.
///
/// A destination that names a role the CALLER supplied needs an `iam:PassRole` step before
/// any log event can be written, and the only such destination is CloudWatch Logs: its
/// `roleArn` is `@required` and is the role Bedrock must take on to write into the log
/// group the same structure names
/// (`smithy_query(query="shape",shape="CloudWatchConfig")`). `S3Config` declares no role
/// member at all, so an S3-only configuration — and one naming no destination — passes no
/// role, has no `iam:PassRole` step to authorize, and is admitted; whether the account may
/// write to a named bucket is not a decision about a passed role.
///
/// The decision for the CloudWatch destination is taken over the SUBMITTED value, in three
/// parts, and a pass that no part refuses is made:
///
/// * the ACCOUNT's PASS-ROLE BOUNDARY over the role being passed
///   ([`crate::service::authorization::pass_role_is_withheld`]). Handing a service a role is
///   a permission of the CALLING PRINCIPAL rather than a property of the role: IAM evaluates
///   `iam:PassRole` on the role's own ARN against the policies in force on that principal,
///   and the account decides which of its roles each of its principals may hand out. This is
///   the part that is a property of the ACCOUNT rather than of the value: the corpus records
///   one account refusing a well-formed, same-account, correctly-trusted destination while
///   another installs the same shape of destination, which is the difference a pass-role
///   boundary makes and nothing about the role itself can express. A caller this gate cannot
///   ground — the account's own published credential, a principal holding a grant this
///   emulator cannot read, an engine with no IAM store — is left admitted, so the recorded
///   success path stands for a caller whose permissions this emulator holds nothing about;
/// * the ACCOUNT the value names. `iam:PassRole` is granted by the account that HOLDS the
///   role, so a principal of this account can hand Amazon Bedrock a role of its own account
///   and no role of another one, whatever that other role trusts. The account is read out of
///   the ARN by the crate's one IAM-ARN reading
///   ([`::service_common::iam_role::role_name`]), which answers the role's name only for a
///   role of THIS account and treats the EMPTY account field the member's published pattern
///   admits (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`) as this account, since a value
///   naming no account names no other one. The pattern itself is refused by the constraint
///   walk ahead of this gate, so a value that reaches here and reads as no role of this
///   account is one whose embedded 12-digit account is another's;
/// * the TRUST the role of this account carries, where this account's IAM HOLDS that role.
///   Amazon Bedrock takes the role on to write
///   into the log group beside it, which it can do only where that role's trust policy
///   admits [`BEDROCK_SERVICE_PRINCIPAL`] the `sts:AssumeRole` such a hand-off is
///   ([`::service_common::iam_role::held_role_is_unassumable`], the reading that judges a
///   caller-supplied role IAM ANSWERED about and stays silent about a role it holds nothing
///   under). This call STORES a trust
///   relationship for every later invocation to be logged under rather than authorizing one
///   named assume, so the trust is validated as a relationship: a statement's `Condition`
///   narrows which assumes it covers, not whether the role trusts the service, and the
///   confused-deputy clauses AWS documents on a logging role are over keys a single
///   invocation carries rather than values of this request.
///
/// A role this account's IAM holds NOTHING under is not judged here at all: it is a
/// destination that does not resolve, which this operation answers before this decision is
/// taken and with a different modelled error
/// ([`super::unvalidated_cloud_watch_destination`]), so the denial below is reserved for a
/// role that DOES resolve and still may not be handed over.
///
/// Refused only on what this account's IAM ANSWERED: a document it holds for the calling
/// principal that does not allow the `iam:PassRole` of that role, or an answer about the
/// role itself — the trust document it reported withholds the grant. An emulator with no IAM
/// to ask, a caller it holds no policies for, a role it holds nothing under, or an answer
/// that could not be read, leaves the pass admitted, because a refusal has to be grounded.
///
/// A denied pass is [`CROSS_ACCOUNT_PASS_ROLE_DENIED`] in the modelled
/// `AccessDeniedException` (`@httpError(403)`) with nothing installed; a pass this account
/// may make installs the configuration, so the CloudWatch Logs destination is reported back
/// by the get, cleared by the delete, and honoured where the configuration is enforced.
pub async fn pass_role_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config: &LoggingConfig,
) -> Option<String> {
    let role_arn = &config.cloud_watch_config.as_ref()?.role_arn;
    if ::service_common::iam_role::role_name(role_arn).is_none() {
        return Some(CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string());
    }
    if crate::service::authorization::pass_role_is_withheld(engine, ctx, role_arn) {
        return Some(CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string());
    }
    ::service_common::iam_role::held_role_is_unassumable(
        ctx,
        region,
        role_arn,
        BEDROCK_SERVICE_PRINCIPAL,
    )
    .await
    .then(|| CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string())
}

/// Why Amazon Bedrock may not be passed the role a JOB-creating request names, or `None`
/// when it may be passed.
///
/// Starting a job makes Amazon Bedrock TAKE ON the submitted role — to read the job's
/// inputs and to write the artifacts it produces — so `roleArn` names a role the service
/// assumes rather than a string the request stores, and the `iam:PassRole` step that
/// decides whether this caller may pass it stands in front of everything else the request
/// needs: no resource it names is resolved until that decision is taken. The operations
/// that make such a pass are the job creates whose `roleArn` is the role the job RUNS as —
/// `CreateModelCustomizationJob`, `CreateModelInvocationJob` and `CreateEvaluationJob`
/// (`smithy_query(query="operation",operation="CreateModelCustomizationJob,CreateModelInvocationJob,CreateEvaluationJob")`:
/// each declares `roleArn` `@required` under `RoleArn` and `AccessDeniedException` at
/// `@httpError(403)`) — and each takes this one decision rather than a copy of it.
///
/// The decision is the one every service that is handed a role to act under makes
/// ([`::service_common::iam_role::role_pass_is_unauthorized`]), asked for THIS pass:
///
/// * the value has to name a role of the CALLER's OWN account — `iam:PassRole` is granted by
///   the account that holds the role, so another account's role is one no principal of this
///   account can hand to a service, whatever that role trusts (an EMPTY account field, which
///   the member's published pattern admits,
///   `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`, names no other account, hence this one);
/// * this account's IAM has to HOLD that role; and
/// * the trust policy the role carries has to grant [`BEDROCK_SERVICE_PRINCIPAL`] the
///   `sts:AssumeRole` this pass would make. A statement's `Condition` is part of its grant,
///   so a grant this pass cannot be shown to satisfy is not one, and the pass is decided
///   under the context the assume itself carries: the account whose request Amazon Bedrock
///   would be acting for ([`SOURCE_ACCOUNT_CONDITION_KEY`]) and the job the role is taken on
///   FOR ([`SOURCE_ARN_CONDITION_KEY`]) — the two confused-deputy keys AWS documents a
///   Bedrock service role's trust policy with. `source_arn` is the ARN of the job the
///   request is creating, which the caller states where that ARN is FIXED at the point this
///   decision is taken: a job addressed by a minted id alone (`ModelInvocationJobArn` and
///   `EvaluationJobArn` are `…:<family>/[a-z0-9]{12}`) is minted out of the Region and a
///   fresh id, both held before anything the request names is resolved, so a trust statement
///   narrowed to that job family is DECIDED and admits the pass — which is what the corpus
///   records a batch-inference submission carrying exactly that trust policy getting.
/// * a clause over a key the pass carries no value for leaves the statement undecided, which
///   IAM never reads as a grant, and no value is guessed for it
///   (`::service_common::iam_authorization` decides a clause or leaves it undecided; it
///   never fills one in). A job whose ARN carries a segment this decision stands IN FRONT of
///   is in that position: `ModelCustomizationJobArn` spells the base model before the id,
///   and the base model is resolved only after this gate, so a customization pass states no
///   source ARN (`None`) and a grant narrowed by that key cannot be shown to cover it —
///   RECORDED, not derived from the documentation: the corpus records that operation
///   refusing a pass of exactly that shape, a role of the caller's own account, trusted to
///   this service, whose grant is narrowed to the customization-job family.
///
/// Refused only on what IAM ANSWERED: an emulator with no IAM to ask, or an answer this
/// could not read, leaves the pass admitted, because a refusal has to be grounded.
///
/// ONE sentence for every refusal this gate makes, [`CROSS_ACCOUNT_PASS_ROLE_DENIED`]: the
/// corpus records these operations refusing a pass with that sentence and with nothing else,
/// and it records it for a role of the caller's OWN account — so the recorded wording is not
/// reserved for the ARN naming another account, and no recording of them words a
/// refused pass any other way. Inventing a second sentence for the other paths of one
/// decision would be a wording no recording supports; so the gate states the recorded one,
/// which carries no part of the request to be wrong about.
///
/// A denied pass is the modelled `AccessDeniedException` (`@httpError(403)`) carrying
/// [`CROSS_ACCOUNT_PASS_ROLE_DENIED`], with no job and nothing else written, which is
/// what every recording of such a create shows.
pub async fn job_pass_role_denial(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    source_arn: Option<&str>,
) -> Option<String> {
    let mut context = vec![(
        SOURCE_ACCOUNT_CONDITION_KEY.to_string(),
        ::engine::emulator_account_id().to_string(),
    )];
    context.extend(
        source_arn.map(|arn| (SOURCE_ARN_CONDITION_KEY.to_string(), arn.to_string())),
    );
    ::service_common::iam_role::role_pass_is_unauthorized(
        ctx,
        region,
        role_arn,
        BEDROCK_SERVICE_PRINCIPAL,
        &context,
    )
    .await
    .then(|| CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string())
}

/// The principal Amazon Bedrock takes on a role a caller passes it AS.
///
/// This service's `aws.api#service$cloudTrailEventSource`
/// (`smithy_query(query="service_info")`), which is the service principal AWS names Amazon
/// Bedrock by in an IAM policy — the principal a role that Amazon Bedrock is to assume has
/// to trust.
pub const BEDROCK_SERVICE_PRINCIPAL: &str = "bedrock.amazonaws.com";

/// `aws:SourceAccount` — IAM's global condition key for the account whose request the
/// service is acting for, which is the one value an `iam:PassRole` decision of this account
/// carries: the account making the call is the account the service would act for.
pub(super) const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

/// `aws:SourceArn` — IAM's global condition key for the RESOURCE the service is acting for,
/// which for a job create is the job the passed role is taken on to run: the other
/// confused-deputy key AWS documents a Bedrock service role's trust policy with.
pub(super) const SOURCE_ARN_CONDITION_KEY: &str = "aws:SourceArn";
