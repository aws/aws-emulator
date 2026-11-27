// SPDX-License-Identifier: CC0-1.0

//! @op UpdateStack
//! IMPLEMENTED: mutates the SAME stored stack (same `StackId`) — replaces the template
//! unless `UsePreviousTemplate` is set, merges `Parameters` honouring
//! `UsePreviousValue`, applies the configuration members the request carries, sets
//! `LastUpdatedTime` and records the `UPDATE_IN_PROGRESS` event. The PROCESSED stage of
//! the template the stack now carries is DEPLOYED: its resources and its outputs are
//! re-evaluated and take effect when the update completes, so a caller reading mid-update
//! still sees the values in force, and the resources the new template adds, changes or
//! drops report their own events. A resource the new template declares anew is CREATED in
//! the service that owns it, and one it no longer declares is deleted there, exactly as on
//! a create (see [`crate::state::provisioning`]); a resource an owning service refuses
//! leaves the stack in `UPDATE_FAILED` with that service's own refusal as the reason, and
//! the update is then ROLLED BACK to the previous stack configuration — put back in the
//! services that own the resources it had already changed there — which lands the stack in
//! `UPDATE_ROLLBACK_COMPLETE`, or in `UPDATE_ROLLBACK_FAILED` where one of them refuses to
//! put a resource back. That rollback is owed however the update failed and however far it
//! got — an update whose very first resource the owning service refused changed nothing
//! anywhere, so its rollback asks those services for nothing and the stack still reaches
//! `UPDATE_ROLLBACK_COMPLETE` — including a
//! failure the clock makes due after this call has answered: an update held at a wait
//! condition whose `Timeout` runs out is rolled back the same way
//! ([`crate::state::settle_due_rollbacks`]). Only a caller that asked for the failed state
//! to be kept (`DisableRollback`) stops at `UPDATE_FAILED`, which is the status
//! `ContinueUpdateRollback` and `RollbackStack` act on. A
//! template that declares or INVOKES a transform the request has not acknowledged with
//! `CAPABILITY_AUTO_EXPAND` is refused with the modelled
//! `InsufficientCapabilitiesException`. A request whose `ClientRequestToken` an operation
//! on that stack already claimed is refused with the modelled
//! `TokenAlreadyExistsException`, before the stack is touched and whatever status it is in
//! — a claimed token is refused here whichever operation claimed it, including this
//! update's own: unlike a create, which is identified by the stack name it would mint and
//! so can answer that stack again, an update that already ran left the stack changed, and
//! this service has no record of the answer it gave to re-answer with.
//!
//! The template is PROCESSED before it is deployed, exactly as a create's is: an
//! `Fn::Transform` naming `AWS::Include` is the Amazon S3 object its `Location` names,
//! spliced into the template ([`crate::state::include`]). That expansion happens on THIS
//! operation, so the snippet the update deploys is the object in force now, and the
//! template compared with the one the stack is in force with is the EXPANDED document: a
//! snippet revised since the stack last deployed is a change this update carries out, and
//! only a request that would deploy the same expanded template and the same configuration
//! is refused with "No updates are to be performed." An include that cannot be read
//! changes nothing — the stack keeps the template, the parameters and the resources it has,
//! and the update fails carrying the transform's own reason.
//!
//! A submitted template is VALIDATED before the stack is touched, by the same pass a
//! create's body goes through ([`crate::state::template_validation`]): a body whose own
//! declarations CloudFormation cannot resolve is refused with HTTP 400 `ValidationError`
//! and the stack keeps the template it has. A `Rules` assertion the parameter values in
//! force do not satisfy is not such a refusal — it fails the update, which likewise leaves
//! the stack as it was.
//!
//! The STACK POLICY in force governs every resource action this update plans
//! ([`crate::state::stack_policy`]): the policy the stack carries, or the
//! `StackPolicyDuringUpdateBody` the request overrides it with for this one update (which is
//! never stored, so `GetStackPolicy` goes on reporting the stored document). An action the
//! policy does not permit fails that resource with the policy's own verdict as its reason
//! and rolls the stack back to `UPDATE_ROLLBACK_COMPLETE`, before anything about the stack
//! or its resources is changed — so a denied update leaves the template, the parameters and
//! every resource exactly as they were. `StackPolicyBody` replaces the stack's stored
//! policy, which `GetStackPolicy` reports back.
//!
//! An update that would REPLACE a resource whose physical name the template states itself is
//! refused the same way, and before anything is deployed
//! ([`crate::state::custom_named_replacement`]): a replacement is created beside the resource
//! it supersedes, so there is no name left for it to take while that resource still holds the
//! one the template asks for. That resource fails with CloudFormation's own reason — which
//! asks the caller to rename it — and the stack rolls back to `UPDATE_ROLLBACK_COMPLETE` with
//! the live resource untouched. An update that states a DIFFERENT name has said what the
//! replacement is to be called, and is carried out.
//!
//! The policy the stack ALREADY carries is also validated against the submitted template
//! before any of that ([`crate::state::stack_policy::unknown_logical_id`]): a stored
//! statement protecting a logical id the submitted template does not declare refuses the
//! call outright with HTTP 400 `ValidationError`, and the stack keeps its status, its events
//! and its policy.
//!
//! Members this slice stores but has nothing to act on: `ResourceTypes`.
//! `DisableValidation` turns off pre-deployment validation. `StackPolicyURL` and
//! `StackPolicyDuringUpdateURL` name those same two policies in Amazon S3: each object is
//! read from this emulator's own Amazon S3 and acted on exactly as the inline body it stands
//! in for ([`crate::state::stack_policy::fetch`]).
//!
//! A `TemplateURL` says WHERE the revised template is rather than what it means, so the
//! object it addresses is read from this emulator's own Amazon S3 and the update is applied
//! from it exactly as from an inline body ([`crate::state::template_url`]).
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateStack::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateStack`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Updates a stack as specified in the template. After the call completes successfully, the
//! stack update starts. You can check the status of the stack through the DescribeStacks
//! action. To get a copy of the template for an existing stack, you can use the GetTemplate
//! action. For more information about updating a stack and monitoring the progress of the
//! update, see Managing Amazon Web Services resources as a single unit with CloudFormation
//! stacks in the CloudFormation User Guide.
//!
//! ## Input
//!
//! * `Capabilities` (Capabilities) — In some cases, you must explicitly acknowledge that your
//!   stack template contains certain capabilities in order for CloudFormation to update the
//!   stack. CAPABILITY_IAM and CAPABILITY_NAMED_IAM Some stack templates might include
//!   resources that can affect permissions in your Amazon Web Services account, for example,
//!   by creating new IAM users. For those stacks, you must explicitly acknowledge this by
//!   specifying one of these capabilities. The following IAM resources require you to specify
//!   either the CAPABILITY_IAM or CAPABILITY_NAMED_IAM capability. If you have IAM resources,
//!   you can specify either capability. If you have IAM resources with custom names, you must
//!   specify CAPABILITY_NAMED_IAM. If …
//! * `ClientRequestToken` (ClientRequestToken, len 1..128, pattern
//!   ^[a-zA-Z0-9][-a-zA-Z0-9]*$) — A unique identifier for this UpdateStack request. Specify
//!   this token if you plan to retry requests so that CloudFormation knows that you're not
//!   attempting to update a stack with the same name. You might retry UpdateStack requests to
//!   ensure that CloudFormation successfully received them. All events triggered by a given
//!   stack operation are assigned the same client request token, which you can use to track
//!   operations. For example, if you execute a CreateStack operation with the token token1,
//!   then all the StackEvents generated by that operation will have ClientRequestToken set as
//!   token1. In the console, stack operations display the client request token on the Events
//!   tab. Stack operations that …
//! * `DeploymentConfig` (DeploymentConfig) — The deployment configuration for this stack
//!   operation, including the deployment mode.
//! * `DisableRollback` (DisableRollback) — Preserve the state of previously provisioned
//!   resources when an operation fails. Default: False
//! * `DisableValidation` (DisableValidation) — Set to true to disable pre-deployment
//!   validations in changeset or stack operations. Default: false
//! * `NotificationARNs` (NotificationARNs, len 0..5) — Amazon Simple Notification Service
//!   topic Amazon Resource Names (ARNs) that CloudFormation associates with the stack.
//!   Specify an empty list to remove all notification topics.
//! * `Parameters` (Parameters) — A list of Parameter structures that specify input parameters
//!   for the stack. For more information, see the Parameter data type.
//! * `ResourceTypes` (ResourceTypes) — Specifies which resource types you can work with, such
//!   as AWS::EC2::Instance or Custom::MyCustomInstance. If the list of resource types doesn't
//!   include a resource that you're updating, the stack update fails. By default,
//!   CloudFormation grants permissions to all resource types. IAM uses this parameter for
//!   CloudFormation-specific condition keys in IAM policies. For more information, see
//!   Control CloudFormation access with Identity and Access Management. Only one of the
//!   Capabilities and ResourceType parameters can be specified.
//! * `RetainExceptOnCreate` (RetainExceptOnCreate) — When set to true, newly created
//!   resources are deleted when the operation rolls back. This includes newly created
//!   resources marked with a deletion policy of Retain. Default: false
//! * `RoleARN` (RoleARN, len 20..2048) — The Amazon Resource Name (ARN) of an IAM role that
//!   CloudFormation assumes to update the stack. CloudFormation uses the role's credentials
//!   to make calls on your behalf. CloudFormation always uses this role for all future
//!   operations on the stack. Provided that users have permission to operate on the stack,
//!   CloudFormation uses this role even if the users don't have permission to pass it. Ensure
//!   that the role grants least privilege. If you don't specify a value, CloudFormation uses
//!   the role that was previously associated with the stack. If no role is available,
//!   CloudFormation uses a temporary session that is generated from your user credentials.
//! * `RollbackConfiguration` (RollbackConfiguration) — The rollback triggers for
//!   CloudFormation to monitor during stack creation and updating operations, and for the
//!   specified monitoring period afterwards.
//! * `StackName` (StackName) **required** — The name or unique stack ID of the stack to
//!   update.
//! * `StackPolicyBody` (StackPolicyBody, len 1..16384) — Structure that contains a new stack
//!   policy body. You can specify either the StackPolicyBody or the StackPolicyURL parameter,
//!   but not both. You might update the stack policy, for example, in order to protect a new
//!   resource that you created during a stack update. If you don't specify a stack policy,
//!   the current policy that is associated with the stack is unchanged.
//! * `StackPolicyDuringUpdateBody` (StackPolicyDuringUpdateBody, len 1..16384) — Structure
//!   that contains the temporary overriding stack policy body. You can specify either the
//!   StackPolicyDuringUpdateBody or the StackPolicyDuringUpdateURL parameter, but not both.
//!   If you want to update protected resources, specify a temporary overriding stack policy
//!   during this update. If you don't specify a stack policy, the current policy that is
//!   associated with the stack will be used.
//! * `StackPolicyDuringUpdateURL` (StackPolicyDuringUpdateURL, len 1..5120) — Location of a
//!   file that contains the temporary overriding stack policy. The URL must point to a policy
//!   (max size: 16KB) located in an S3 bucket in the same Region as the stack. The location
//!   for an Amazon S3 bucket must start with https://. URLs from S3 static websites are not
//!   supported. You can specify either the StackPolicyDuringUpdateBody or the
//!   StackPolicyDuringUpdateURL parameter, but not both. If you want to update protected
//!   resources, specify a temporary overriding stack policy during this update. If you don't
//!   specify a stack policy, the current policy that is associated with the stack will be
//!   used.
//! * `StackPolicyURL` (StackPolicyURL, len 1..5120) — Location of a file that contains the
//!   updated stack policy. The URL must point to a policy (max size: 16KB) located in an S3
//!   bucket in the same Region as the stack. The location for an Amazon S3 bucket must start
//!   with https://. URLs from S3 static websites are not supported. You can specify either
//!   the StackPolicyBody or the StackPolicyURL parameter, but not both. You might update the
//!   stack policy, for example, in order to protect a new resource that you created during a
//!   stack update. If you don't specify a stack policy, the current policy that is associated
//!   with the stack is unchanged.
//! * `Tags` (Tags, len 0..50) — Key-value pairs to associate with this stack. CloudFormation
//!   also propagates these tags to supported resources in the stack. You can specify a
//!   maximum number of 50 tags. If you don't specify this parameter, CloudFormation doesn't
//!   modify the stack's tags. If you specify an empty value, CloudFormation removes all
//!   associated tags.
//! * `TemplateBody` (TemplateBody, len 1..) — Structure that contains the template body with
//!   a minimum length of 1 byte and a maximum length of 51,200 bytes. Conditional: You must
//!   specify only one of the following parameters: TemplateBody, TemplateURL, or set the
//!   UsePreviousTemplate to true.
//! * `TemplateURL` (TemplateURL, len 1..5120) — The URL of a file that contains the template
//!   body. The URL must point to a template that's located in an Amazon S3 bucket or a
//!   Systems Manager document. The location for an Amazon S3 bucket must start with https://.
//!   Conditional: You must specify only one of the following parameters: TemplateBody,
//!   TemplateURL, or set the UsePreviousTemplate to true.
//! * `UsePreviousTemplate` (UsePreviousTemplate) — Reuse the existing template that is
//!   associated with the stack that you are updating. When using templates with the
//!   AWS::LanguageExtensions transform, provide the template instead of using
//!   UsePreviousTemplate to ensure new parameter values and Systems Manager parameter updates
//!   are applied correctly. For more information, see AWS::LanguageExtensions transform.
//!   Conditional: You must specify only one of the following parameters: TemplateBody,
//!   TemplateURL, or set the UsePreviousTemplate to true.
//!
//! ## Output
//!
//! * `OperationId` (OperationId) — A unique identifier for this update operation that can be
//!   used to track the operation's progress and events.
//! * `StackId` (StackId) — Unique identifier of the stack.
//!
//! ## Errors
//!
//! * `InsufficientCapabilitiesException` (HTTP 400) — The template contains resources with
//!   capabilities that weren't specified in the Capabilities parameter.
//! * `TokenAlreadyExistsException` (HTTP 400) — A client request token already exists.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    status, StoredDeploymentConfig, StoredParameter, StoredRollbackConfiguration, StoredStack,
    StoredTag,
};

/// The stored values this update is resolved against: every member the merge below reads
/// and the no-op check compares.
///
/// Cloned field by field rather than cloning the whole `StoredStack`, so an update never
/// copies the stack's event log.
struct PreviousValues {
    template_body: Option<String>,
    processed_template: crate::state::template::Processed,
    /// The body that stage was computed over — the stored body with any `AWS::Include`
    /// snippet already spliced in ([`StoredStack::included_template_body`]).
    included_template_body: Option<String>,
    parameters: Vec<StoredParameter>,
    tags: Vec<StoredTag>,
    notification_arns: Vec<String>,
    capabilities: Vec<String>,
    role_arn: Option<String>,
    rollback_configuration: Option<StoredRollbackConfiguration>,
    deployment_config: Option<StoredDeploymentConfig>,
    /// The stack policy in force when this update arrived: what governs the resource
    /// actions it plans, unless the request overrides it for this one update.
    stack_policy_body: Option<String>,
    /// The identity the stack's own pseudo parameters resolve to, and the resources and
    /// imports its deployed template is in force with: what the planned resource actions
    /// are computed against ([`crate::state::change_plan`]).
    region: String,
    stack_name: String,
    stack_id: String,
    deployed: Vec<crate::state::DeployedResource>,
    imports: ::std::collections::BTreeMap<String, String>,
    /// The status the stack's last operation left it in: what decides whether this update
    /// has to ask for the failed state to be preserved
    /// ([`crate::state::fix_forward_refusal`]), whether resubmitting the same configuration
    /// is a no-op at all, and which of its resources carry the configuration the template in
    /// force declares ([`crate::state::change_plan`]).
    status: String,
}

impl PreviousValues {
    fn of(stack: &StoredStack) -> Self {
        Self {
            template_body: stack.template_body.clone(),
            processed_template: stack.processed_template.clone(),
            included_template_body: stack.included_template_body().map(str::to_string),
            parameters: stack.parameters.clone(),
            tags: stack.tags.clone(),
            notification_arns: stack.notification_arns.clone(),
            capabilities: stack.capabilities.clone(),
            role_arn: stack.role_arn.clone(),
            rollback_configuration: stack.rollback_configuration.clone(),
            deployment_config: stack.deployment_config.clone(),
            status: stack.status.clone(),
            stack_policy_body: stack.stack_policy_body.clone(),
            region: stack.region.clone(),
            stack_name: stack.stack_name.clone(),
            stack_id: stack.stack_id.clone(),
            deployed: stack.resources.clone(),
            imports: stack.imports.clone(),
        }
    }
}

/// What THIS update asks for about the rollback of its own failure, or `None` where it asks
/// for nothing — the setting in force on the stack then stands.
///
/// The two members that state it, read as one ([`crate::state::rollback_disabled`]), and
/// then the default of the mode the update runs in
/// ([`crate::state::mode_disables_rollback`]): a stated flag is what the request asked for
/// whichever mode it names, and only an update that states neither is answered by the mode.
fn disables_rollback(input: &UpdateStackInput) -> Option<bool> {
    crate::state::rollback_disabled(input.disable_rollback, input.deployment_config.as_ref())
        .or_else(|| {
            crate::state::mode_disables_rollback(
                input
                    .deployment_config
                    .as_ref()
                    .and_then(|config| config.mode.as_deref()),
            )
            .then_some(true)
        })
}

/// The logical ids the template this update submits declares: what the stored stack policy
/// is validated against ([`crate::state::stack_policy::unknown_logical_id`]).
///
/// Read off the stage the update would DEPLOY — the same document the plan below is computed
/// from — so a resource a transform's expansion contributes is a logical id the template
/// references, and a template this service cannot process declares none here (the
/// deployment's own failure is what reports that).
fn submitted_logical_ids(
    submitted_body: &str,
    submitted_stage: &crate::state::template::Processed,
) -> Vec<String> {
    submitted_stage
        .body(submitted_body)
        .and_then(crate::state::declared_template)
        .map(|template| {
            template
                .resources
                .iter()
                .map(|resource| resource.logical_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The resource-level plan this update would carry out: the comparison of the revision in
/// force on the stack with the one this update would deploy, which is the same comparison
/// `CreateChangeSet` reviews an update through ([`crate::state::changes_against`]).
///
/// Read off the stage the update would DEPLOY, over the body that stage was computed over, so
/// a stack whose template spliced in an `AWS::Include` snippet is compared with the document
/// it really deployed. A template this service could not process has no plan at all: the
/// deployment's own failure is what reports that.
fn planned_changes(
    previous: &PreviousValues,
    submitted_body: &str,
    submitted_stage: &crate::state::template::Processed,
    parameters: &[StoredParameter],
) -> Vec<crate::state::StoredResourceChange> {
    let Some(submitted) = submitted_stage
        .body(submitted_body)
        .and_then(crate::state::declared_template)
    else {
        return Vec::new();
    };
    crate::state::changes_against(
        Some(crate::state::DeployedStack {
            region: &previous.region,
            stack_name: &previous.stack_name,
            stack_id: &previous.stack_id,
            body: previous.included_template_body.as_deref(),
            processed: &previous.processed_template,
            parameters: &previous.parameters,
            deployed: &previous.deployed,
            status: &previous.status,
            imports: &previous.imports,
            notification_arns: &previous.notification_arns,
        }),
        &submitted,
        parameters,
    )
}

/// The verdict that refuses this update before it deploys anything, or `None` where none
/// does ([`crate::state::RefusedUpdate`]).
///
/// Computed from the resource-level plan [`planned_changes`] answers for the two revisions —
/// the one the stack is in force with and the one this update would deploy — so every verdict
/// is reached against the actions the deployment really performs, and from the resources the
/// stack has right now, which is what a replacement's name would collide with.
fn refused_update(
    previous: &PreviousValues,
    governing_policy: Option<&str>,
    submitted_body: &str,
    submitted_stage: &crate::state::template::Processed,
    parameters: &[StoredParameter],
) -> Option<crate::state::RefusedUpdate> {
    crate::state::RefusedUpdate::of(
        governing_policy,
        &planned_changes(previous, submitted_body, submitted_stage, parameters),
        &previous.deployed,
    )
}

// The request's `Parameters` are resolved against the stack's stored values by the shared
// `crate::state::merged_parameters`, which `CreateChangeSet` resolves the same member
// with.

// The resources a template declares are created, kept or deleted in the services that own
// them by the shared `crate::state::provisioning::reconcile`, which `CreateStack` and
// `ExecuteChangeSet` deploy through as well.

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateStackError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateStackError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#UpdateStack")]
#[derive(Debug)]
pub enum UpdateStackErrorModeled {
    InsufficientCapabilitiesException(InsufficientCapabilitiesException),
    TokenAlreadyExistsException(TokenAlreadyExistsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_stack::UpdateStackError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateStackError = crate::scaffolded::Scaffolded<UpdateStackErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateStackErrorModeled> for UpdateStackError {
    fn from(modeled: UpdateStackErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateStack` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateStack<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateStack<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateStack<Eng>
{
    type Input = UpdateStackInput;
    type Output = UpdateStackOutput;
    type Error = UpdateStackError;

    const NAME: &'static str = "UpdateStack";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STACK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STACK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        mut input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // CloudFormation checks the constraints on the stack identifier a request names
        // BEFORE it resolves the stack, so an identifier no stack could carry is answered as
        // the malformed request it is (`crate::state::stack_identifier`).
        crate::state::refuse_malformed_identifier(
            crate::state::STACK_NAME_PATH,
            Some(&input.stack_name),
        )?;
        // "Conditional: You must specify only one of the following parameters:
        // TemplateBody, TemplateURL, or set the UsePreviousTemplate to true." Each way of
        // not stating exactly one source is its own refusal — the template stated both by
        // value and by reference, the stack's own template reused ALONGSIDE a submitted one,
        // and no source at all — in the one place that owns the rule and its three
        // sentences.
        crate::state::validate_template_sources(
            input.template_body.as_deref(),
            input.template_url.as_deref(),
            input.use_previous_template == Some(true),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // A `TemplateURL` says WHERE the revised template is, not what it means: the object
        // it addresses is read from this emulator's own Amazon S3 and becomes the body this
        // update deploys, so a template submitted by reference and the same one submitted
        // inline apply the same update (`crate::state::template_url`).
        let from_reference = input.template_url.is_some();
        if let Some(url) = input.template_url.take() {
            let body = crate::state::template_url::fetch(
                &*self.engine,
                ctx,
                &crate::state::request_region(ctx.parts()),
                &url,
            )
            .await
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
            input.template_body = Some(body);
        }
        // A stack policy reaches CloudFormation either inline, as the request's own policy
        // body, or by reference, as a URL naming "a policy (maximum size: 16 KB) located in
        // an Amazon S3 bucket in the same Amazon Web Services Region as the stack" — and
        // either member of a pair may be given, "but not both".
        crate::state::stack_policy::one_policy_source(
            "StackPolicyBody",
            "StackPolicyURL",
            input.stack_policy_body.as_deref(),
            input.stack_policy_url.as_deref(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        crate::state::stack_policy::one_policy_source(
            "StackPolicyDuringUpdateBody",
            "StackPolicyDuringUpdateURL",
            input.stack_policy_during_update_body.as_deref(),
            input.stack_policy_during_update_url.as_deref(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The reference says WHERE each policy document is, not what it means: the object it
        // addresses is read from this emulator's own Amazon S3 and becomes the body this
        // update acts on, so a policy submitted by reference and the same one submitted
        // inline govern the update alike — the one that OVERRIDES the stack's policy for
        // this update, and the one the update leaves ON the stack
        // (`crate::state::stack_policy::fetch`).
        if let Some(url) = input.stack_policy_url.take() {
            let body = crate::state::stack_policy::fetch(
                &*self.engine,
                ctx,
                &crate::state::request_region(ctx.parts()),
                "StackPolicyURL",
                "StackPolicyBody",
                &url,
            )
            .await
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
            input.stack_policy_body = Some(body);
        }
        if let Some(url) = input.stack_policy_during_update_url.take() {
            let body = crate::state::stack_policy::fetch(
                &*self.engine,
                ctx,
                &crate::state::request_region(ctx.parts()),
                "StackPolicyDuringUpdateURL",
                "StackPolicyDuringUpdateBody",
                &url,
            )
            .await
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
            input.stack_policy_during_update_body = Some(body);
        }
        // The same rule a create's rollback triggers are held to: a trigger naming a type
        // CloudFormation cannot monitor is refused before the stack is touched, so an
        // update never puts unusable rollback monitoring on it
        // (`crate::state::validate_rollback_configuration`).
        crate::state::validate_rollback_configuration(input.rollback_configuration.as_ref())
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The same rule a create's rollback switches are held to: the two members say one
        // thing, so an update that sets them to DIFFERENT values is refused before the stack
        // is touched rather than run under a rollback setting it never consistently asked
        // for (`crate::state::validate_rollback_switches`).
        crate::state::validate_rollback_switches(
            input.disable_rollback,
            input.deployment_config.as_ref(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The same rule a create's tags are held to: a key in AWS's own reserved namespace
        // is refused before the stack is touched, so an update cannot put a key the service
        // itself owns onto the stack — unless the service that owns the namespace is the one
        // updating it (`crate::state::stack_tags`).
        crate::state::stack_tags::validate_optional_tags_from(
            &*self.engine,
            ctx,
            input.tags.as_deref(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // "Only one of the Capabilities and ResourceType parameters can be specified": the
        // two members state two different bounds on this update, so a request stating both
        // states no one bound and is refused for its SHAPE, before either is weighed
        // (`crate::state::resource_type_allow_list`).
        crate::state::resource_type_allow_list::one_bound(
            input.capabilities.as_deref().unwrap_or_default(),
            input.resource_types.as_deref().unwrap_or_default(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no stack to update, and nothing
            // was written.
            return Err(crate::scaffolded::Scaffolded::not_implemented("UpdateStack"));
        };
        // The body this update deploys, with its transforms applied — computed BEFORE the
        // store lock below, because it is caller-sized work that must not be done while
        // every other request waits. `UsePreviousTemplate` ("Reuse the existing template
        // that is associated with the stack that you are updating") needs no work at all:
        // the stack already carries that body's processed stage.
        let submitted = match &input.template_body {
            Some(body) => {
                // The ceiling the SOURCE that carried this template is held to: the
                // documented `TemplateBody` maximum for a body the request carried inline,
                // and the documented maximum of a template read from an object for one the
                // reference above resolved.
                crate::state::validate_submitted_template_body(body, from_reference)
                    .map_err(crate::scaffolded::Scaffolded::validation_error)?;
                // The documented per-template quotas, the ceilings
                // `DescribeAccountLimits` advertises for this account: a body past one is
                // refused before the update starts, so the stack keeps the template it
                // has rather than one holding none of what the body declared.
                crate::state::validate_template_quotas(body)
                    .map_err(crate::scaffolded::Scaffolded::validation_error)?;
                // A submitted template is only one CloudFormation can deploy: a body
                // declaring a resource type it does not recognise is refused before the
                // update starts, so the stack keeps the template it has
                // (`crate::state::resource_types`).
                crate::state::validate_resource_types(body)
                    .map_err(crate::scaffolded::Scaffolded::validation_error)?;
                Some(body.clone())
            }
            None => None,
        };
        // The update runs as the `RoleARN` it names, so a role CloudFormation cannot
        // assume is refused before the stack is touched — the same check `CreateStack`
        // makes. A request that names none keeps the role already on the stack, which was
        // checked when the request that set it named it.
        crate::state::refuse_unassumable_role(&*self.engine, input.role_arn.as_deref())?;
        let now = ::std::time::SystemTime::now();
        // A stack name or id names a stack only in the Region the request was signed for.
        let request_region = crate::state::request_region(ctx.parts());
        // Whether this stack can take an update at all depends on the status it is in, so a
        // rollback the failure of an earlier update already made due is carried out first:
        // it was owed at the instant that update failed, whether or not anybody called
        // (`crate::state::settle_due_rollbacks`).
        crate::state::settle_due_rollbacks(ctx, &*self.engine).await;
        // The store is read under the lock and the lock is RELEASED with this block: what
        // the update computes from the request and the stack's previous values needs no
        // lock, and the calls that create or delete the stack's resources must not run
        // under one.
        let (previous, managed_names) = {
            let mut store = state.lock();
            store.settle(now);

            let not_found = || {
                crate::scaffolded::Scaffolded::validation_error(
                    crate::state::changed_stack_does_not_exist(&input.stack_name),
                )
            };
            let previous = store
                .resolve(&request_region, &input.stack_name)
                .ok_or_else(not_found)?;
            // A stack id resolves a deleted stack too, but a deleted stack cannot be
            // updated.
            if !previous.is_live() {
                return Err(not_found());
            }
            // "A unique identifier for this UpdateStack request": a token an operation on
            // this stack already claimed names THAT operation, so presenting it for this
            // one is the duplicate submission the modelled `TokenAlreadyExistsException`
            // refuses — weighed before the stack's own status is, since a duplicate
            // submission is a bad request whatever the stack is doing.
            if let Some(message) = crate::state::reused_client_request_token(
                previous,
                input.client_request_token.as_deref(),
            ) {
                return Err(UpdateStackErrorModeled::TokenAlreadyExistsException(
                    TokenAlreadyExistsException {
                        message: Some(message),
                    },
                )
                .into());
            }
            // An operation still in flight on this stack, or a stack whose status says it
            // holds no configuration an update could change
            // ([`crate::state::status_takes_an_update`]): a deletion or a rollback that
            // failed and has to be resolved on its own terms, a create that was rolled back,
            // and a stack a `CREATE` change set created that nothing has executed yet (it
            // has no template and no resources to update — the change set is what brings it
            // into existence). None of them can take an update.
            if previous.pending.is_some()
                || !crate::state::status_takes_an_update(&previous.status)
            {
                return Err(crate::scaffolded::Scaffolded::validation_error(
                    crate::state::stack_cannot_be_updated(&previous.stack_id, &previous.status),
                ));
            }
            // ...and a stack whose own update FAILED and was left standing takes one only
            // from a caller that asks for that state to be preserved again — the gate every
            // call that deploys onto such a stack goes through
            // ([`crate::state::fix_forward_refusal`]).
            if let Some(message) =
                crate::state::fix_forward_refusal(previous, disables_rollback(&input))
            {
                return Err(crate::scaffolded::Scaffolded::validation_error(message));
            }
            let values = PreviousValues::of(previous);
            // ...and which names the live stacks of the Region already manage: a name one of
            // them holds is CloudFormation's own to report on the resource rather than the
            // existence check's to fail
            // ([`crate::state::early_validation::named_additions`]). The index the WALK is
            // held to is read again under the writing lock below, since that is the one it
            // must be current with.
            (values, store.managed_physical_names(&request_region))
        };

        let template_body = match submitted {
            Some(body) => body,
            // `UsePreviousTemplate`: "Reuse the existing template that is associated with
            // the stack that you are updating." A stack a `CREATE` change set created and
            // nothing has executed yet carries no template to reuse.
            None => previous.template_body.clone().ok_or_else(|| {
                crate::scaffolded::Scaffolded::validation_error(format!(
                    "Stack [{}] has no template to reuse",
                    input.stack_name
                ))
            })?,
        };
        // "In some cases, you must explicitly acknowledge that your stack template contains
        // certain capabilities in order for CloudFormation to update the stack ... If you
        // don't specify either of these capabilities, CloudFormation returns an
        // InsufficientCapabilities error": the acknowledgement is the one THIS request
        // carries, and the template it would deploy — submitted or reused — is what is held
        // to it, before the stack is touched.
        if let Some(message) = crate::state::insufficient_capabilities(
            &template_body,
            input.capabilities.as_deref().unwrap_or_default(),
        ) {
            return Err(UpdateStackErrorModeled::InsufficientCapabilitiesException(
                InsufficientCapabilitiesException {
                    message: Some(message),
                },
            )
            .into());
        }
        // "If the list of resource types doesn't include a resource that you're creating,
        // the stack creation fails": the bound THIS request put on its own deploy — the
        // member is the permission this call works with, so a request that states none
        // states no bound — is weighed against the template this update would deploy,
        // before the stack is touched (`crate::state::resource_type_allow_list`).
        crate::state::validate_allowed_resource_types(
            &template_body,
            input.resource_types.as_deref().unwrap_or_default(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        let parameters =
            crate::state::merged_parameters(&previous.parameters, input.parameters.as_deref())
                .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // A parameter the template declares and the request passed no value for takes the
        // template's own `Default`, exactly as on a create.
        let mut parameters = crate::state::parameters_in_force(&template_body, parameters);
        // The same Parameter Store resolution a create performs: an SSM-typed parameter's
        // value is the NAME of a parameter, and the value in force on the updated stack is
        // the one the store holds NOW (`crate::state::ssm_parameters`).
        crate::state::ssm_parameters::resolve(ctx, &request_region, &template_body, &mut parameters)
            .await;
        let parameter_values = crate::state::parameter_value_map(&parameters);
        // The stage this update would deploy, computed from the body it deploys and the
        // parameter values in force: the stage is a function of BOTH — a directive and a
        // transform's own expansion read those values — so an update that reuses the stack's
        // own template still computes the stage its OWN parameters make, never the one the
        // deployment before it computed from the values that were in force then.
        let processed_template =
            crate::state::template::process_with_parameters(&template_body, &parameter_values);
        // The same template validation a create is held to: a body whose own declarations
        // CloudFormation cannot resolve is refused HERE, so the stack keeps the template it
        // has rather than starting an update from one real AWS would never accept.
        crate::state::validate_submitted_template(
            &template_body,
            &processed_template,
            &parameter_values,
            &request_region,
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The same input CONTRACT a create is held to, in the same place in the order: the
        // parameter set this update resolved has to match what the template it would deploy
        // declares, so the stack keeps the parameters it has rather than starting an update
        // real AWS would refuse (`crate::state::parameter_constraints`).
        crate::state::validate_parameter_set(
            &template_body,
            &parameters,
            input.parameters.as_deref(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // "Rules: validates a parameter or a combination of parameters passed to a template
        // during a stack creation or stack update": an assertion the values in force do not
        // satisfy fails the update, which leaves the stack as it was — and so does a
        // directive the declared transform cannot apply where it stands.
        let deployment_failure = crate::state::template_validation::deployment_failure(
            &template_body,
            &parameter_values,
        );
        // The same AWS-specific parameter-type resolution a create performs: a parameter
        // whose declared `Type` is one of them carries the identifier of an EXISTING resource
        // of this account and Region, so each such value is resolved against the service that
        // owns the resource before anything is changed. A value that names none fails this
        // update, which leaves the stack as it was
        // (`crate::state::parameter_constraints::aws_specific_types`).
        let parameter_failure =
            crate::state::parameter_constraints::aws_specific_types::deployment_failure(
                ctx,
                &request_region,
                &template_body,
                &parameters,
            )
            .await;
        // "If you don't specify this parameter, CloudFormation doesn't modify the stack's
        // tags. If you specify an empty value, CloudFormation removes all associated
        // tags." The same shape holds for the other collection members the request may
        // restate: absent leaves the stored value alone, present replaces it.
        let tags = match input.tags.as_deref() {
            Some(tags) => crate::state::stored_tags(tags),
            None => previous.tags.clone(),
        };
        // "Specify an empty list to remove all notification topics."
        let notification_arns = match &input.notification_ar_ns {
            Some(arns) => arns.clone(),
            None => previous.notification_arns.clone(),
        };
        let capabilities = match &input.capabilities {
            Some(capabilities) => capabilities.clone(),
            None => previous.capabilities.clone(),
        };
        // "If you don't specify a value, CloudFormation uses the role that was previously
        // associated with the stack."
        let role_arn = input.role_arn.clone().or(previous.role_arn.clone());
        // The same role, kept for the deployment below: it is what the calls this update
        // makes in the services that own the stack's resources are made under
        // ([`crate::state::service_role`]).
        let role_in_force = role_arn.clone();
        let rollback_configuration = match input.rollback_configuration.as_ref() {
            Some(configuration) => {
                crate::state::stored_rollback_configuration(Some(configuration))
            }
            None => previous.rollback_configuration.clone(),
        };
        let deployment_config = match input.deployment_config.as_ref() {
            Some(config) => crate::state::stored_deployment_config(Some(config)),
            None => previous.deployment_config.clone(),
        };

        // The macro acknowledgement the template needs, against the capabilities in force
        // on this update (the request's when it restated them, the stack's otherwise):
        // "If your stack template contains one or more macros [...] you must acknowledge
        // this capability."
        let missing = crate::state::missing_auto_expand_capability(&template_body, &capabilities);
        if !missing.is_empty() {
            return Err(UpdateStackErrorModeled::InsufficientCapabilitiesException(
                InsufficientCapabilitiesException {
                    message: Some(crate::state::template::insufficient_capabilities_message(
                        &missing,
                    )),
                },
            )
            .into());
        }

        // The template this update would deploy is PROCESSED first, exactly as a create's
        // is: an `Fn::Transform` of `AWS::Include` is the Amazon S3 snippet its `Location`
        // names, read and spliced in before anything is provisioned
        // (`crate::state::include`). The snippet is read HERE, on this operation, rather
        // than taken from what an earlier one read: the template a snippet is part of is
        // only ever the document the objects in force RIGHT NOW make, so it is also what
        // this update is compared with below.
        let processing =
            crate::state::include::process(ctx, &request_region, &template_body).await;
        // ...and then the REGISTERED macros its `Transform` section names are performed over
        // that document, exactly as a create's are: an update deploys the document the macros
        // produced, so a macro's output is what this update is compared with and what its
        // resources are created from ([`crate::state::private_macro::expand_declared`]).
        let processing = crate::state::private_macro::expand_declared(
            ctx,
            &*self.engine,
            &request_region,
            &template_body,
            &parameter_values,
            processing,
        )
        .await;
        // The acknowledgement is held to the PROCESSED template, exactly as a create's is:
        // a resource an included snippet contributes is one this update would deploy.
        if let crate::state::include::Processed::Expanded(processed) = &processing {
            if let Some(message) = crate::state::insufficient_capabilities(
                processed,
                input.capabilities.as_deref().unwrap_or_default(),
            ) {
                return Err(UpdateStackErrorModeled::InsufficientCapabilitiesException(
                    InsufficientCapabilitiesException {
                        message: Some(message),
                    },
                )
                .into());
            }
        }
        let transform_failure = match &processing {
            crate::state::include::Processed::Failed(reason) => Some(reason.clone()),
            _ => None,
        }
        .or(deployment_failure)
        .or(parameter_failure);
        // The transforms the template DECLARES are applied to the spliced document, so the
        // stage this update deploys and `GetTemplate` serves is the whole pipeline's — and
        // that expansion, like the splice, happens before the store lock below.
        // The body that stage is computed over, which is also the body the plan below is
        // read from: the spliced document where a transform named a snippet, else the
        // submitted body itself.
        let included_body = match &processing {
            crate::state::include::Processed::Expanded(processed) => processed.clone(),
            _ => template_body.clone(),
        };
        let processed_template = match &processing {
            crate::state::include::Processed::Expanded(processed) => {
                crate::state::template::process_with_parameters(processed, &parameter_values)
            }
            _ => processed_template,
        };

        // The policy the stack ALREADY carries is validated against the template this
        // update submits BEFORE anything else is weighed: "stack policies can only be
        // applied to logical ids referenced in the template", so a stored statement naming
        // a logical id the submitted template does not declare refuses the call outright —
        // nothing is deployed, no event is recorded and the stack keeps the status it has
        // ([`crate::state::stack_policy::unknown_logical_id`]).
        if let Some(stored_policy) = &previous.stack_policy_body {
            crate::state::stack_policy::unknown_logical_id(
                stored_policy,
                &submitted_logical_ids(&included_body, &processed_template),
            )
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        }

        // CloudFormation refuses an update that would change nothing about the stack. What
        // decides that is the template the update would DEPLOY — the one its transforms
        // expanded, since the stack's resources are computed from that document and not
        // from the body that names the transform — so a snippet revised since the stack
        // last deployed is a change the update carries out, and a body naming no transform
        // is compared with itself exactly as the submitted text. A template whose
        // processing FAILED is compared with nothing: the operation reports that failure
        // rather than calling the update a no-op.
        //
        // A stack whose last operation FAILED and was left standing is never a no-op,
        // whatever the request restates: its resources do not carry the configuration its
        // template declares, so resubmitting that configuration is the RETRY of the work the
        // failure left outstanding ([`crate::state::status_preserved_a_failure`]).
        // A body that differs from the stack's only in codepoints the template store cannot
        // hold is the SAME template to CloudFormation: the text a read of the stored template
        // serves is ASCII ([`crate::state::stored_template_body`]), so a client that
        // resubmits what `GetTemplate` answered has submitted the template the stack already
        // carries — which is this refusal and not an update.
        if !crate::state::status_preserved_a_failure(&previous.status)
            && !matches!(processing, crate::state::include::Processed::Failed(_))
            && previous
                .included_template_body
                .as_deref()
                .is_some_and(|stored| {
                    crate::state::ascii_text_eq(stored, &included_body)
                })
            && parameters == previous.parameters
            && tags == previous.tags
            && notification_arns == previous.notification_arns
            && capabilities == previous.capabilities
            && role_arn == previous.role_arn
            && rollback_configuration == previous.rollback_configuration
            && deployment_config == previous.deployment_config
            // ...and the stack POLICY the update would leave in force is the one already in
            // force: "structure that contains a new stack policy body" is a member of the
            // stack's configuration this update writes, so a request that submits a
            // different document — inline or by reference — changes the stack and is not a
            // no-op, whatever it leaves the template and the parameters as.
            && input
                .stack_policy_body
                .as_ref()
                .is_none_or(|body| previous.stack_policy_body.as_ref() == Some(body))
        {
            return Err(crate::scaffolded::Scaffolded::validation_error(
                "No updates are to be performed.",
            ));
        }
        // Everything above read the store; nothing below it does until the template has
        // been deployed.
        let operation_id = crate::state::new_operation_id();

        // What refuses this update from the PLAN alone, weighed BEFORE anything about the
        // stack is written — a refused update deploys nothing, and so changes neither the
        // stack's configuration nor its resources ([`crate::state::RefusedUpdate`]): the
        // stack policy in force ("updates on the resources you specify are denied"), and a
        // replacement CloudFormation cannot carry out because the resource it would replace
        // is one the template NAMES ITSELF.
        //
        // Which policy is in force is what `StackPolicyDuringUpdateBody` decides: "a
        // temporary overriding stack policy during this update. If you don't specify a stack
        // policy, the current policy that is associated with the stack will be used." The
        // override applies to THIS update alone and is never stored, so the stack goes on
        // carrying the document `GetStackPolicy` reads back — and a `StackPolicyBody` this
        // request submits is the document the stack is left with, not the one that decides
        // this update ("the current policy that is associated with the stack is unchanged"
        // is what omitting it means), so it cannot lift a Deny the update is held to.
        //
        // A template whose processed stage this update could not compute has no plan to
        // weigh: that failure is the one reported, below.
        let governing_policy = input
            .stack_policy_during_update_body
            .clone()
            .or_else(|| previous.stack_policy_body.clone());
        let refusal = match &transform_failure {
            // Asked about the body the stage was computed OVER — the spliced document where a
            // transform named an `AWS::Include` snippet — so the plan every verdict is
            // reached against covers the resources that snippet contributes too.
            None => refused_update(
                &previous,
                governing_policy.as_deref(),
                &included_body,
                &processed_template,
                &parameters,
            ),
            Some(_) => None,
        };
        if let Some(refusal) = refusal {
            // The refused resource's update FAILS, and the stack rolls back: "if the stack
            // update fails, CloudFormation rolls back the stack to the previous stack
            // configuration" — which here is the configuration still in force, nothing
            // having been deployed. A caller that asked for a failed update's state to be
            // kept (`DisableRollback`) stops at `UPDATE_FAILED`, as on any other failure.
            let mut store = state.lock();
            let stack = store
                .resolve_mut(&request_region, &input.stack_name)
                .filter(|stack| stack.is_live())
                .ok_or_else(|| {
                    crate::scaffolded::Scaffolded::validation_error(
                        crate::state::stack_does_not_exist(&input.stack_name),
                    )
                })?;
            // The same preconditions the write block re-asserts, for the same reason: the
            // read above released the lock, and only one operation may be in flight.
            if let Some(message) = crate::state::reused_client_request_token(
                stack,
                input.client_request_token.as_deref(),
            ) {
                return Err(UpdateStackErrorModeled::TokenAlreadyExistsException(
                    TokenAlreadyExistsException {
                        message: Some(message),
                    },
                )
                .into());
            }
            if stack.pending.is_some() || !crate::state::status_takes_an_update(&stack.status) {
                return Err(crate::scaffolded::Scaffolded::validation_error(
                    crate::state::stack_cannot_be_updated(&stack.stack_id, &stack.status),
                ));
            }
            let client_request_token = input
                .client_request_token
                .clone()
                .unwrap_or_else(crate::state::new_client_request_token);
            stack.client_request_token = Some(client_request_token.clone());
            stack.begin_transition(
                now,
                status::UPDATE_IN_PROGRESS,
                status::UPDATE_COMPLETE,
                Some(operation_id.clone()),
                Some(client_request_token),
            );
            // The resource's own events come before the stack's terminal one, exactly as a
            // deployment's do: the verdict is what the event stream reports about that
            // resource, and the rollback that follows a failed update is what then skips it
            // ([`crate::state::RefusedUpdate::transitions`]).
            let rolls_back = !disables_rollback(&input).unwrap_or(stack.disable_rollback);
            stack.schedule_resource_events(&refusal.transitions(rolls_back));
            let failed_at = ::std::time::SystemTime::now();
            // What the STACK reports it failed for is the verdict's own account of the
            // refusal ([`crate::state::RefusedUpdate::stack_reason`]).
            let reason = refusal.stack_reason();
            // "Preserve the state of previously provisioned resources when an operation
            // fails": the request's own `DisableRollback` where it carries one, else the
            // value in force on the stack — the same reading the deploying path gives that
            // member, so a refused update and a failed one answer it alike. It decides
            // whether the stack RESTS in `UPDATE_FAILED`, and so whether an event of its
            // own reports that status
            // ([`crate::state::StoredStack::fail_transition_superseded`]).
            if !rolls_back {
                stack.fail_transition(failed_at, status::UPDATE_FAILED, reason);
            } else {
                stack.fail_transition_superseded(failed_at, status::UPDATE_FAILED, reason.clone());
                stack.begin_update_rollback(failed_at, reason);
            }
            return Ok(UpdateStackOutput {
                stack_id: Some(stack.stack_id.clone()),
                operation_id: Some(operation_id),
            });
        }

        // The pre-deployment EXISTENCE check, the stage a create runs too: a resource this
        // update would CREATE under a physical name the service that owns the type ALREADY
        // holds cannot be created, so the update fails at that stage with nothing
        // provisioned and CloudFormation's own reason. A resource the stack already manages
        // is no addition — it exists because the stack has it — so only the declarations
        // this update brings are asked about. Each question reaches the service that owns
        // the type, so they are put HERE, with no store lock held
        // ([`crate::state::early_validation`]). A template this service could not process,
        // and an update that turned the validations off, are not asked at all.
        let name_conflict = if input.disable_validation == Some(true)
            || transform_failure.is_some()
        {
            None
        } else {
            let already_managed: Vec<String> = previous
                .deployed
                .iter()
                .map(|deployed| deployed.resource.logical_id.clone())
                .collect();
            let candidates = crate::state::early_validation::named_additions(
                processed_template.body(&included_body),
                crate::state::template::StackScope {
                    region: &request_region,
                    stack_name: &previous.stack_name,
                    stack_id: &previous.stack_id,
                    // The topics this update puts in force: what a name built from
                    // `AWS::NotificationARNs` would resolve to under it.
                    notification_arns: &notification_arns,
                },
                &parameter_values,
                &already_managed,
                &managed_names,
            );
            crate::state::early_validation::taken_names(ctx, &request_region, &candidates)
                .await
                .into_iter()
                .next()
        };
        // The ROLLBACK TRIGGERS this update is monitored by, read before anything is
        // deployed: an alarm already in `ALARM` when the operation begins rolls the whole
        // operation back, so nothing is asked of any service that owns one of the stack's
        // resources and none of them is changed at all
        // ([`crate::state::rollback_triggers`]). The read reaches CloudWatch, so it happens
        // HERE, with no store lock held, beside the existence check just above — and not at
        // all for a deployment already refused, whose one diagnostic is that refusal.
        let firing_triggers = if transform_failure.is_some() || name_conflict.is_some() {
            None
        } else {
            crate::state::rollback_triggers::firing_trigger_refusal(
                ctx,
                &request_region,
                rollback_configuration.as_ref(),
            )
            .await
        };

        // The store lock is taken for the write and RELEASED at the end of this block: the
        // calls that create or delete the stack's resources run after it, and each of
        // those reaches another service's own handler, which takes that service's lock.
        //
        // The preconditions are re-asserted HERE, under the writing lock, rather than
        // trusted from the read above: between the two, another request may have deleted
        // the stack or started an operation on it, and only one update may be in flight.
        let (
            stack_id,
            region,
            declared,
            previous_resources,
            mut provenance,
            refused_deployment,
            refused_by_validation,
        ) = {
            let mut store = state.lock();
            // What a `Fn::ImportValue` in the submitted template resolves against: the
            // exported output values in force in the Region this request is signed for,
            // read before the stack is resolved so the two cannot disagree about them.
            let exports = store.exported_values(&request_region);
            // ...and the outputs an `Fn::GetStackOutput` in it reads off the stacks of that
            // Region, read in the same breath and for the same reason.
            let stack_outputs = store.stack_output_values(&request_region);
            // ...and which physical names the live stacks of the Region manage, read in the
            // same breath: a resource this update CREATES under a name another stack owns
            // fails in CloudFormation's own words ([`crate::state::physical_name`]).
            let managed = store.managed_physical_names(&request_region);
            let stack = store
                .resolve_mut(&request_region, &input.stack_name)
                .filter(|stack| stack.is_live())
                .ok_or_else(|| {
                    crate::scaffolded::Scaffolded::validation_error(
                        crate::state::changed_stack_does_not_exist(&input.stack_name),
                    )
                })?;
            // Re-asserted under the WRITING lock, like the preconditions below: between
            // the read and here another request may have claimed this very token for an
            // operation of its own, and only one of the two may have it.
            if let Some(message) = crate::state::reused_client_request_token(
                stack,
                input.client_request_token.as_deref(),
            ) {
                return Err(UpdateStackErrorModeled::TokenAlreadyExistsException(
                    TokenAlreadyExistsException {
                        message: Some(message),
                    },
                )
                .into());
            }
            if stack.pending.is_some() || !crate::state::status_takes_an_update(&stack.status) {
                return Err(crate::scaffolded::Scaffolded::validation_error(
                    crate::state::stack_cannot_be_updated(&stack.stack_id, &stack.status),
                ));
            }
            // The template could not be PROCESSED, so this update never reached the stage
            // that provisions anything: nothing about the stack changes — it keeps the
            // template, the parameters and the resources it has — and the operation fails
            // with the transform's own reason.
            if let Some(reason) = transform_failure {
                stack.begin_transition(
                    now,
                    status::UPDATE_IN_PROGRESS,
                    status::UPDATE_COMPLETE,
                    Some(operation_id.clone()),
                    input.client_request_token.clone(),
                );
                stack.fail_transition(
                    ::std::time::SystemTime::now(),
                    status::UPDATE_FAILED,
                    reason,
                );
                return Ok(UpdateStackOutput {
                    stack_id: Some(stack.stack_id.clone()),
                    operation_id: Some(operation_id),
                });
            }
            // What the stack's resources are BEFORE this deployment: what the update has
            // to keep, replace or delete in the services that own them.
            let previous_resources = stack.current_resources().to_vec();
            // ...and what the stack's own CONFIGURATION record is before it: the revision a
            // failure of this update is rolled back to. The members below are written onto
            // the stack where the update starts rather than staged like its resources, so
            // the revision they replace is snapshotted here, while it is still in force
            // ([`crate::state::StackConfiguration`]).
            stack.hold_previous_configuration(Some(stack.configuration()));
            // The update's template and the PROCESSED stage of it this service computed:
            // the stage is what the update deploys. Where the submitted body named an
            // `AWS::Include`, the spliced document is recorded too — it is the body that
            // stage was computed over.
            stack.set_template_body(template_body, processed_template);
            if let crate::state::include::Processed::Expanded(processed) = processing {
                stack.set_processed_template_body(processed);
            }
            stack.parameters = parameters;
            stack.tags = tags;
            stack.notification_arns = notification_arns;
            stack.capabilities = capabilities;
            stack.role_arn = role_arn;
            stack.rollback_configuration = rollback_configuration;
            stack.deployment_config = deployment_config;
            // Either member of the request that says so disables the rollback of this
            // update, and an update that states neither is answered by the mode it runs in
            // ([`disables_rollback`]); a request that asks for nothing leaves the setting in
            // force on the stack exactly as it is. It is RECORDED on the stack because that
            // is where the failure path reads it back: a wait that runs out fails this
            // operation on the clock, with no call of the client's to consult
            // ([`crate::state::StoredStack::begin_owed_update_rollback`]).
            if let Some(disable_rollback) = disables_rollback(&input) {
                stack.disable_rollback = disable_rollback;
            }
            // An update names no failure ACTION — the member belongs to the calls that create
            // a stack — so from here on the stack carries none: what the setting above reports
            // and what a failure of THIS operation may be deployed onto again are this
            // request's statements, not the create's
            // ([`crate::state::StoredStack::on_failure`]).
            stack.on_failure = None;
            // `RetainExceptOnCreate` is THIS operation's, and its documented default is
            // `false`: "when set to true, newly created resources are deleted when the
            // operation rolls back […] Default: false" — so an update that states it nowhere
            // runs under that default rather than under whatever an earlier operation of the
            // same stack asked for. Recorded on the stack for the same reason the setting
            // above is: the rollback this update may owe is driven by the clock, with no call
            // of the client's to consult
            // ([`crate::state::StoredStack::rolled_back_resource_policies`]).
            stack.retain_except_on_create = input.retain_except_on_create;
            // "All events triggered by a given stack operation are assigned the same
            // client request token": the caller's token when it named one, otherwise the
            // one CloudFormation assigns the operation itself.
            let client_request_token = input
                .client_request_token
                .clone()
                .unwrap_or_else(crate::state::new_client_request_token);
            stack.client_request_token = Some(client_request_token.clone());
            // "Structure that contains a new stack policy body": present replaces the
            // stored policy, absent leaves it alone, as for the other restated members.
            if let Some(policy) = &input.stack_policy_body {
                stack.stack_policy_body = Some(policy.clone());
            }
            if let Some(resource_types) = &input.resource_types {
                stack.resource_types = resource_types.clone();
            }
            stack.last_updated_time = Some(now);
            // The update deploys the PROCESSED stage of the template the stack now
            // carries: its resources and its outputs are re-evaluated from the new body
            // and the merged parameters, each resource the deployment acts on reports its
            // own events, and all of it is reported once the update completes.
            let cannot_deploy = stack.deploy_template(&exports, &stack_outputs).err();
            // The update starts now and settles to UPDATE_COMPLETE once the clock makes
            // that due; the stack id never changes.
            stack.begin_transition(
                now,
                status::UPDATE_IN_PROGRESS,
                status::UPDATE_COMPLETE,
                Some(operation_id.clone()),
                Some(client_request_token),
            );
            // The walk that changes this stack's resources runs below, with this store
            // unlocked: the update is not due its terminal status until that walk has
            // reported back ([`crate::state::StoredStack::carrying_out_its_deployment`]).
            stack.carrying_out_its_deployment();
            // A wait condition this update CREATES again — the template changed its handle —
            // waits for its own signals: the ones that satisfied the wait condition it
            // replaces belong to the deployment that created that one
            // ([`crate::state::StoredStack::retire_signals_of_open_waits`]).
            stack.retire_signals_of_open_waits();
            // A template this service cannot deploy ends the update in the modelled
            // UPDATE_FAILED with the reason, rather than in an UPDATE_COMPLETE that
            // reports a deployment nothing performed. The resources and outputs in force
            // are left as they were, which is what a failed update leaves behind.
            if let Some(reason) = cannot_deploy {
                stack.fail_pending_transition(status::UPDATE_FAILED, reason);
            }
            // "The intrinsic function Fn::ImportValue returns the value of an output
            // exported by another stack": an export name no live stack of this account and
            // Region publishes has no value to import, so CloudFormation refuses the
            // deployment before anything is asked of the services that own the resources —
            // the update then fails naming the export and is rolled back below.
            //
            // An exported output value another live stack still IMPORTS is a rule of the
            // other stage: the exports an update publishes are installed when it COMPLETES,
            // so that one is decided where the update would publish them, after its resources
            // have been changed (below, beside the export-name uniqueness rule).
            let unresolved_cross_stack = stack.unresolved_cross_stack_failure();
            // "Set to true to disable pre-deployment validations in [...] stack
            // operations", default false: with them ON, a declaration whose properties its
            // type's contract does not admit — a required property it does not carry, a
            // value outside the set the property publishes — fails the update before
            // anything is provisioned: nothing is asked of the service that owns the
            // resource, so no resource of this stack is changed and none reports an event of
            // its own. An update that disables them deploys the declaration as written and
            // the same problem surfaces as that resource's own failure
            // ([`crate::state::property_validation`]). The failure is reported as the
            // validation result it is, on an event about the resource NOTHING was done to,
            // which is the detail `DescribeEvents` carries.
            //
            // Not asked at all of a deployment CloudFormation has already refused for a
            // reference it could not resolve, for the reason `create_stack` records: that
            // refusal is the one diagnostic for every property whose value it took away.
            let property_failures = if input.disable_validation != Some(true)
                && unresolved_cross_stack.is_none()
            {
                crate::state::property_validation::failures(&stack.declarations_to_validate())
            } else {
                Vec::new()
            };
            // One event per error, recorded LAST ERROR FIRST for the reason `create_stack`
            // records them that way: the operation's event stream reports the events of one
            // operation newest first, so this is the order that reads them in the order the
            // stage found them.
            for failure in property_failures.iter().rev() {
                stack.record_validation_failure(
                    ::std::time::SystemTime::now(),
                    &failure.logical_id,
                    &failure.resource_type,
                    crate::state::StoredValidationFailure {
                        name: crate::state::property_validation::PROPERTY_VALIDATION.to_string(),
                        status: crate::state::early_validation::VALIDATION_FAILED.to_string(),
                        reason: failure.reason.clone(),
                        path: failure.path.clone(),
                    },
                    Some(operation_id.clone()),
                    stack.client_request_token.clone(),
                );
            }
            // ...and so is the existence check's own failure, about the resource whose
            // identifier the owning service already holds. A deployment CloudFormation has
            // already refused is not reported as failing this stage too: the first refusal
            // is its one diagnostic.
            let name_conflict = name_conflict.filter(|_| {
                unresolved_cross_stack.is_none() && property_failures.is_empty()
            });
            if let Some(conflicting) = &name_conflict {
                stack.record_validation_failure(
                    ::std::time::SystemTime::now(),
                    &conflicting.logical_id,
                    &conflicting.resource_type,
                    crate::state::early_validation::name_conflict_failure(conflicting),
                    Some(operation_id.clone()),
                    stack.client_request_token.clone(),
                );
            }
            // What a failed pre-deployment validation refuses the deployment with: the
            // summary over the stage's errors, whichever stage failed, each error's own
            // detail being on the validation result recorded above
            // ([`crate::state::early_validation::validation_failure_summary`]).
            let failed_validation = (!property_failures.is_empty())
                .then(|| {
                    crate::state::property_validation::validation_failure_summary(
                        &property_failures,
                    )
                })
                .or_else(|| {
                    name_conflict.as_ref().map(|_| {
                        // ONE error: the existence check refuses the deployment on the first
                        // addition whose identifier the owning service already holds.
                        crate::state::early_validation::validation_failure_summary(1)
                    })
                });
            // ...and whether that stage is what refused THIS deployment, known here rather
            // than re-derived from the reason text below: a refusal reached before it stands
            // instead.
            let refused_by_validation =
                unresolved_cross_stack.is_none() && failed_validation.is_some();
            let refused_deployment = unresolved_cross_stack
                .or(failed_validation)
                // ...and the rollback an alarm already in `ALARM` forces, read before this
                // block was entered: it rolls the operation back having changed nothing.
                .or(firing_triggers);
            (
                stack.stack_id.clone(),
                stack.region.clone(),
                stack.current_resources().to_vec(),
                previous_resources,
                crate::state::provisioning::StackProvenance {
                    managed_names: managed,
                    // This deployment CHANGES resources the stack already has, so a resource
                    // whose change its provider refuses is put back the way it was. Where
                    // the stack RESTS in `UPDATE_FAILED` there is no rollback of the stack's
                    // own to report that revert, so this deployment performs it and the
                    // resource's own rollback transitions report it; where the stack rolls
                    // back, that rollback is what declares the previous configuration again
                    // ([`crate::state::provisioning::FailedResourceRevert`]).
                    failed_resource_revert: if stack.disable_rollback {
                        crate::state::provisioning::FailedResourceRevert::OfTheResourceAlone
                    } else {
                        crate::state::provisioning::FailedResourceRevert::None
                    },
                    // ...and the declarations this update deploys are resolved again where a
                    // value one of them reads has moved, so a resource is deployed with the
                    // values in force after this deployment's own changes
                    // ([`crate::state::provisioning::StackProvenance::declarations_resolve_again`]).
                    declarations_resolve_again: true,
                    // ...and one whose read had nothing to resolve against at all is withheld
                    // until it has
                    // ([`crate::state::provisioning::StackProvenance::reads_unpublished_attributes`]).
                    reads_unpublished_attributes: stack
                        .declarations_reading_unpublished_attributes(),
                    // ...and whether this update states the stack's TAGS differently from the
                    // set its resources carry, which is a change to every one of them
                    // ([`crate::state::provisioning::StackProvenance::tags_changed`]).
                    tags_changed: !crate::state::same_tags(&previous.tags, &stack.tags),
                    ..stack.provenance()
                },
                refused_deployment,
                refused_by_validation,
            )
        };

        // ...and the pre-condition on the exports this update would PUBLISH is the WALK's to
        // ask, once its changes are made and before it takes down what it supersedes: an
        // update the pin cancels must leave the resource publishing the pinned value exactly
        // where it is ([`crate::state::exports::ExportPublication`]). Installed HERE, with the
        // store lock released: it is a handle onto the store, and the walk is what reads
        // through it — under its own lock, for the one reading it makes.
        provenance.publication = Some(crate::state::exports::ExportPublication::of(
            &state,
            &region,
            &stack_id,
        ));

        // The update's resources are reconciled with the ones the stack already had: what
        // the new template declares again is kept, what it declares anew is created in the
        // service that owns it, and what it no longer declares is deleted there — unless
        // CLOUDFORMATION itself refused the deployment, in which case nothing is asked of
        // any of those services at all.
        let refused_anything = refused_deployment.is_some();
        // The calls below are the STACK's, made under its service role: "CloudFormation
        // uses the role's credentials to make calls on your behalf", so the role in force
        // on the stack decides them ([`crate::state::service_role`]).
        let role = crate::state::service_role::ServiceRole::of(&*self.engine, role_in_force.as_deref());
        // ...and every ROLE the template hands to an owning service is one the principal whose
        // credentials this update's calls are made with has to be allowed to pass
        // ([`crate::state::pass_role`]), for the reason `CreateStack`'s own gate records.
        let principal = crate::state::pass_role::principal_of(
            &*self.engine,
            ctx,
            role_in_force.as_deref(),
        );
        let materialised = crate::state::pass_role::as_principal(
            principal,
            crate::state::service_role::acting_as(
                role,
                crate::state::provisioning::reconcile(
                    ctx,
                    &region,
                    &provenance,
                    declared,
                    &previous_resources,
                    refused_deployment,
                    // An update is where CloudFormation stops managing a resource the template
                    // no longer declares, so that resource's own `DeletionPolicy` decides what
                    // becomes of it.
                    crate::state::provisioning::Policies::Honoured,
                ),
            ),
        )
        .await;

        let output = UpdateStackOutput {
            stack_id: Some(stack_id.clone()),
            operation_id: Some(operation_id),
        };
        // What the failure (if any) owes the stack beyond the UPDATE_FAILED it records: the
        // ROLLBACK of the update, and the configuration to put back in the services that own
        // the resources the update had already changed there.
        //
        // "If the stack update fails, CloudFormation rolls back the stack to the previous
        // stack configuration" — so the rollback is what a failed update is owed WHATEVER the
        // update got through before it failed. How far it got decides how much the rollback
        // has to put back, never whether the stack rolls back: an update whose very first
        // resource the owning service REFUSED changed nothing anywhere, so its rollback asks
        // those services for nothing, and the stack still goes through
        // UPDATE_ROLLBACK_IN_PROGRESS to UPDATE_ROLLBACK_COMPLETE — the terminal status a
        // client waiting on a failed update reads. The one update that stops at
        // UPDATE_FAILED is the one whose caller asked for the failed state to be kept
        // (`DisableRollback`), which is the state `ContinueUpdateRollback` and
        // `RollbackStack` act on.
        let owed = crate::state::OwedRollback {
            previous: previous_resources.clone(),
            // A deployment CloudFormation refused outright changed nothing in any owning
            // service, so what it APPLIED is the configuration that was already in force:
            // its rollback has nothing to put back.
            applied: if refused_anything {
                previous_resources.clone()
            } else {
                materialised.resources.clone()
            },
            // ...and the resources an owning service was ASKED to change and refused, which
            // this rollback declares the previous configuration of again whatever the
            // records say ([`crate::state::provisioning::Materialised::part_changed`]).
            part_changed: materialised.part_changed.clone(),
        };
        {
            let mut store = state.lock();
            let Some(stack) = store.resolve_mut(&request_region, &stack_id) else {
                return Ok(output);
            };
            // A resource whose owning service has not reported it ready yet holds this
            // operation open ([`crate::state::stabilising`]), which is what withholds the
            // transition that would report the resource created — and so does one whose
            // delete that service accepted while still holding it, which is what keeps the
            // update from completing while a resource it replaced or dropped is still there.
            stack.record_stabilisations(&materialised.stabilising, ::std::time::SystemTime::now());
            // The resources' own transitions come BEFORE the stack's terminal event: the
            // event stream is what the update did, in the order it did it.
            stack.schedule_resource_events(&materialised.transitions);
            stack.record_materialised_resources(materialised.resources.clone());
            // What a rollback of THIS update would have to put back in the services that own
            // the resources — held whether or not the update has failed yet, since a wait
            // condition it is held at can still fail it with no call of its own to work that
            // out. The plan says WHAT such a rollback has to put back, which for an update
            // that changed nothing anywhere is nothing at all.
            stack.hold_rollback_plan(Some(owed));
            // An update whose resources were all changed can still be one CloudFormation
            // cannot carry through: the exports it would publish are installed when the update
            // completes, so an export name another live stack of the Region already publishes,
            // and an exported value another live stack still IMPORTS that this update would
            // withdraw, fail it rather than the request, which was perfectly valid. Both are
            // decided by the WALK, where the deployment's changes are made and before it takes
            // down anything it supersedes — so they arrive here as its own failure
            // ([`crate::state::exports::ExportPublication`] records why that is the stage).
            let failure = materialised.failure;
            // Whether CloudFormation itself reached that failure before asking any owning
            // service for anything — a dynamic reference the deployment could not resolve —
            // which decides which report carries it
            // ([`crate::state::provisioning::Materialised::diagnosed`]).
            let diagnosed = materialised.diagnosed;
            if let Some(failure) = failure {
                // The deployment could not be carried out: the update does not reach
                // UPDATE_COMPLETE, and the stack reports what failed
                // ([`crate::state::provisioning::Materialised::failure`]) — the summary of
                // the resources an owning service refused, whose own refusals are on those
                // resources' events, or CloudFormation's own diagnostic where it refused the
                // deployment before asking anybody.
                //
                // The update is STILL IN FLIGHT, though: an operation is in flight from the
                // call that opened it until the clock makes its conclusion due, whichever way
                // that conclusion went, so this failure is held and reported at that instant
                // ([`crate::state::StoredStack::hold_failed_deployment`]) — which is what
                // leaves `CancelUpdateStack` the span it acts in, and what reports the
                // failure's own consequences: the stack rests in UPDATE_FAILED where its
                // caller asked for the failed state to be kept (`DisableRollback`) and is
                // rolled back to the previous stack configuration where it did not, with the
                // resources the update had already changed put back in the services that own
                // them.
                stack.hold_failed_deployment(
                    ::std::time::SystemTime::now(),
                    status::UPDATE_FAILED,
                    failure,
                    refused_by_validation,
                    diagnosed,
                    &materialised.transitions,
                );
            }
        }

        // Putting the previous configuration back is the rollback's own work in the services
        // that own the resources, and it is performed through the ONE path every rollback
        // this service owes goes through — with the store unlocked, and before the rollback's
        // own terminal status falls due.
        crate::state::settle_due_rollbacks(ctx, &*self.engine).await;

        // The transitions this update still owes — its conclusion, a wait that runs out, the
        // rollback a failure owes — and the status-change events each of them owes the
        // account's default event bus: carried out by this service itself, since the client
        // these events are for never reads the stack ([`crate::state::stack_progress`]).
        crate::state::stack_progress::drive_started_operation(
            ctx,
            &self.engine,
            &region,
            &stack_id,
        );

        Ok(output)
    }
}
