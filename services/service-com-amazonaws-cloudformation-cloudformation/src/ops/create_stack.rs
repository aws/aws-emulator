// SPDX-License-Identifier: CC0-1.0

//! @op CreateStack
//! IMPLEMENTED: allocates a stack id, persists the stack (template body verbatim,
//! parameters, tags, capabilities, notification ARNs, stack policy and the rest of the
//! request's configuration) in this service's store, records its `CREATE_IN_PROGRESS`
//! event and answers the stack id. The template's PROCESSED stage is then DEPLOYED: each
//! resource it declares becomes a stack-resource record carrying the physical resource id
//! CloudFormation's documented naming rules give it, each one reporting its own events,
//! and each declared output is evaluated against those resources and the stack's
//! parameters, which is what `DescribeStacks` reports once the create completes. A request
//! naming a live stack's name IN THE REGION IT IS SIGNED FOR is refused with the modelled
//! `AlreadyExistsException` — a stack name is a per-Region namespace, so the same name in
//! another Region is a second stack — one whose template declares or INVOKES a transform it
//! has not acknowledged with `CAPABILITY_AUTO_EXPAND` with the modelled
//! `InsufficientCapabilitiesException`, and one naming a `RoleARN` the account's IAM
//! records show CloudFormation cannot assume with the
//! service's `ValidationError` (see `crate::state::role_trust_refusal`).
//!
//! It then CREATES those resources in the services that own them, through this
//! emulator's own in-process callback: see [`crate::state::provisioning`] for which
//! resource types are created and what a type with no provisioner yet records instead. A
//! resource an owning service refuses fails the create with that service's own refusal as
//! the reason, and the stack is then ROLLED BACK — the resources the create had already
//! made are removed again and the stack settles into `ROLLBACK_COMPLETE` — because
//! "ROLLBACK — Roll back the stack creation" is `OnFailure`'s documented default. The
//! other two documented behaviours are honoured as documented: `DO_NOTHING` (which
//! `DisableRollback=true` also spells) leaves the stack `CREATE_FAILED` with everything
//! the create made, and `DELETE` takes the stack away with them.
//!
//! Members this handler stores for a LATER call to act on: `StackPolicyBody`, the stack
//! policy that governs which of the stack's resources an update may change (`UpdateStack`
//! and `ExecuteChangeSet` are held to it, `GetStackPolicy` reports it back) — a create
//! brings every resource into existence, so no action a policy governs is taken here.
//! Stored but not acted on: `ResourceTypes` (which resource types the stack's operations
//! may work with).
//! `DisableValidation` turns off pre-deployment validation.
//! `StackPolicyURL` names that same stack policy in Amazon S3: the object it addresses is
//! read from this emulator's own Amazon S3 and applied exactly as an inline body is.
//!
//! A `TemplateURL` says WHERE the template is rather than what it means, so the object it
//! addresses is read from this emulator's own Amazon S3 and the stack is created from it
//! exactly as from an inline body ([`crate::state::template_url`]).
//!
//! An `Fn::ImportValue` the template carries is resolved against the exported output values
//! in force in the account and Region the create runs in: a name no live stack of that
//! Region publishes has no value to import, so the create fails the STACK — nothing is asked
//! of any owning service, the reason names the export, and the stack rolls back — rather
//! than deploying a resource configured from a value that does not exist.
//!
//! `Capabilities` is WEIGHED against the template's own declarations: a template declaring
//! an IAM resource the request did not acknowledge is refused with the modelled
//! `InsufficientCapabilitiesException`, before the stack is created — see
//! [`crate::state::insufficient_capabilities`] — and so is one that names a MACRO without
//! `CAPABILITY_AUTO_EXPAND`, whether its `Transform` section names one for the whole
//! template or an `Fn::Transform` intrinsic names one for part of it. It is weighed against
//! the PROCESSED
//! template too, since a resource an included snippet contributes is one this stack would
//! deploy.
//!
//! The template is PROCESSED before it is deployed: an `Fn::Transform` naming
//! `AWS::Include` is the Amazon S3 object its `Location` names, read through this
//! emulator's own in-process callback and spliced into the template
//! ([`crate::state::include`]), and the transforms the template declares are then applied
//! to that spliced document ([`crate::state::template::process`]) — which is the stage
//! `GetTemplate(TemplateStage=Processed)` serves and the stack deploys. A snippet that
//! cannot be read fails the STACK — the create is accepted and answers a stack id, and the
//! stack then fails and rolls back carrying the transform's own reason, with nothing
//! provisioned.
//!
//! `ClientRequestToken` names the operation this create starts, and the stack remembers
//! it: a create repeating the stack's name under the token that already created it is the
//! RETRY that member is documented for, and it answers that stack's id again instead of
//! `AlreadyExistsException` — nothing is created twice. A retry is settled by the NAME and
//! the TOKEN alone: the body it carries is deliberately not compared with the create the
//! token claimed, because the token is what identifies the request ("a unique identifier
//! for this `CreateStack` request"), so a retry under a token already claimed by one of
//! that stack's OTHER operations is not a retry at all and is refused with the modelled
//! `TokenAlreadyExistsException`.
//!
//! `RollbackConfiguration` is held to the resource types a rollback trigger may name: a
//! trigger naming anything but the two CloudWatch alarm types the member documents is
//! refused with HTTP 400 `ValidationError` before anything is written, so no stack carries
//! rollback monitoring that could never fire
//! (`crate::state::validate_rollback_configuration`).
//!
//! The submitted template is VALIDATED before anything is written: a body whose own
//! declarations CloudFormation cannot resolve is refused with HTTP 400 `ValidationError`
//! and leaves no stack behind — a body neither reader will read, a function no template may
//! use, an intrinsic spelled with arguments it does not take, a `Ref` or an `Fn::GetAtt`
//! naming something the template does not declare, a `Condition` it does not declare, an
//! `Fn::FindInMap` whose lookup the `Mappings` section does not carry, and a dependency
//! cycle among the declared resources. See [`crate::state::template_validation`] for the
//! whole pass. A template whose `Resources` section declares a resource type CloudFormation
//! does not recognise is refused the same way, naming those types: see
//! [`crate::state::resource_types`] for the recognised set.
//!
//! What the template says about the DEPLOYMENT rather than about the request fails the
//! stack instead: a `Rules` assertion the parameter values in force do not satisfy, and an
//! `Fn::ForEach` in a section the declared transform does not define it for, are accepted
//! here and then end the create in `CREATE_FAILED`, which rolls back as `OnFailure`'s
//! default asks.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateStack::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `CreateStack`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Creates a stack as specified in the template. After the call completes successfully, the
//! stack creation starts. You can check the status of the stack through the DescribeStacks
//! operation. For more information about creating a stack and monitoring stack progress,
//! see Managing Amazon Web Services resources as a single unit with CloudFormation stacks
//! in the CloudFormation User Guide.
//!
//! ## Input
//!
//! * `Capabilities` (Capabilities) — In some cases, you must explicitly acknowledge that your
//!   stack template contains certain capabilities in order for CloudFormation to create the
//!   stack. CAPABILITY_IAM and CAPABILITY_NAMED_IAM Some stack templates might include
//!   resources that can affect permissions in your Amazon Web Services account; for example,
//!   by creating new IAM users. For those stacks, you must explicitly acknowledge this by
//!   specifying one of these capabilities. The following IAM resources require you to specify
//!   either the CAPABILITY_IAM or CAPABILITY_NAMED_IAM capability. If you have IAM resources,
//!   you can specify either capability. If you have IAM resources with custom names, you must
//!   specify CAPABILITY_NAMED_IAM. If …
//! * `ClientRequestToken` (ClientRequestToken, len 1..128, pattern
//!   ^[a-zA-Z0-9][-a-zA-Z0-9]*$) — A unique identifier for this CreateStack request. Specify
//!   this token if you plan to retry requests so that CloudFormation knows that you're not
//!   attempting to create a stack with the same name. You might retry CreateStack requests to
//!   ensure that CloudFormation successfully received them. All events initiated by a given
//!   stack operation are assigned the same client request token, which you can use to track
//!   operations. For example, if you execute a CreateStack operation with the token token1,
//!   then all the StackEvents generated by that operation will have ClientRequestToken set as
//!   token1. In the console, stack operations display the client request token on the Events
//!   tab. Stack operations that …
//! * `DeploymentConfig` (DeploymentConfig) — The deployment configuration for this stack
//!   operation, including the deployment mode.
//! * `DisableRollback` (DisableRollback) — Set to true to disable rollback of the stack if
//!   stack creation failed. You can specify either DisableRollback or OnFailure, but not
//!   both. Default: false
//! * `DisableValidation` (DisableValidation) — Set to true to disable pre-deployment
//!   validations in changeset or stack operations. Default: false
//! * `EnableTerminationProtection` (EnableTerminationProtection) — Whether to enable
//!   termination protection on the specified stack. If a user attempts to delete a stack with
//!   termination protection enabled, the operation fails and the stack remains unchanged. For
//!   more information, see Protect CloudFormation stacks from being deleted in the
//!   CloudFormation User Guide. Termination protection is deactivated on stacks by default.
//!   For nested stacks, termination protection is set on the root stack and can't be changed
//!   directly on the nested stack.
//! * `NotificationARNs` (NotificationARNs, len 0..5) — The Amazon SNS topic ARNs to publish
//!   stack related events. You can find your Amazon SNS topic ARNs using the Amazon SNS
//!   console or your Command Line Interface (CLI).
//! * `OnFailure` (OnFailure, one of: DELETE | DO_NOTHING | ROLLBACK) — Determines what action
//!   will be taken if stack creation fails. This must be one of: DO_NOTHING, ROLLBACK, or
//!   DELETE. You can specify either OnFailure or DisableRollback, but not both. Although the
//!   default setting is ROLLBACK, there is one exception. This exception occurs when a
//!   StackSet attempts to deploy a stack instance and the stack instance fails to create
//!   successfully. In this case, the CreateStack call overrides the default setting and sets
//!   the value of OnFailure to DELETE. Default: ROLLBACK
//! * `Parameters` (Parameters) — A list of Parameter structures that specify input parameters
//!   for the stack. For more information, see the Parameter data type.
//! * `ResourceTypes` (ResourceTypes) — Specifies which resource types you can work with, such
//!   as AWS::EC2::Instance or Custom::MyCustomInstance. If the list of resource types doesn't
//!   include a resource that you're creating, the stack creation fails. By default,
//!   CloudFormation grants permissions to all resource types. IAM uses this parameter for
//!   CloudFormation-specific condition keys in IAM policies. For more information, see
//!   Control CloudFormation access with Identity and Access Management. Only one of the
//!   Capabilities and ResourceType parameters can be specified.
//! * `RetainExceptOnCreate` (RetainExceptOnCreate) — When set to true, newly created
//!   resources are deleted when the operation rolls back. This includes newly created
//!   resources marked with a deletion policy of Retain. Default: false
//! * `RoleARN` (RoleARN, len 20..2048) — The Amazon Resource Name (ARN) of an IAM role that
//!   CloudFormation assumes to create the stack. CloudFormation uses the role's credentials
//!   to make calls on your behalf. CloudFormation always uses this role for all future
//!   operations on the stack. Provided that users have permission to operate on the stack,
//!   CloudFormation uses this role even if the users don't have permission to pass it. Ensure
//!   that the role grants least privilege. If you don't specify a value, CloudFormation uses
//!   the role that was previously associated with the stack. If no role is available,
//!   CloudFormation uses a temporary session that's generated from your user credentials.
//! * `RollbackConfiguration` (RollbackConfiguration) — The rollback triggers for
//!   CloudFormation to monitor during stack creation and updating operations, and for the
//!   specified monitoring period afterwards.
//! * `StackName` (StackName) **required** — The name that's associated with the stack. The
//!   name must be unique in the Region in which you are creating the stack. A stack name can
//!   contain only alphanumeric characters (case sensitive) and hyphens. It must start with an
//!   alphabetical character and can't be longer than 128 characters.
//! * `StackPolicyBody` (StackPolicyBody, len 1..16384) — Structure that contains the stack
//!   policy body. For more information, see Prevent updates to stack resources in the
//!   CloudFormation User Guide. You can specify either the StackPolicyBody or the
//!   StackPolicyURL parameter, but not both.
//! * `StackPolicyURL` (StackPolicyURL, len 1..5120) — Location of a file that contains the
//!   stack policy. The URL must point to a policy (maximum size: 16 KB) located in an S3
//!   bucket in the same Region as the stack. The location for an Amazon S3 bucket must start
//!   with https://. URLs from S3 static websites are not supported. You can specify either
//!   the StackPolicyBody or the StackPolicyURL parameter, but not both.
//! * `Tags` (Tags, len 0..50) — Key-value pairs to associate with this stack. CloudFormation
//!   also propagates these tags to the resources created in the stack. A maximum number of 50
//!   tags can be specified.
//! * `TemplateBody` (TemplateBody, len 1..) — Structure that contains the template body with
//!   a minimum length of 1 byte and a maximum length of 51,200 bytes. Conditional: You must
//!   specify either TemplateBody or TemplateURL, but not both.
//! * `TemplateURL` (TemplateURL, len 1..5120) — The URL of a file that contains the template
//!   body. The URL must point to a template (max size: 1 MB) that's located in an Amazon S3
//!   bucket or a Systems Manager document. The location for an Amazon S3 bucket must start
//!   with https://. URLs from S3 static websites are not supported. Conditional: You must
//!   specify either the TemplateBody or the TemplateURL parameter, but not both.
//! * `TimeoutInMinutes` (TimeoutMinutes, range 1..) — The amount of time that can pass before
//!   the stack status becomes CREATE_FAILED; if DisableRollback is not set or is set to
//!   false, the stack will be rolled back.
//!
//! ## Output
//!
//! * `OperationId` (OperationId) — A unique identifier for this stack operation that can be
//!   used to track the operation's progress and events.
//! * `StackId` (StackId) — Unique identifier of the stack.
//!
//! ## Errors
//!
//! * `AlreadyExistsException` (HTTP 400) — The resource with the name requested already
//!   exists.
//! * `InsufficientCapabilitiesException` (HTTP 400) — The template contains resources with
//!   capabilities that weren't specified in the Capabilities parameter.
//! * `LimitExceededException` (HTTP 400) — The quota for the resource has already been
//!   reached. For information about resource and stack limitations, see CloudFormation quotas
//!   in the CloudFormation User Guide.
//! * `TokenAlreadyExistsException` (HTTP 400) — A client request token already exists.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{status, StoredStack};

/// The `OnFailure` value that means the same thing as `DisableRollback=true` (the model
/// documents the two as alternative spellings: "You can specify either OnFailure or
/// DisableRollback, but not both").
const ON_FAILURE_DO_NOTHING: &str = "DO_NOTHING";

/// The `OnFailure` value that asks for the stack itself to go when its creation fails:
/// "DELETE — Delete the stack if stack creation failed."
const ON_FAILURE_DELETE: &str = "DELETE";

// `CreateStackInput$StackName`'s documented rule lives in
// `crate::state::validate_stack_name`, which `CreateChangeSet` holds a new stack's name
// to as well; `Parameters` are resolved by the shared
// `crate::state::merged_parameters`.

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateStackError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateStackError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#CreateStack")]
#[derive(Debug)]
pub enum CreateStackErrorModeled {
    AlreadyExistsException(AlreadyExistsException),
    InsufficientCapabilitiesException(InsufficientCapabilitiesException),
    LimitExceededException(LimitExceededException),
    TokenAlreadyExistsException(TokenAlreadyExistsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_stack::CreateStackError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateStackError = crate::scaffolded::Scaffolded<CreateStackErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateStackErrorModeled> for CreateStackError {
    fn from(modeled: CreateStackErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// What a create naming a stack name that is already IN USE by a live stack of the Region
/// answers, or `None` for a name no live stack of that Region carries.
///
/// "The name must be unique in the Region in which you are creating the stack" — unique
/// among the LIVE stacks OF THAT REGION: the same name in another Region is another stack
/// and both coexist, and a name is free again once its stack's deletion has completed. The
/// decision is this function's alone, so the create makes it in ONE place: once BEFORE the
/// template it submits is weighed — a request that cannot have the name is refused for the
/// name, whatever the body says — and again under the writing lock, where another request
/// may have taken the name since.
fn name_in_use(
    store: &crate::state::ComAmazonawsCloudformationCloudformationState,
    region: &str,
    input: &CreateStackInput,
) -> Option<Result<CreateStackOutput, CreateStackError>> {
    let live = store
        .live_stacks(region)
        .find(|live| live.stack_name == input.stack_name)?;
    // "Specify this token if you plan to retry requests so that CloudFormation knows that
    // you're not attempting to create a stack with the same name": a create repeating the
    // name AND the token the create of that live stack claimed is that retry, so it
    // re-answers the stack that create made — nothing is created a second time — rather
    // than refusing the name.
    let retried = input
        .client_request_token
        .as_deref()
        .and_then(|token| live.client_request_token_claim(token))
        .filter(|claimed| claimed.opened == status::CREATE_IN_PROGRESS);
    if let Some(claimed) = retried {
        return Some(Ok(CreateStackOutput {
            stack_id: Some(live.stack_id.clone()),
            operation_id: claimed.operation_id.clone(),
        }));
    }
    // Not that retry, but a token one of the stack's OTHER operations already claimed: a
    // second operation under a token that names another is the duplicate submission the
    // modelled `TokenAlreadyExistsException` refuses, whichever operation presents it.
    if let Some(message) =
        crate::state::reused_client_request_token(live, input.client_request_token.as_deref())
    {
        return Some(Err(CreateStackErrorModeled::TokenAlreadyExistsException(
            TokenAlreadyExistsException {
                message: Some(message),
            },
        )
        .into()));
    }
    Some(Err(CreateStackErrorModeled::AlreadyExistsException(
        AlreadyExistsException {
            message: Some(format!("Stack [{}] already exists", input.stack_name)),
        },
    )
    .into()))
}

/// The `CreateStack` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateStack<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateStack<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateStack<Eng>
{
    type Input = CreateStackInput;
    type Output = CreateStackOutput;
    type Error = CreateStackError;

    const NAME: &'static str = "CreateStack";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_STACK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_STACK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Conditional: "You must specify either TemplateBody or TemplateURL, but not
        // both" (`CreateStackInput$TemplateBody`).
        let template_body = match (&input.template_body, &input.template_url) {
            (Some(body), None) => body.clone(),
            (Some(_), Some(_)) => {
                return Err(crate::scaffolded::Scaffolded::validation_error(
                    crate::state::BOTH_TEMPLATE_SOURCES,
                ))
            }
            (None, None) => {
                return Err(crate::scaffolded::Scaffolded::validation_error(
                    "Either TemplateBody or TemplateURL must be specified",
                ))
            }
            // A `TemplateURL` says WHERE the template is, not what it means: the object it
            // addresses is read from this emulator's own Amazon S3 and becomes the body
            // this create deploys, so a template submitted by reference and the same one
            // submitted inline create the same stack (`crate::state::template_url`).
            (None, Some(url)) => crate::state::template_url::fetch(
                ctx,
                &crate::state::request_region(ctx.parts()),
                url,
            )
            .await
            .map_err(crate::scaffolded::Scaffolded::validation_error)?,
        };
        // "You can specify either DisableRollback or OnFailure, but not both."
        if input.disable_rollback.is_some() && input.on_failure.is_some() {
            return Err(crate::scaffolded::Scaffolded::validation_error(
                "You cannot specify both OnFailure and DisableRollback",
            ));
        }
        // "Specify either AWS::CloudWatch::Alarm or AWS::CloudWatch::CompositeAlarm
        // resource types": a trigger naming anything else is refused HERE, before anything
        // is written, so no stack is created carrying rollback monitoring that could never
        // fire (`crate::state::validate_rollback_configuration`).
        crate::state::validate_rollback_configuration(input.rollback_configuration.as_ref())
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // A stack policy reaches CloudFormation either inline, as the request's own
        // `StackPolicyBody`, or by reference, as a `StackPolicyURL` naming "a policy
        // (maximum size: 16 KB) located in an Amazon S3 bucket in the same Amazon Web
        // Services Region as the stack" — and "you can specify either the StackPolicyBody
        // or the StackPolicyURL parameter, but not both".
        crate::state::stack_policy::one_policy_source(
            "StackPolicyBody",
            "StackPolicyURL",
            input.stack_policy_body.as_deref(),
            input.stack_policy_url.as_deref(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The reference says WHERE the policy document is, not what it means: the object it
        // addresses is read from this emulator's own Amazon S3 and becomes the policy this
        // stack carries, so a policy submitted by reference and the same one submitted
        // inline protect the stack alike (`crate::state::stack_policy::fetch`).
        let stack_policy_body = match &input.stack_policy_url {
            Some(url) => Some(
                crate::state::stack_policy::fetch(
                    ctx,
                    &crate::state::request_region(ctx.parts()),
                    "StackPolicyURL",
                    "StackPolicyBody",
                    url,
                )
                .await
                .map_err(crate::scaffolded::Scaffolded::validation_error)?,
            ),
            None => input.stack_policy_body.clone(),
        };
        crate::state::validate_stack_name(&input.stack_name)
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // A tag key in AWS's own reserved namespace is not a tag a caller may set
        // (`crate::state::stack_tags`): refused HERE, so nothing is created carrying a key
        // the service itself owns.
        crate::state::stack_tags::validate_optional_tags(input.tags.as_deref())
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The documented `TemplateBody` maximum, which the model carries no `@length` for:
        // a stored body is caller-sized otherwise.
        crate::state::validate_template_body(&template_body)
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // "Only one of the Capabilities and ResourceType parameters can be specified": the
        // two members state two different bounds on this create, so a request stating both
        // states no one bound and is refused for its SHAPE, before either bound is weighed
        // (`crate::state::resource_type_allow_list`).
        crate::state::resource_type_allow_list::one_bound(
            input.capabilities.as_deref().unwrap_or_default(),
            input.resource_types.as_deref().unwrap_or_default(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // This service was mounted on an engine that is not the shared in-memory
            // one, so there is no store to create the stack in: nothing was written, and
            // 501 is the only honest answer.
            return Err(crate::scaffolded::Scaffolded::not_implemented("CreateStack"));
        };
        // "The name must be unique in the Region in which you are creating the stack": the
        // name is a property of the REQUEST, so a create naming a live stack of this Region
        // is refused with the modelled `AlreadyExistsException` HERE — before the template
        // it submits is weighed at all — and the stack that already has the name is left
        // exactly as it was. A create weighing the body first would answer a request that
        // cannot have the name for something about its template instead ([`name_in_use`]).
        {
            let mut store = state.lock();
            // Materialises only the transitions the clock already owed, which would have
            // happened on any other request at this instant — among them the deletion that
            // frees a name again.
            store.settle(::std::time::SystemTime::now());
            if let Some(answered) =
                name_in_use(&store, &crate::state::request_region(ctx.parts()), &input)
            {
                return answered;
            }
        }
        // "If the list of resource types doesn't include a resource that you're creating,
        // the stack creation fails": the bound this request put on its own deploy is
        // weighed against the template it submits, before anything is created
        // (`crate::state::resource_type_allow_list`).
        crate::state::validate_allowed_resource_types(
            &template_body,
            input.resource_types.as_deref().unwrap_or_default(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // "If your stack template contains one or more macros, and you choose to create a
        // stack directly from the processed template [...] you must acknowledge this
        // capability" — the modelled refusal for the request that has not.
        let missing = crate::state::missing_auto_expand_capability(
            &template_body,
            input.capabilities.as_deref().unwrap_or_default(),
        );
        if !missing.is_empty() {
            return Err(CreateStackErrorModeled::InsufficientCapabilitiesException(
                InsufficientCapabilitiesException {
                    message: Some(crate::state::template::insufficient_capabilities_message(
                        &missing,
                    )),
                },
            )
            .into());
        }
        // The documented per-template quotas, which `DescribeAccountLimits` advertises as
        // this account's limits: a body declaring more than one of them allows is refused
        // HERE rather than created as a stack holding none of what it declares.
        crate::state::validate_template_quotas(&template_body)
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // "Creates a stack as specified in the template" — only a template CloudFormation
        // can deploy: a body declaring a resource type it does not recognise is refused
        // HERE, before the stack is created, so a template real AWS refuses outright is not
        // driven to CREATE_COMPLETE against resources nobody created. See
        // `crate::state::resource_types` for the recognised set.
        crate::state::validate_resource_types(&template_body)
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // "In some cases, you must explicitly acknowledge that your stack template contains
        // certain capabilities in order for CloudFormation to create the stack": a template
        // whose IAM resources this request did not acknowledge is refused HERE, before the
        // stack is created, so nothing mints a privileged identity the caller never
        // acknowledged.
        if let Some(message) = crate::state::insufficient_capabilities(
            &template_body,
            input.capabilities.as_deref().unwrap_or_default(),
        ) {
            return Err(CreateStackErrorModeled::InsufficientCapabilitiesException(
                InsufficientCapabilitiesException {
                    message: Some(message),
                },
            )
            .into());
        }
        // `Parameter$UsePreviousValue` is a stack-UPDATE instruction ("During a stack
        // update, use the existing parameter value that the stack is using for a given
        // parameter key"), so a create — which has no previous values at all — is refused
        // rather than silently storing an empty one. That is the shared resolution with an
        // empty previous set.
        let parameters = crate::state::merged_parameters(&[], input.parameters.as_deref())
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // What the stack reports as `Parameters`: the values the request supplied, plus
        // the template's own `Default` for a declared parameter it passed none for.
        // Resolved HERE because the template's own processing reads them — a
        // language-extensions directive counting a list-typed parameter's items among them.
        let mut parameters = crate::state::parameters_in_force(&template_body, parameters);
        // A parameter the template declares with an SSM-typed `Type` carries the NAME of a
        // Parameter Store parameter, not a value: the value the store holds is read HERE,
        // before anything is written or deployed, so every expression the template builds
        // from the parameter carries that value while the stack goes on reporting the name
        // as `Parameter$ParameterValue` (`crate::state::ssm_parameters`).
        crate::state::ssm_parameters::resolve(
            ctx,
            &crate::state::request_region(ctx.parts()),
            &template_body,
            &mut parameters,
        )
        .await;
        let parameter_values = crate::state::parameter_value_map(&parameters);
        // Applying the template's transforms is caller-sized work, so it happens HERE —
        // before the store lock below — and never while every other request waits on that
        // lock. The stage it computes is what the stack deploys.
        let processed_template =
            crate::state::template::process_with_parameters(&template_body, &parameter_values);
        // "Creates a stack as specified in the template" — a template CloudFormation
        // VALIDATES first: a body whose own declarations it cannot resolve is refused HERE,
        // with nothing created, rather than accepted with a stack id and then deployed into
        // a stack whose resources carry nulls (`crate::state::template_validation`).
        crate::state::validate_submitted_template(
            &template_body,
            &processed_template,
            &parameter_values,
            &crate::state::request_region(ctx.parts()),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The `Parameters` section is the template's input CONTRACT, so the parameter set
        // this create resolved is held to it before anything is created: a declared
        // parameter with no `Default` and no value in force has no value at all, and a key
        // the request named that the template declares nothing for answers nothing in it
        // (`crate::state::parameter_constraints`). Weighed after the rules above, since a
        // `Parameters` key that is no parameter NAME declares no parameter to have a value.
        crate::state::validate_parameter_set(
            &template_body,
            &parameters,
            input.parameters.as_deref(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // "Rules: validates a parameter or a combination of parameters passed to a template
        // during a stack creation": an assertion that does not hold for the values in force
        // is not a bad request — it fails the create this call opens, which then rolls back,
        // and so does a directive the declared transform cannot apply where it stands.
        let deployment_failure = crate::state::template_validation::deployment_failure(
            &template_body,
            &parameter_values,
        );

        // "CloudFormation uses the role's credentials to make calls on your behalf": a
        // role whose trust policy does not let cloudformation.amazonaws.com assume it is
        // one the stack could never deploy under, so the request is refused before
        // anything is written.
        crate::state::refuse_unassumable_role(&*self.engine, input.role_arn.as_deref())?;
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx.parts());

        // "CloudFormation processes the template fragments" a transform names BEFORE it
        // does anything with the template's resources: an `Fn::Transform` of `AWS::Include`
        // is the Amazon S3 object its `Location` names, read and spliced in here, so what
        // deploys below is the PROCESSED template and no intrinsic reaches the service that
        // owns a resource. A snippet that cannot be read fails this stack rather than this
        // request — "after the call completes successfully, the stack creation starts", and
        // the processing is part of that creation.
        let processing = crate::state::include::process(ctx, &region, &template_body).await;
        // The capabilities are weighed again over the PROCESSED template: a resource a
        // snippet CONTRIBUTES is as much the stack's as one the body declares itself, so
        // the acknowledgement this request carries is held to the template that really
        // deploys — nothing mints a privileged identity a caller never acknowledged because
        // an include is what brought it in.
        if let crate::state::include::Processed::Expanded(processed) = &processing {
            if let Some(message) = crate::state::insufficient_capabilities(
                processed,
                input.capabilities.as_deref().unwrap_or_default(),
            ) {
                return Err(CreateStackErrorModeled::InsufficientCapabilitiesException(
                    InsufficientCapabilitiesException {
                        message: Some(message),
                    },
                )
                .into());
            }
        }

        // The stack record, built before the store is locked: reading a caller-sized
        // template body is work the request paid for, and doing it outside the critical
        // section keeps one caller's template from holding the lock every other
        // CloudFormation request needs. The stack id is minted here too (it is drawn, not
        // read from the store), so nothing below the lock has to compute anything from the
        // body.
        let operation_id = crate::state::new_operation_id();
        // "All events triggered by a given stack operation are assigned the same client
        // request token": the caller's token when it named one, otherwise the one
        // CloudFormation assigns the operation itself.
        let client_request_token = input
            .client_request_token
            .clone()
            .unwrap_or_else(crate::state::new_client_request_token);
        let mut stack = StoredStack::new_processed(
            crate::state::new_stack_id(&region, &input.stack_name),
            input.stack_name.clone(),
            Some(template_body),
            processed_template,
            now,
        );
        stack.region = region.clone();
        // What the stack reports as `Parameters`, resolved above: the values the request
        // supplied, plus the template's own `Default` for a declared parameter it passed
        // none for.
        stack.parameters = parameters;
        stack.capabilities = input.capabilities.clone().unwrap_or_default();
        stack.tags = crate::state::stored_tags(input.tags.as_deref().unwrap_or_default());
        stack.notification_arns = input.notification_ar_ns.clone().unwrap_or_default();
        stack.role_arn = input.role_arn.clone();
        stack.timeout_in_minutes = input.timeout_in_minutes;
        // "Specifies whether to disable rollback of the stack if the stack operation
        // fails" is what BOTH `DisableRollback` and `DeploymentConfig$DisableRollback`
        // say, so the create runs with rollback disabled when either states it
        // ([`crate::state::rollback_disabled`]). A create that states neither is answered
        // by the failure action it asked for ("DO_NOTHING … is equivalent to specifying
        // true for the DisableRollback parameter"), and only a create that states none of
        // the three by the default of the mode it runs in.
        stack.disable_rollback = crate::state::rollback_disabled(
            input.disable_rollback,
            input.deployment_config.as_ref(),
        )
        .unwrap_or_else(|| match input.on_failure.as_deref() {
            Some(on_failure) => on_failure == ON_FAILURE_DO_NOTHING,
            None => crate::state::mode_disables_rollback(
                input
                    .deployment_config
                    .as_ref()
                    .and_then(|config| config.mode.as_deref()),
            ),
        });
        stack.enable_termination_protection =
            input.enable_termination_protection.unwrap_or(false);
        stack.retain_except_on_create = input.retain_except_on_create;
        stack.deployment_config =
            crate::state::stored_deployment_config(input.deployment_config.as_ref());
        stack.rollback_configuration =
            crate::state::stored_rollback_configuration(input.rollback_configuration.as_ref());
        stack.client_request_token = Some(client_request_token.clone());
        stack.stack_policy_body = stack_policy_body;
        stack.resource_types = input.resource_types.clone().unwrap_or_default();

        // What the template's processing left: the processed body is the one the stack
        // deploys, and a processing FAILURE is the stack's own failure reason. The
        // transforms the template declares are applied to the SPLICED document, so the
        // stage served and deployed is the whole pipeline's — and, like the first pass
        // above, that expansion happens before the store lock.
        let transform_failure = match processing {
            crate::state::include::Processed::Unchanged => None,
            crate::state::include::Processed::Expanded(processed) => {
                stack.processed_template = crate::state::template::process_with_parameters(
                    &processed,
                    &parameter_values,
                );
                stack.set_processed_template_body(processed);
                None
            }
            crate::state::include::Processed::Failed(reason) => Some(reason),
        }
        // A `Rules` assertion the values in force do not satisfy, or a directive the
        // declared transform cannot apply where it stands, fails this create the same way:
        // the stack is created, deploys nothing, and settles into the rollback below
        // carrying that reason.
        .or(deployment_failure);

        // Everything the template's resources and outputs are computed from is now in
        // place: materialise the resources its PROCESSED stage declares and evaluate its
        // outputs, which `DescribeStacks` reports once the create completes. A template
        // this service cannot deploy at all answers the reason here, and the create below
        // ends in the modelled CREATE_FAILED with it.
        //
        // What a `Fn::ImportValue` resolves against is read first, under its own short
        // lock: the exported output values in force in this Region, beside the outputs an
        // `Fn::GetStackOutput` reads off the stacks of that Region. Evaluating the
        // caller-sized template then happens off that lock, as everything above it does.
        let (exports, stack_outputs, managed_names) = {
            let mut store = state.lock();
            store.settle(now);
            (
                store.exported_values(&region),
                store.stack_output_values(&region),
                // ...and which names the live stacks of the Region already manage, read in
                // the same breath: a name one of them holds is CloudFormation's own to
                // report on the resource rather than this stage's to fail
                // ([`crate::state::early_validation::named_additions`]). The index the WALK
                // is held to is read again under the writing lock below, since that is the
                // one it must be current with.
                store.managed_physical_names(&region),
            )
        };
        let cannot_deploy = if transform_failure.is_some() {
            // Processing the template failed, so the create never reached the stage that
            // provisions anything: the stack declares no resource and no output, and the
            // reason below is the transform's rather than an owning service's.
            stack.deploy_nothing();
            None
        } else {
            stack.deploy_template(&exports, &stack_outputs).err()
        };
        // "The intrinsic function Fn::ImportValue returns the value of an output exported by
        // another stack": an export name no live stack of this account and Region publishes
        // has no value to import, so the declaration naming it cannot be deployed at all.
        // That is CLOUDFORMATION's own refusal of the deployment, reported the way the other
        // refusals it makes before asking anybody are — nothing is created, and the create
        // fails and rolls back naming the export rather than completing a stack whose
        // resource carries no imported value.
        let unresolved_cross_stack = stack.unresolved_cross_stack_failure();
        // "Set to true to disable pre-deployment validations in [...] stack operations",
        // default false: with them ON, a declaration whose properties its type's contract
        // does not admit — a required property it does not carry, a value outside the set the
        // property publishes — fails the create before anything is provisioned: nothing is
        // asked of the service that owns the resource, so no resource of this stack is
        // created and none reports an event of its own. A create that disables them deploys
        // the declaration as written and the same problem surfaces as that resource's own
        // failure ([`crate::state::property_validation`]).
        //
        // The stage judges the DEPLOYED property document, in which a value this service
        // could not evaluate is absent, so it is not asked at all about a deployment
        // CloudFormation has already refused for a reference it could not resolve: that
        // refusal is the one diagnostic for every property whose value it took away, and
        // reporting a property as missing instead would name the symptom rather than the
        // cause.
        let property_failure = (input.disable_validation != Some(true)
            && unresolved_cross_stack.is_none())
            .then(|| crate::state::property_validation::failure(&stack.current_resources()))
            .flatten();
        let refused_before_validation = transform_failure.or(unresolved_cross_stack);
        // The OTHER pre-deployment validation stage, over the same `DisableValidation`
        // switch: the resource EXISTENCE check. A resource this create would make under a
        // physical name the service that owns the type ALREADY holds cannot be created, so
        // the create fails at this stage — nothing is asked of any owning service, no
        // resource of this stack is created, and the deployment is refused by
        // CloudFormation itself rather than by the service that would have held the name
        // ([`crate::state::early_validation`]).
        //
        // The questions are put HERE, with no store lock held, because each one reaches the
        // service that owns the type — exactly where `CreateChangeSet` puts them for the
        // plan it offers. A deployment already refused above is not asked about at all: that
        // refusal is its one diagnostic.
        let name_conflict = if input.disable_validation == Some(true)
            || refused_before_validation.is_some()
            || property_failure.is_some()
        {
            None
        } else {
            let candidates = crate::state::early_validation::named_additions(
                stack.processed_stage_body(),
                crate::state::template::StackScope {
                    region: &region,
                    stack_name: &stack.stack_name,
                    stack_id: &stack.stack_id,
                    notification_arns: &stack.notification_arns,
                },
                &parameter_values,
                // A create manages nothing yet: every resource its template declares is one
                // it would create.
                &[],
                &managed_names,
            );
            crate::state::early_validation::taken_names(ctx, &region, &candidates)
                .await
                .into_iter()
                .next()
        };
        // What a failed pre-deployment validation refuses the deployment with: the
        // validation's own reason, which is what the stack then reports it failed for.
        let failed_validation = property_failure
            .as_ref()
            .map(|failure| failure.reason.clone())
            .or_else(|| {
                name_conflict
                    .as_ref()
                    .map(|conflicting| {
                        crate::state::early_validation::name_conflict_failure(conflicting).reason
                    })
            });
        // ...and whether that stage is what refused THIS deployment, known here rather than
        // re-derived from the reason text below: a refusal reached before it stands instead.
        let refused_by_validation =
            refused_before_validation.is_none() && failed_validation.is_some();
        let refused_deployment = refused_before_validation.or(failed_validation);

        // The store lock is taken for the write and RELEASED at the end of this block:
        // the calls that create the stack's resources run after it, and each of those
        // reaches another service's own handler, which takes that service's lock — a
        // client reading THIS service meanwhile must not be blocked behind them.
        let (stack_id, declared, provenance) = {
            let mut store = state.lock();
            store.settle(now);

            // "The name must be unique in the Region in which you are creating the stack":
            // re-asserted HERE, under the writing lock, rather than trusted from the check
            // this handler made before it weighed the template — between the two, another
            // request may have taken the name ([`name_in_use`] is the one decision).
            if let Some(answered) = name_in_use(&store, &region, &input) {
                return answered;
            }

            // "The quota for the resource has already been reached": the store holds at
            // most `MAX_STACKS` live stacks PER REGION, which is the quota CloudFormation
            // meters ("no more than N stacks can exist in this account and Region") and
            // what bounds its growth. Nothing above this point WROTE a stack, so a refused
            // request adds none — the one change it can leave behind is `settle`'s, which
            // only materialises the transitions the clock already owed and would have
            // happened on any other request at this instant.
            if store.live_stacks(&region).count() >= crate::state::MAX_STACKS {
                return Err(CreateStackErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(format!(
                            "Limit for stacks exceeded: no more than {} stacks can exist in \
                             this account and Region",
                            crate::state::MAX_STACKS
                        )),
                    },
                )
                .into());
            }
            // The Region that quota is metered in comes from the request's own credential
            // scope, so it is a value the CALLER chooses: this is the ceiling on the store
            // as a whole, which is what bounds its growth however many Regions a caller
            // signs for.
            if store.live_stack_count() >= crate::state::MAX_LIVE_STACKS {
                return Err(CreateStackErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(format!(
                            "Limit for stacks exceeded: no more than {} stacks can exist in \
                             this account",
                            crate::state::MAX_LIVE_STACKS
                        )),
                    },
                )
                .into());
            }

            // The same quota, counted in RESOURCES rather than stacks: CloudFormation
            // meters concurrent resources per account ("CloudFormation has the following
            // limits per account: Number of concurrent resources"), and this create is
            // what would bring the template's resources into the account. Checked on the
            // same `LimitExceededException` channel and in the same place, so a create
            // past either ceiling writes nothing — and so the ceiling
            // `DescribeAccountLimits` reports as `ConcurrentResourcesLimit` is the one a
            // caller is really held to.
            if store.current_resource_count() + stack.current_resource_count()
                > crate::state::MAX_CONCURRENT_RESOURCES
            {
                return Err(CreateStackErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(format!(
                            "Limit for concurrent resources exceeded: no more than {} \
                             resources can exist across the stacks of this account",
                            crate::state::MAX_CONCURRENT_RESOURCES
                        )),
                    },
                )
                .into());
            }

            // "After the call completes successfully, the stack creation starts": the
            // stack is answered in CREATE_IN_PROGRESS and settles to CREATE_COMPLETE once
            // the clock makes that due. `begin_transition` is what records the operation
            // the stack reports as its last, and the token every event of this operation
            // carries.
            stack.begin_transition(
                now,
                status::CREATE_IN_PROGRESS,
                status::CREATE_COMPLETE,
                Some(operation_id.clone()),
                Some(client_request_token.clone()),
            );
            // A pre-deployment validation this create's resources did not pass is reported
            // as the validation result it is: an event about the resource NOTHING was done
            // to, naming the validation, its status and what failed it — which is the detail
            // `DescribeEvents` reports for a failure CloudFormation caught before
            // provisioning ([`crate::state::property_validation`]).
            if let Some(failure) = &property_failure {
                stack.record_validation_failure(
                    now,
                    &failure.logical_id,
                    &failure.resource_type,
                    crate::state::StoredValidationFailure {
                        name: crate::state::property_validation::PROPERTY_VALIDATION.to_string(),
                        status: crate::state::early_validation::VALIDATION_FAILED.to_string(),
                        reason: failure.reason.clone(),
                        path: failure.path.clone(),
                    },
                    Some(operation_id.clone()),
                    Some(client_request_token.clone()),
                );
            }
            // ...and so is the existence check's own failure, about the resource whose
            // identifier the owning service already holds: which identifier of which type is
            // taken, at the element of the template that declared it
            // ([`crate::state::early_validation`]).
            if let Some(conflicting) = &name_conflict {
                stack.record_validation_failure(
                    now,
                    &conflicting.logical_id,
                    &conflicting.resource_type,
                    crate::state::early_validation::name_conflict_failure(conflicting),
                    Some(operation_id.clone()),
                    Some(client_request_token.clone()),
                );
            }
            // The create was accepted — the request is valid and the stack exists — but
            // this service cannot deploy that template, so the operation it started ends
            // in the modelled CREATE_FAILED with the reason, never in a CREATE_COMPLETE
            // reporting resources nothing created.
            if let Some(reason) = cannot_deploy {
                stack.fail_pending_transition(status::CREATE_FAILED, reason);
            }
            let stack_id = stack.stack_id.clone();
            // How this emulator is reached, kept for the work a provider's answer owes once
            // it lands out of band ([`crate::state::provider_driver`]): the requests this
            // create is about to publish are answered at a plane that is not a handler, and
            // what the answer then makes due — the next resource in dependency order, the
            // rollback of a resource the provider failed — is a call only a handler can
            // make. Remembered BEFORE the walk below publishes anything, so a provider that
            // answers the very first request of a stack nobody has read yet still drives
            // what that answer owes.
            store.remember_provisioning_reach(ctx);
            // What the template declared, as the deployment computed it: the resources
            // whose owning services now have to create them, and the stack identity and
            // tags CloudFormation tags each of them with.
            let declared = stack.current_resources().to_vec();
            // ...and which names the OTHER live stacks of the Region already manage, read
            // here because the walk below holds no lock: a resource whose name one of them
            // owns fails in CloudFormation's own words
            // ([`crate::state::physical_name`]).
            let provenance = crate::state::provisioning::StackProvenance {
                managed_names: store.managed_physical_names(&region),
                ..stack.provenance()
            };
            store.stacks.push(stack);
            (stack_id, declared, provenance)
        };

        // "Creates a stack as specified in the template": each declared resource is
        // created in the service that owns it, and what that service answered is what the
        // stack records for it. A create has nothing to reconcile against, so the previous
        // set is empty.
        //
        // A deployment CLOUDFORMATION refused — a template whose transforms could not be
        // applied, a `Rules` assertion the values in force do not satisfy, or an
        // `Fn::ImportValue` naming an export nothing publishes — is passed that refusal, so
        // nothing is asked of any owning service and the stack records CloudFormation's own
        // diagnostic rather than a summary of resources that tried and failed.
        // Every call below is made under the stack's service role: "CloudFormation uses
        // the role's credentials to make calls on your behalf", so a call that role is
        // denied is not made at all ([`crate::state::service_role`]). The same role governs
        // the rollback further down, which is the same stack's work.
        let role =
            crate::state::service_role::ServiceRole::of(&*self.engine, input.role_arn.as_deref());
        let materialised = crate::state::service_role::acting_as(
            ::std::sync::Arc::clone(&role),
            crate::state::provisioning::reconcile(
                ctx,
                &region,
                &provenance,
                declared,
                &[],
                refused_deployment,
                // A create removes nothing of its own; where it does take a resource down —
                // its own rollback — that walk is the one below, under the policies the
                // request asked for.
                crate::state::provisioning::Policies::Honoured,
            ),
        )
        .await;

        let output = CreateStackOutput {
            stack_id: Some(stack_id.clone()),
            operation_id: Some(operation_id),
        };
        // What the failure (if any) owes the stack beyond the CREATE_FAILED it records:
        // the resources to remove again, and the status that removal settles the stack
        // into.
        let unwind = {
            let mut store = state.lock();
            // Which live stack of this Region already publishes each exported output value,
            // read BEFORE the stack is resolved for writing: "export names must be unique
            // within a Region", so a name another stack holds is one this create cannot
            // publish ([`crate::state::StoredStack::conflicting_export`]).
            let publishers = store.exporting_stacks(&region);
            // Unreachable unless the stack was deleted while its resources were being
            // created: there is then nothing left to record them on.
            let Some(stack) = store.resolve_mut(&region, &stack_id) else {
                return Ok(output);
            };
            // A resource whose owning service has not reported it ready yet holds this
            // operation open ([`crate::state::stabilising`]): recorded BEFORE the events
            // below, since that is what withholds the transition that would report the
            // resource created.
            stack.record_stabilisations(&materialised.stabilising, now);
            // The resources' own transitions come BEFORE the stack's terminal event: the
            // event stream is what the deployment did, in the order it did it.
            stack.schedule_resource_events(&materialised.transitions);
            stack.record_materialised_resources(materialised.resources);
            // A deployment whose resources were all created can still be one CloudFormation
            // cannot carry through: the exports it would publish are installed when the
            // create completes, so an export name another live stack of the Region already
            // publishes fails it HERE — after the walk, with what the create made in place —
            // rather than at the request, which was perfectly valid.
            let failure = materialised
                .failure
                .or_else(|| stack.conflicting_export(&publishers));
            match failure {
                None => None,
                Some(failure) => {
                    // The deployment could not be carried out: the stack never reaches
                    // CREATE_COMPLETE, and it reports what failed
                    // ([`crate::state::provisioning::Materialised::failure`]) — the
                    // summary of the resources an owning service refused to create, whose
                    // own refusals are on those resources' events, or CloudFormation's own
                    // diagnostic where it refused the deployment before asking anybody (a
                    // dynamic reference it could not resolve), which no resource's event
                    // could carry because no resource was ever asked for.
                    let failed_at = ::std::time::SystemTime::now();
                    // "ROLLBACK — Roll back the stack creation. This is the default": the
                    // failed create does not stop there. What the caller asked for decides
                    // which of the three documented `OnFailure` behaviours it gets —
                    // DO_NOTHING (which `DisableRollback` also spells) keeps the failed
                    // state and everything the create had already made, DELETE takes the
                    // stack itself away, and ROLLBACK removes what the create made and
                    // leaves the stack reporting that it was rolled back.
                    //
                    // That same answer decides how the failure is REPORTED: only the stack
                    // that is left in `CREATE_FAILED` rests in it, so only that one carries
                    // a stack-level event for it; where the removal begins at this very
                    // instant, the failure is reported by the rollback's own event
                    // ([`crate::state::StoredStack::fail_transition_superseded`]).
                    //
                    // A failure the PRE-DEPLOYMENT VALIDATION stage reached is the one
                    // exception: it ran before anything was asked of any owning service, so
                    // no resource has an event that could carry it and the stack's own
                    // `CREATE_FAILED` event reports it either way, with the detailed status
                    // the model gives a validation failure
                    // ([`crate::state::StoredStack::fail_transition_on_validation`]).
                    if stack.disable_rollback {
                        if refused_by_validation {
                            stack.fail_transition_on_validation(
                                failed_at,
                                status::CREATE_FAILED,
                                failure,
                                true,
                            );
                        } else {
                            stack.fail_transition(failed_at, status::CREATE_FAILED, failure);
                        }
                        // The failed state is KEPT, so what the create DID make is what the
                        // stack goes on reporting, each resource in the status the walk left
                        // it ([`crate::state::StoredStack::keep_failed_resources`]).
                        stack.keep_failed_resources(&materialised.transitions, failed_at);
                        None
                    } else {
                        if refused_by_validation {
                            stack.fail_transition_on_validation(
                                failed_at,
                                status::CREATE_FAILED,
                                failure.clone(),
                                false,
                            );
                        } else {
                            stack.fail_transition_superseded(
                                failed_at,
                                status::CREATE_FAILED,
                                failure.clone(),
                            );
                        }
                        let deletes_the_stack =
                            input.on_failure.as_deref() == Some(ON_FAILURE_DELETE);
                        if deletes_the_stack {
                            stack.begin_transition(
                                failed_at,
                                status::DELETE_IN_PROGRESS,
                                status::DELETE_COMPLETE,
                                Some(crate::state::new_operation_id()),
                                input.client_request_token.clone(),
                            );
                        } else {
                            stack.begin_rollback(failed_at, failure);
                        }
                        // With the records the stack HOLDS, which is the set the removal
                        // reports a deletion of ([`crate::state::provisioning::unwind`]),
                        // and what that removal does with the `DeletionPolicy` of a resource
                        // this create made.
                        Some((
                            stack.current_resources(),
                            stack.held_resource_ids(),
                            crate::state::provisioning::dependencies_in_flight(stack),
                            stack.rolled_back_resource_policies(),
                        ))
                    }
                }
            }
        };

        // The resources the failed create had already made in the services that own them
        // are removed again, with the store UNLOCKED: each delete reaches another service's
        // own handler, exactly as the creates above did. The status the removal settles
        // into is already due on the stack's clock; only a delete the owning service
        // REFUSES changes it, and then to the failed status for that unwind rather than to
        // one that claims the resources are gone.
        if let Some((resources, held, needs, policies)) = unwind {
            let deleted = crate::state::service_role::acting_as(
                role,
                crate::state::provisioning::unwind(
                    ctx,
                    &region,
                    &resources,
                    &crate::state::provisioning::Teardown {
                        stack_id: &stack_id,
                        held: &held,
                        needs: &needs,
                        on_refusal: crate::state::provisioning::OnRefusal::Stop,
                        policies,
                    },
                ),
            )
            .await;
            let mut store = state.lock();
            if let Some(stack) = store.resolve_mut(&region, &stack_id) {
                // A resource whose owning service accepted the delete and still holds it
                // holds this rollback open ([`crate::state::stabilising`]): it is not
                // reported complete while a resource it removed is still there.
                stack.record_stabilisations(
                    &deleted.removing,
                    ::std::time::SystemTime::now(),
                );
                stack.schedule_resource_events(&deleted.transitions);
                stack.record_materialised_resources(deleted.resources);
                if let Some(failure) = deleted.failure {
                    let failed_status = if stack.status == status::DELETE_IN_PROGRESS {
                        status::DELETE_FAILED
                    } else {
                        status::ROLLBACK_FAILED
                    };
                    stack.fail_transition(
                        ::std::time::SystemTime::now(),
                        failed_status,
                        failure,
                    );
                }
            }
        }

        Ok(output)
    }
}
