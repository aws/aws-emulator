// SPDX-License-Identifier: CC0-1.0

//! @op CreateChangeSet
//! IMPLEMENTED: persists a change set — its template, parameters, tags, capabilities and
//! the rest of the request's configuration — together with the resource-level change list
//! computed by comparing the PROCESSED stage of that template, with the parameter values
//! it submits, against the revision in force on the stack, and answers the change set's
//! ARN and its stack's id. The plan is therefore over the resources the execution would
//! actually deploy, a resource a transform expands included.
//!
//! A change-set name the stack already carries is answered by COMPARING the attributes the
//! request submits with the ones the stored change set carries: the same attributes name
//! the same change set, so the call answers the change set that is already there, while
//! differing ones cannot be created under that name and the refusal names the attribute
//! that differs (`crate::state::StoredChangeSet::mismatching_attribute`). A request that
//! would CREATE a stack that already exists is refused as the `ValidationError` it is —
//! the request cannot be satisfied at all — rather than as a change-set name collision.
//!
//! `ChangeSetType=CREATE` also brings the stack itself into existence, in
//! `REVIEW_IN_PROGRESS` with no template and no resources, keyed by the `StackId` this call
//! answers (`ChangeSetType`: "CloudFormation creates a stack with a unique stack ID, but no
//! template or resources. The stack will be in the REVIEW_IN_PROGRESS state until you
//! execute the change set"); `ExecuteChangeSet` is what puts the template on it.
//!
//! An UPDATE change set is the PLAN of an update of the stack as it stands, so the statuses
//! that take a plan are the statuses that take the update
//! (`crate::state::status_takes_an_update`): a stack whose own failure another operation has
//! to resolve first (a deletion or a rollback that could not be carried out, a create that
//! was rolled back), one with an operation still in flight, and one a `CREATE` change set
//! brought into existence are refused with the `ValidationError` `UpdateStack` answers,
//! naming the stack and the state it really is in.
//!
//! CloudFormation answers this call while it is still computing the change set ("After the
//! CreateChangeSet call successfully completes, CloudFormation starts creating the change
//! set"), so the change set is answered `CREATE_PENDING` and settles to `CREATE_COMPLETE`
//! (or to `FAILED`, when it would change nothing, or when this service cannot apply the
//! transforms its template declares) once the clock makes that due — which is what
//! `DescribeChangeSet` reports and what `ExecuteChangeSet` waits for.
//!
//! The change list is read by the same template reader a deployment uses
//! (`crate::state::template`) and computed by `crate::state::change_plan`, so it covers the
//! JSON and YAML bodies `CreateStack` deploys; a body that reader will not read, or one
//! declaring no resources, is refused with the `ValidationError` CloudFormation answers for
//! a template format error — as is a body whose own declarations it cannot resolve, by the
//! same validation pass `CreateStack` holds a submitted template to
//! ([`crate::state::template_validation`]).
//!
//! A `TemplateURL` says WHERE that body is rather than what it means, so the object it
//! addresses is read from this emulator's own Amazon S3 and everything above happens on it
//! unchanged ([`crate::state::template_url`]); a URL naming no readable object is the
//! `ValidationError` the request's own reference earns.
//!
//! A resource the plan would create that ALREADY EXISTS is adopted rather than created
//! where the request asks for it — `ChangeSetType=IMPORT` names the resources to adopt and
//! `ImportExistingResources` asks for every name the template states that is already there
//! — and the change set reports each of them with the `Import` action and the identifier it
//! already carries ([`crate::state::resource_import`]). A name another stack already MANAGES
//! is not adoptable: the change set fails naming the stack that holds it
//! ([`crate::state::physical_name`]), since adopting it would take over a resource
//! CloudFormation manages elsewhere.
//!
//! The parameters whose value lives in AWS Systems Manager Parameter Store are resolved
//! while the plan is computed ([`crate::state::ssm_parameters`]), so the plan is computed
//! from the values an execution would deploy — and a name Parameter Store does not answer
//! for is the `ValidationError` the request earns, with no stack and no change set created.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateChangeSet::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `CreateChangeSet`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Creates a list of changes that will be applied to a stack so that you can review the
//! changes before executing them. You can create a change set for a stack that doesn't
//! exist or an existing stack. If you create a change set for a stack that doesn't exist,
//! the change set shows all of the resources that CloudFormation will create. If you create
//! a change set for an existing stack, CloudFormation compares the stack's information with
//! the information that you submit in the change set and lists the differences. Use change
//! sets to understand which resources CloudFormation will create or change, and how it will
//! change resources in an existing stack, before you create or update a stack. To create a
//! change set for a stack that doesn't exist, for the ChangeSetType parameter, specify
//! CREATE. To create a change set for an existing stack, specify UPDATE for the
//! ChangeSetType parameter. To create a change set for an import operation, specify IMPORT
//! for the ChangeSetType parameter. After the CreateChangeSet call successfully completes,
//! CloudFormation starts creating the change set. To check the status of the change set or
//! to review it, use the DescribeChangeSet action. When you are satisfied with the changes
//! the change set will make, execute the change set by using the ExecuteChangeSet action.
//! CloudFormation doesn't make changes until you execute the change set. To create a change
//! set for the entire stack hierarchy, set IncludeNestedStacks to True.
//!
//! ## Input
//!
//! * `Capabilities` (Capabilities) — In some cases, you must explicitly acknowledge that your
//!   stack template contains certain capabilities in order for CloudFormation to create the
//!   stack. CAPABILITY_IAM and CAPABILITY_NAMED_IAM Some stack templates might include
//!   resources that can affect permissions in your Amazon Web Services account, for example,
//!   by creating new IAM users. For those stacks, you must explicitly acknowledge this by
//!   specifying one of these capabilities. The following IAM resources require you to specify
//!   either the CAPABILITY_IAM or CAPABILITY_NAMED_IAM capability. If you have IAM resources,
//!   you can specify either capability. If you have IAM resources with custom names, you must
//!   specify CAPABILITY_NAMED_IAM. If …
//! * `ChangeSetName` (ChangeSetName, len 1..128, pattern ^[a-zA-Z][-a-zA-Z0-9]*$)
//!   **required** — The name of the change set. The name must be unique among all change sets
//!   that are associated with the specified stack. A change set name can contain only
//!   alphanumeric, case sensitive characters, and hyphens. It must start with an alphabetical
//!   character and can't exceed 128 characters.
//! * `ChangeSetType` (ChangeSetType, one of: CREATE | IMPORT | UPDATE) — The type of change
//!   set operation. To create a change set for a new stack, specify CREATE. To create a
//!   change set for an existing stack, specify UPDATE. To create a change set for an import
//!   operation, specify IMPORT. If you create a change set for a new stack, CloudFormation
//!   creates a stack with a unique stack ID, but no template or resources. The stack will be
//!   in the REVIEW_IN_PROGRESS state until you execute the change set. By default,
//!   CloudFormation specifies UPDATE. You can't use the UPDATE type to create a change set
//!   for a new stack or the CREATE type to create a change set for an existing stack.
//! * `ClientToken` (ClientToken, len 1..128) — A unique identifier for this CreateChangeSet
//!   request. Specify this token if you plan to retry requests so that CloudFormation knows
//!   that you're not attempting to create another change set with the same name. You might
//!   retry CreateChangeSet requests to ensure that CloudFormation successfully received them.
//! * `DeploymentConfig` (DeploymentConfig) — The deployment configuration for this stack
//!   operation, including the deployment mode.
//! * `DeploymentMode` (DeploymentMode, one of: REVERT_DRIFT) — Determines how CloudFormation
//!   handles configuration drift during deployment. REVERT_DRIFT – Creates a drift-aware
//!   change set that brings actual resource states in line with template definitions.
//!   Provides a three-way comparison between actual state, previous deployment state, and
//!   desired state. For more information, see Using drift-aware change sets in the
//!   CloudFormation User Guide.
//! * `Description` (Description, len 1..1024) — A description to help you identify this
//!   change set.
//! * `DisableValidation` (DisableValidation) — Set to true to disable pre-deployment
//!   validations in changeset or stack operations. Default: false
//! * `ImportExistingResources` (ImportExistingResources) — Indicates if the change set
//!   auto-imports resources that already exist. For more information, see Import Amazon Web
//!   Services resources into a CloudFormation stack automatically in the CloudFormation User
//!   Guide. This parameter can only import resources that have custom names in templates. For
//!   more information, see name type in the CloudFormation User Guide. To import resources
//!   that do not accept custom names, such as EC2 instances, use the ResourcesToImport
//!   parameter instead.
//! * `IncludeNestedStacks` (IncludeNestedStacks) — Creates a change set for the all nested
//!   stacks specified in the template. The default behavior of this action is set to False.
//!   To include nested sets in a change set, specify True.
//! * `NotificationARNs` (NotificationARNs, len 0..5) — The Amazon Resource Names (ARNs) of
//!   Amazon SNS topics that CloudFormation associates with the stack. To remove all
//!   associated notification topics, specify an empty list.
//! * `OnStackFailure` (OnStackFailure, one of: DELETE | DO_NOTHING | ROLLBACK) — Determines
//!   what action will be taken if stack creation fails. If this parameter is specified, the
//!   DisableRollback parameter to the ExecuteChangeSet API operation must not be specified.
//!   This must be one of these values: DELETE - Deletes the change set if the stack creation
//!   fails. This is only valid when the ChangeSetType parameter is set to CREATE. If the
//!   deletion of the stack fails, the status of the stack is DELETE_FAILED. DO_NOTHING - if
//!   the stack creation fails, do nothing. This is equivalent to specifying true for the
//!   DisableRollback parameter to the ExecuteChangeSet API operation. ROLLBACK - if the stack
//!   creation fails, roll back the stack. This is equivalent to specifying false for …
//! * `Parameters` (Parameters) — A list of Parameter structures that specify input parameters
//!   for the change set. For more information, see the Parameter data type.
//! * `ResourceTypes` (ResourceTypes) — Specifies which resource types you can work with, such
//!   as AWS::EC2::Instance or Custom::MyCustomInstance. If the list of resource types doesn't
//!   include a resource type that you're updating, the stack update fails. By default,
//!   CloudFormation grants permissions to all resource types. IAM uses this parameter for
//!   condition keys in IAM policies for CloudFormation. For more information, see Control
//!   CloudFormation access with Identity and Access Management in the CloudFormation User
//!   Guide. Only one of the Capabilities and ResourceType parameters can be specified.
//! * `ResourcesToImport` (ResourcesToImport, len 0..200) — The resources to import into your
//!   stack.
//! * `RoleARN` (RoleARN, len 20..2048) — The Amazon Resource Name (ARN) of an IAM role that
//!   CloudFormation assumes when executing the change set. CloudFormation uses the role's
//!   credentials to make calls on your behalf. CloudFormation uses this role for all future
//!   operations on the stack. Provided that users have permission to operate on the stack,
//!   CloudFormation uses this role even if the users don't have permission to pass it. Ensure
//!   that the role grants least permission. If you don't specify a value, CloudFormation uses
//!   the role that was previously associated with the stack. If no role is available,
//!   CloudFormation uses a temporary session that is generated from your user credentials.
//! * `RollbackConfiguration` (RollbackConfiguration) — The rollback triggers for
//!   CloudFormation to monitor during stack creation and updating operations, and for the
//!   specified monitoring period afterwards.
//! * `StackName` (StackNameOrId, len 1.., pattern
//!   ^([a-zA-Z][-a-zA-Z0-9]*)|(arn:\b(aws|aws-us-gov|aws-cn)\b:[-a-zA-Z0-9:/._+]*)$)
//!   **required** — The name or the unique ID of the stack for which you are creating a
//!   change set. CloudFormation generates the change set by comparing this stack's
//!   information with the information that you submit, such as a modified template or
//!   different parameter input values.
//! * `Tags` (Tags, len 0..50) — Key-value pairs to associate with this stack. CloudFormation
//!   also propagates these tags to resources in the stack. You can specify a maximum of 50
//!   tags.
//! * `TemplateBody` (TemplateBody, len 1..) — A structure that contains the body of the
//!   revised template, with a minimum length of 1 byte and a maximum length of 51,200 bytes.
//!   CloudFormation generates the change set by comparing this template with the template of
//!   the stack that you specified. Conditional: You must specify only one of the following
//!   parameters: TemplateBody, TemplateURL, or set the UsePreviousTemplate to true.
//! * `TemplateURL` (TemplateURL, len 1..5120) — The URL of the file that contains the revised
//!   template. The URL must point to a template (max size: 1 MB) that's located in an Amazon
//!   S3 bucket or a Systems Manager document. CloudFormation generates the change set by
//!   comparing this template with the stack that you specified. The location for an Amazon S3
//!   bucket must start with https://. URLs from S3 static websites are not supported.
//!   Conditional: You must specify only one of the following parameters: TemplateBody,
//!   TemplateURL, or set the UsePreviousTemplate to true.
//! * `UsePreviousTemplate` (UsePreviousTemplate) — Whether to reuse the template that's
//!   associated with the stack to create the change set. When using templates with the
//!   AWS::LanguageExtensions transform, provide the template instead of using
//!   UsePreviousTemplate to ensure new parameter values and Systems Manager parameter updates
//!   are applied correctly. For more information, see AWS::LanguageExtensions transform.
//!   Conditional: You must specify only one of the following parameters: TemplateBody,
//!   TemplateURL, or set the UsePreviousTemplate to true.
//!
//! ## Output
//!
//! * `Id` (ChangeSetId, len 1.., pattern ^arn:[-a-zA-Z0-9:/]*$) — The Amazon Resource Name
//!   (ARN) of the change set.
//! * `StackId` (StackId) — The unique ID of the stack.
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

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    change_set_status, change_set_type, early_validation, execution_status, status,
    StoredChangeSet, StoredStack,
};

/// The `OnStackFailure` value that is only valid for a change set which CREATES its stack:
/// "DELETE — Deletes the change set if the stack creation fails. This is only valid when the
/// ChangeSetType parameter is set to CREATE."
const ON_STACK_FAILURE_DELETE: &str = "DELETE";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateChangeSetError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateChangeSetError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#CreateChangeSet")]
#[derive(Debug)]
pub enum CreateChangeSetErrorModeled {
    AlreadyExistsException(AlreadyExistsException),
    InsufficientCapabilitiesException(InsufficientCapabilitiesException),
    LimitExceededException(LimitExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_change_set::CreateChangeSetError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateChangeSetError = crate::scaffolded::Scaffolded<CreateChangeSetErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateChangeSetErrorModeled> for CreateChangeSetError {
    fn from(modeled: CreateChangeSetErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateChangeSet` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateChangeSet<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateChangeSet<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateChangeSet<Eng>
{
    type Input = CreateChangeSetInput;
    type Output = CreateChangeSetOutput;
    type Error = CreateChangeSetError;

    const NAME: &'static str = "CreateChangeSet";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CHANGE_SET_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CHANGE_SET_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        mut input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
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
        // A `TemplateURL` says WHERE the revised template is, not what it means: the
        // object it addresses is read from this emulator's own Amazon S3 and becomes the
        // body everything below plans from, so a template submitted by reference and the
        // same one submitted inline produce the same change set
        // (`crate::state::template_url`).
        if let Some(url) = input.template_url.take() {
            let body = crate::state::template_url::fetch(
                ctx,
                &crate::state::request_region(ctx.parts()),
                &url,
            )
            .await
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
            input.template_body = Some(body);
        }
        // "By default, CloudFormation specifies UPDATE."
        let requested_type = input
            .change_set_type
            .as_deref()
            .unwrap_or(change_set_type::UPDATE);
        // An IMPORT change set brings resources that ALREADY EXIST under a stack's
        // management: it creates nothing, so the resources it names are adopted rather
        // than provisioned ([`crate::state::resource_import`]).
        let imports_resources = requested_type == change_set_type::IMPORT;
        let resources_to_import = input.resources_to_import.clone().unwrap_or_default();
        // "The resources to import into your stack" — only an IMPORT change set imports,
        // and an IMPORT names the resources it imports.
        if !imports_resources && !resources_to_import.is_empty() {
            return Err(crate::scaffolded::Scaffolded::validation_error(
                "ResourcesToImport can only be specified when ChangeSetType is IMPORT",
            ));
        }
        if imports_resources && resources_to_import.is_empty() {
            return Err(crate::scaffolded::Scaffolded::validation_error(
                crate::state::resource_import::RESOURCES_TO_IMPORT_REQUIRED,
            ));
        }
        // "DELETE — Deletes the change set if the stack creation fails. This is only valid
        // when the ChangeSetType parameter is set to CREATE": what becomes of a stack whose
        // CREATION failed is only a question for a change set that would create the stack, so
        // a request stating that outcome for any other type of change set is refused before
        // the plan is written.
        if input.on_stack_failure.as_deref() == Some(ON_STACK_FAILURE_DELETE)
            && requested_type != change_set_type::CREATE
        {
            return Err(crate::scaffolded::Scaffolded::validation_error(format!(
                "OnStackFailure parameter can only be specified to \
                 {ON_STACK_FAILURE_DELETE} when ChangeSetType is {}.",
                change_set_type::CREATE
            )));
        }
        // The rollback configuration this change set would put on the stack is held to the
        // same rule a create's and an update's is: a trigger naming a type CloudFormation
        // cannot monitor is refused before the change set exists
        // (`crate::state::validate_rollback_configuration`).
        crate::state::validate_rollback_configuration(input.rollback_configuration.as_ref())
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The tags this change set would put on the stack are held to the same rule a
        // create's and an update's are: a key in AWS's own reserved namespace is refused
        // before the change set exists (`crate::state::stack_tags`).
        crate::state::stack_tags::validate_optional_tags(input.tags.as_deref())
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // "Only one of the Capabilities and ResourceType parameters can be specified": the
        // two members state two different bounds on the operation this change set plans, so
        // a request stating both states no one bound and is refused for its SHAPE
        // (`crate::state::resource_type_allow_list`).
        crate::state::resource_type_allow_list::one_bound(
            input.capabilities.as_deref().unwrap_or_default(),
            input.resource_types.as_deref().unwrap_or_default(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        if let Some(body) = &input.template_body {
            // The documented `TemplateBody` maximum, which the model carries no `@length`
            // for.
            crate::state::validate_template_body(body)
                .map_err(crate::scaffolded::Scaffolded::validation_error)?;
            // The documented per-template quotas, the ceilings `DescribeAccountLimits`
            // advertises for this account: a body past one is refused before the change
            // set is created, since it is no plan CloudFormation would execute.
            crate::state::validate_template_quotas(body)
                .map_err(crate::scaffolded::Scaffolded::validation_error)?;
            // A change set is a plan over a template CloudFormation can deploy: a body
            // declaring a resource type it does not recognise is refused before the change
            // set is created (`crate::state::resource_types`).
            crate::state::validate_resource_types(body)
                .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        }
        // The values this change set would deploy with: what a language-extensions
        // directive, a mapping lookup or a key an expression decides resolves against, and
        // therefore part of what the stage below is computed from.
        let supplied: ::std::collections::BTreeMap<String, String> = input
            .parameters
            .iter()
            .flatten()
            .filter_map(|parameter| {
                Some((
                    parameter.parameter_key.clone()?,
                    parameter.parameter_value.clone()?,
                ))
            })
            .collect();
        // The PROCESSED stage of a submitted body: the document the change set is PLANNED
        // from and the one its execution deploys, so the plan a caller reviews and the
        // deployment that is carried out cannot disagree. Applying a template's transforms
        // is caller-sized work, so it happens HERE — before the store lock below — the way
        // `CreateStack` does it, and from the same values, so all three submitting
        // operations see one document. A change set reusing the stack's template takes the
        // stage the stack already stores instead, so nothing expands a template under the
        // lock.
        let submitted_processed = input.template_body.as_deref().map(|body| {
            crate::state::template::process_with_parameters(body, &supplied)
        });
        // The same template validation `CreateStack` applies: a change set is a plan over a
        // template CloudFormation would deploy, so a body whose own declarations it cannot
        // resolve is refused before the plan is written.
        if let (Some(body), Some(processed)) =
            (input.template_body.as_deref(), submitted_processed.as_ref())
        {
            crate::state::validate_submitted_template(
                body,
                processed,
                &supplied,
                &crate::state::request_region(ctx.parts()),
            )
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // This service was mounted on an engine that is not the shared in-memory one,
            // so there is no store to create the change set in: nothing was written.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CreateChangeSet",
            ));
        };
        // The change set carries the `RoleARN` its execution would run as, so a role
        // CloudFormation cannot assume is refused before the plan is written — the same
        // check `CreateStack` makes.
        crate::state::refuse_unassumable_role(&*self.engine, input.role_arn.as_deref())?;
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx.parts());
        // The submitted template as the plan below reads it, with the parameter values this
        // change set would deploy with: the ONE reading both stages beneath this need — the
        // parameter-store resolution, and the names the existence check asks about. A change
        // set reusing the stack's own template submits no body and so declares nothing new.
        let declared_submitted = submitted_processed
            .as_ref()
            .and_then(|processed| processed.body(input.template_body.as_deref()?))
            .and_then(crate::state::declared_template);
        let values_in_force = declared_submitted.as_ref().map(|declared| {
            let mut values = declared.parameter_defaults();
            values.extend(supplied.clone());
            values
        });
        // The PARAMETER RESOLUTION stage: a template parameter whose type puts its value in
        // AWS Systems Manager Parameter Store carries the NAME of a parameter there, and
        // CloudFormation resolves it while it computes the plan — so the plan is computed
        // from the values an execution would deploy. A name Parameter Store does not answer
        // for leaves the request with no value for that parameter, and since nothing has
        // been created yet the request itself is refused
        // ([`crate::state::ssm_parameters::resolve_in_force`]): no stack, no change set.
        //
        // What is resolved is every parameter this change set would DEPLOY with, not only
        // the ones the request restated: the template in force decides which of them address
        // a parameter-store name (the submitted body, or the stack's own where the request
        // reuses it), and the values in force are that template's declared defaults, the
        // values the stack already carries for a parameter the request does not restate, and
        // the ones the request supplies — in that order of precedence, which is the set
        // `merged_parameters` and `parameters_in_force` resolve to under the lock below. The
        // stack's half is read under a lock of its own, released before the reads.
        let (stack_template, stack_parameters, stack_notification_arns) = {
            let store = state.lock();
            match store
                .resolve(&region, &input.stack_name)
                .filter(|stack| stack.is_live())
            {
                Some(stack) => (
                    stack.template_body.clone(),
                    stack.parameters.clone(),
                    stack.notification_arns.clone(),
                ),
                None => (None, Vec::new(), Vec::new()),
            }
        };
        let resolution_body = input.template_body.clone().or(stack_template);
        let addressed_values = resolution_body.as_deref().map(|body| {
            let mut values = crate::state::declared_template(body)
                .map(|declared| declared.parameter_defaults())
                .unwrap_or_default();
            for parameter in &stack_parameters {
                values.insert(parameter.key.clone(), parameter.value.clone());
            }
            values.extend(supplied.clone());
            values
        });
        let resolved_parameters = match (resolution_body.as_deref(), &addressed_values) {
            (Some(body), Some(values)) => {
                crate::state::ssm_parameters::resolve_in_force(ctx, &region, body, values)
                    .await
                    .map_err(crate::scaffolded::Scaffolded::validation_error)?
            }
            _ => ::std::collections::BTreeMap::new(),
        };
        // An IMPORT change set adopts resources that already exist, so the template has to
        // state what becomes of each of them when the stack it joins stops managing it:
        // "each resource to import must have a DeletionPolicy attribute in your template"
        // ([`crate::state::resource_import`]). Held BEFORE anything is written, so a refused
        // import leaves no stack behind.
        if imports_resources {
            // An IMPORT states its resources in a template, so one submitted by neither
            // value nor reference has nothing to import them into: this service's own
            // wording for that request, as `crate::state::resource_import`'s other input
            // refusals are.
            let declared = declared_submitted.as_ref().ok_or_else(|| {
                crate::scaffolded::Scaffolded::validation_error(
                    "An IMPORT change set must submit the template the resources are \
                     imported into",
                )
            })?;
            crate::state::resource_import::validate_deletion_policies(
                declared,
                &resources_to_import,
            )
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        }
        // The resources the submitted template NAMES, each with the identifier its owning
        // service would know it by ([`early_validation::named_resources`]): what the
        // existence check asks about, and what an adopted resource is reported under.
        //
        // The scope a stated name is evaluated in is what the REQUEST carries: the Region,
        // the stack's name and the parameter values this change set would deploy with. The
        // stack's ID exists only once the stack does, so a name built from it resolves to
        // nothing here and that resource is simply not asked about — a name no service holds
        // is never reported as taken.
        // The topics this change set would put in force, read the same way the record below
        // reads them: the ones the request STATES, and the stack's own where it states none.
        // One rule for both stages, so a name built from `AWS::NotificationARNs` resolves
        // here to what the execution would deploy with.
        let topics_in_force: &[String] = match input.notification_ar_ns.as_deref() {
            Some(stated) => stated,
            None => &stack_notification_arns,
        };
        let named_resources = match (&declared_submitted, &values_in_force) {
            (Some(declared), Some(values)) => early_validation::named_resources(
                declared,
                crate::state::template::StackScope {
                    region: &region,
                    stack_name: &input.stack_name,
                    stack_id: "",
                    // The topics this change set would put in force: what a name built
                    // from `AWS::NotificationARNs` would resolve to under it.
                    notification_arns: topics_in_force,
                },
                values,
            ),
            _ => Vec::new(),
        };
        // The PRE-DEPLOYMENT validation stage: a resource the change set would CREATE under
        // an identifier the service that owns it ALREADY holds cannot be created, so the plan
        // is not one that could be executed — and CloudFormation establishes that while
        // creating the change set rather than at execution time
        // (`crate::state::early_validation`). The questions are asked HERE, before the store
        // lock below, because each one reaches the service that owns the type; which of those
        // resources the plan would really create is decided from the plan itself, under the
        // lock. `DisableValidation` turns the stage off, and a change set created with it
        // carries the plan unvalidated.
        //
        // An IMPORT creates nothing, so it has no addition for that check to be about: what
        // the reads establish for it is whether the resources it ADOPTS are really there,
        // which is the same question put about the identifiers the request named
        // (`import_candidates`). One read per named resource either way.
        let validating = input.disable_validation != Some(true);
        let import_plan = if imports_resources {
            crate::state::resource_import::import_changes(&resources_to_import, &named_resources)
        } else {
            Vec::new()
        };
        let import_candidates = crate::state::resource_import::import_candidates(&import_plan);
        let taken_names = if validating && !imports_resources {
            early_validation::taken_names(ctx, &region, &named_resources).await
        } else {
            Vec::new()
        };
        // `None` where the reads were not made at all — a request that turned the stage off
        // establishes nothing about any identifier, so nothing is reported as missing.
        let held_imports = if validating && imports_resources {
            Some(early_validation::taken_names(ctx, &region, &import_candidates).await)
        } else {
            None
        };
        // A DRIFT-AWARE change set compares the template against what the stack's resources
        // REALLY are rather than against the template alone: "REVERT_DRIFT — creates a
        // drift-aware change set that brings actual resource states in line with template
        // definitions". So the resources the stack has deployed are read back from the
        // services that own them HERE, with the store unlocked, exactly as the existence
        // check above is asked — and under a lock of its own, released before the reads.
        let reverts_drift = crate::state::drift::reverts_drift(input.deployment_mode.as_deref());
        let drifted = if reverts_drift {
            let deployed = {
                let store = state.lock();
                store
                    .resolve(&region, &input.stack_name)
                    .filter(|stack| stack.is_live())
                    .map(|stack| stack.current_resources())
                    .unwrap_or_default()
            };
            crate::state::drift::drifted(ctx, &region, &deployed).await
        } else {
            Vec::new()
        };
        let mut store = state.lock();
        store.settle(now);

        // The stack the change set is for, when it already exists. A stack in
        // REVIEW_IN_PROGRESS does NOT exist yet — "the stack will be in the
        // REVIEW_IN_PROGRESS state until you execute the change set" — so a second CREATE
        // change set for it is accepted and targets that same record.
        let existing = store
            .resolve(&region, &input.stack_name)
            .filter(|stack| stack.is_live())
            .map(|stack| (stack.stack_id.clone(), stack.status.clone()));
        let in_review = existing
            .as_ref()
            .is_some_and(|(_, status)| *status == status::REVIEW_IN_PROGRESS);

        // "You can't use the UPDATE type to create a change set for a new stack or the
        // CREATE type to create a change set for an existing stack." An IMPORT brings
        // resources into a stack that may or may not be there yet ("to create a change set
        // for an import operation, specify IMPORT"), and where it is not, that change set
        // creates it — in review, exactly as a CREATE change set does.
        let creates_the_stack = requested_type == change_set_type::CREATE
            || (imports_resources && existing.is_none());
        if creates_the_stack {
            if existing.is_some() && !in_review {
                // The request names a stack that is already there with the type that
                // CREATES one, so it is the REQUEST that cannot be satisfied — not the
                // change set's name colliding with another. CloudFormation answers the
                // `ValidationError` it answers for a request whose shape is wrong, naming
                // both the stack and the change set the request asked for; the modelled
                // `AlreadyExistsException` is what a change-set NAME collision answers, and
                // a client branching on the code has to be able to tell the two apart.
                return Err(crate::scaffolded::Scaffolded::validation_error(format!(
                    "Stack [{}] already exists and cannot be created again with the \
                     changeSet [{}].",
                    input.stack_name, input.change_set_name
                )));
            }
            if existing.is_none() {
                // The stack does not exist yet, so this call names it: the documented
                // stack-name rule applies, as it does on `CreateStack`.
                crate::state::validate_stack_name(&input.stack_name)
                    .map_err(crate::scaffolded::Scaffolded::validation_error)?;
                // "The quota for the resource has already been reached": the store holds at
                // most `MAX_STACKS` live stacks per Region and `MAX_LIVE_STACKS` across
                // every Region (the Region being the caller's own credential scope), and a
                // CREATE change set creates one.
                if store.live_stacks(&region).count() >= crate::state::MAX_STACKS
                    || store.live_stack_count() >= crate::state::MAX_LIVE_STACKS
                {
                    return Err(CreateChangeSetErrorModeled::LimitExceededException(
                        LimitExceededException {
                            message: Some(format!(
                                "Limit for stacks exceeded: no more than {} stacks can exist \
                                 in this account and Region",
                                crate::state::MAX_STACKS
                            )),
                        },
                    )
                    .into());
                }
            }
        }
        // The stack this change set belongs to, when it already exists: an UPDATE change
        // set compares against it, and a CREATE change set for a stack still in
        // REVIEW_IN_PROGRESS targets that same record. `None` means this call mints it.
        let target_stack_id = match &existing {
            Some((stack_id, stack_status)) => {
                // An UPDATE change set is the PLAN of an update of the stack as it stands,
                // so the statuses that take a plan are the statuses that take the update
                // ([`crate::state::status_takes_an_update`]): a stack whose own failure
                // another operation has to resolve first, one with an operation still in
                // flight, and one a CREATE change set brought into existence — which holds
                // no template to compare an update against — answer the refusal
                // `UpdateStack` answers, naming the state the stack really is in.
                if !creates_the_stack && !crate::state::status_takes_an_update(stack_status) {
                    return Err(crate::scaffolded::Scaffolded::validation_error(
                        crate::state::stack_cannot_be_updated(stack_id, stack_status),
                    ));
                }
                Some(stack_id.clone())
            }
            None => {
                if !creates_the_stack {
                    // An UPDATE change set compares against a stack that exists.
                    return Err(crate::scaffolded::Scaffolded::validation_error(
                        crate::state::changed_stack_does_not_exist(&input.stack_name),
                    ));
                }
                None
            }
        };

        // The stack's current template (with the processed stage it already stores),
        // parameters and DEPLOYED resources, which the change set is computed against, plus
        // the identity its pseudo parameters resolve to. A stack that does not exist yet has
        // none of them — and so does a stack id that resolves to nothing in the Region the
        // request is signed for, which is the same case: there is nothing to compare
        // against, and the arm below is the one a new stack takes.
        let (previous_template, previous_processed, previous_parameters, deployed, stack_identity) =
            match target_stack_id
                .as_deref()
                .and_then(|stack_id| store.resolve(&region, stack_id))
            {
                None => (
                    None,
                    crate::state::template::Processed::Same,
                    Vec::new(),
                    Vec::new(),
                    None,
                ),
                Some(stack) => (
                    stack.template_body.clone(),
                    stack.processed_template.clone(),
                    stack.parameters.clone(),
                    stack.resources.clone(),
                    Some((
                        stack.region.clone(),
                        stack.stack_name.clone(),
                        stack.stack_id.clone(),
                    )),
                ),
            };
        // What a `Fn::ImportValue` in the stack's template resolved to: the plan's two
        // sides resolve imports against it, the way they resolve references against the
        // resources the stack has deployed.
        let previous_imports = target_stack_id
            .as_deref()
            .and_then(|stack_id| store.resolve(&region, stack_id))
            .map(|stack| stack.imports.clone())
            .unwrap_or_default();
        // The Amazon SNS topics in force on the stack, which is what
        // `AWS::NotificationARNs` resolves to on both sides of the plan.
        let previous_notification_arns = target_stack_id
            .as_deref()
            .and_then(|stack_id| store.resolve(&region, stack_id))
            .map(|stack| stack.notification_arns.clone())
            .unwrap_or_default();
        // "Whether to reuse the template that's associated with the stack to create the
        // change set." Either way the body and its processed stage travel together, which
        // `zip` is what expresses: a submitted body ALWAYS has the stage computed for it
        // above the lock, and a reused one takes the stage its stack already stores.
        let (template_body, processed_template) =
            match input.template_body.clone().zip(submitted_processed) {
                Some(submitted) => submitted,
                None => (
                    previous_template.clone().ok_or_else(|| {
                        crate::scaffolded::Scaffolded::validation_error(format!(
                            "Stack [{}] has no template to reuse",
                            input.stack_name
                        ))
                    })?,
                    previous_processed.clone(),
                ),
            };
        // "In some cases, you must explicitly acknowledge that your stack template contains
        // certain capabilities ... If you don't specify either of these capabilities,
        // CloudFormation returns an InsufficientCapabilities error": the template this
        // change set would put on the stack is held to the acknowledgement THIS request
        // carries, before the plan is written.
        if let Some(message) = crate::state::insufficient_capabilities(
            &template_body,
            input.capabilities.as_deref().unwrap_or_default(),
        ) {
            return Err(
                CreateChangeSetErrorModeled::InsufficientCapabilitiesException(
                    InsufficientCapabilitiesException {
                        message: Some(message),
                    },
                )
                .into(),
            );
        }
        // The macro acknowledgement (`CAPABILITY_AUTO_EXPAND`) that `CreateStack` and
        // `UpdateStack` hold a transform-declaring template to is deliberately NOT asked
        // for here: "this capacity doesn't apply to creating change sets, and specifying it
        // when creating change sets has no effect" — reviewing a macro's expansion in a
        // change set is the very thing the acknowledgement stands in for.
        // "If the list of resource types doesn't include a resource that you're creating,
        // the stack creation fails": the bound this request put on the operation it plans is
        // weighed against the template that plan would deploy, before the plan is written
        // (`crate::state::resource_type_allow_list`).
        crate::state::validate_allowed_resource_types(
            &template_body,
            input.resource_types.as_deref().unwrap_or_default(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;

        let parameters =
            crate::state::merged_parameters(&previous_parameters, input.parameters.as_deref())
                .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The parameters this change set would DEPLOY with, which is what it reports: the
        // resolved set above, plus the template's own `Default` for every parameter the
        // template declares and nothing supplied a value for — the same set `CreateStack`
        // and `UpdateStack` record ([`crate::state::parameters_in_force`]), since the plan
        // reports the values its execution would use. The template this change set would put
        // on the stack is also what says which of its parameters are masked, which that
        // resolution records, and `ExecuteChangeSet` carries onto the stack with the values.
        let mut parameters = crate::state::parameters_in_force(&template_body, parameters);
        // A change set is a plan over a deployment, so the parameter set it would deploy
        // with is held to the template's input CONTRACT before the plan is written: the
        // same rule `CreateStack` and `UpdateStack` apply
        // (`crate::state::parameter_constraints`).
        crate::state::validate_parameter_set(
            &template_body,
            &parameters,
            input.parameters.as_deref(),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;
        // The VALUE in force for an SSM-typed parameter is the one Parameter Store held when
        // this call resolved it above, recorded beside the name that addressed it: the change
        // set reports the name as its `ParameterValue` and that value as its `ResolvedValue`,
        // and the plan below is computed from it.
        crate::state::ssm_parameters::record_resolved_for(
            &template_body,
            &mut parameters,
            &resolved_parameters,
        );
        let parameters = parameters;

        // The change list is what a change set IS: "Creates a list of changes that will be
        // applied to a stack so that you can review the changes before executing them."
        // It is computed here, once — never on a read — by the SAME template reader a
        // deployment uses, from the PROCESSED stage of both templates, so the plan covers
        // exactly the resources the execution would deploy (a resource a transform expands
        // included) for the JSON and YAML bodies `CreateStack` deploys alike; and from the
        // stack's REVISION rather than its template alone: CloudFormation "compares the
        // stack's information with the information that you submit in the change set", and
        // the parameter values in force are part of that information.
        //
        // A template whose transforms this service cannot apply has no processed stage, so
        // there is nothing to plan from: the change set is still created, and settles to
        // `FAILED` with that reason below — the same answer `CreateStack` gives such a
        // template, and never a plan over a document nobody processed.
        let unprocessable = processed_template.unavailable_reason().map(str::to_string);
        let processed_body = processed_template.body(&template_body).map(str::to_string);
        let submitted = match &processed_body {
            Some(body) => {
                let Some(submitted) = crate::state::declared_template(body) else {
                    return Err(crate::scaffolded::Scaffolded::validation_error(
                        crate::state::template::not_well_formed(body),
                    ));
                };
                // A change set is a plan over resources, so a template that declares none
                // has nothing to plan — and one carrying an entry with no `Type` is refused
                // for that entry, in the wording the one place that owns it gives.
                if let Some(refusal) =
                    crate::state::template::resources_refusal(&submitted)
                {
                    return Err(crate::scaffolded::Scaffolded::validation_error(refusal));
                }
                submitted
            }
            // No document was processed, so none is read: the plan below is empty for the
            // same reason.
            None => crate::state::template::Template::default(),
        };
        // The plan itself, over the two processed stages: the stack's own template is read at
        // its processed stage too, by the shared reader `UpdateStack` plans through — a stack
        // with no template yet, or one whose template declares no resources, simply has none
        // to compare against, so every resource the submitted template declares is an
        // addition. A SUBMITTED template with NO processed
        // stage leaves the change set with no plan at all: not a removal of everything the
        // stack holds (which is what comparing against an empty declaration set would
        // read as), because nothing was computed from any document — the change set
        // settles to `FAILED` with the reason instead.
        let mut changes = if unprocessable.is_some() {
            Vec::new()
        } else if imports_resources {
            // An IMPORT change set does not compare templates at all: it adopts exactly the
            // resources the request names, each under the identifier it already exists with
            // ([`crate::state::resource_import`]), which is the plan computed above.
            import_plan.clone()
        } else {
            crate::state::changes_against(
                stack_identity.as_ref().map(|(region, stack_name, stack_id)| {
                    crate::state::DeployedStack {
                        region,
                        stack_name,
                        stack_id,
                        body: previous_template.as_deref(),
                        processed: &previous_processed,
                        parameters: &previous_parameters,
                        deployed: &deployed,
                        imports: &previous_imports,
                        notification_arns: &previous_notification_arns,
                    }
                }),
                &submitted,
                &parameters,
            )
        };
        // What a DRIFT-AWARE change set adds to that plan: one `Modify` per resource the
        // service that owns it reports DIFFERENTLY from the configuration the stack recorded,
        // putting the template's value back over the live one
        // ([`crate::state::change_plan::drift_change`]). A resource the template comparison
        // already reports a change for keeps that report: the template states the change
        // itself, which the execution would apply anyway.
        for drifted in &drifted {
            let already_reported = changes
                .iter()
                .any(|change| change.logical_resource_id == drifted.logical_id);
            if !already_reported {
                changes.push(crate::state::change_plan::drift_change(drifted));
            }
        }

        // The configuration the change set would carry, resolved once: it is what the
        // comparison below holds a re-created name to AND what the record stores.
        let capabilities = input.capabilities.clone().unwrap_or_default();
        let resource_types = input.resource_types.clone().unwrap_or_default();
        // "The Amazon Resource Names (ARNs) of Amazon SNS topics that CloudFormation
        // associates with the stack": a request that names none states nothing about the
        // stack's topics, so the ones in force stay in force — the same reading `UpdateStack`
        // gives its own member. An explicitly EMPTY list is a statement, and it CLEARS them.
        let notification_arns = match input.notification_ar_ns.clone() {
            Some(stated) => stated,
            None => previous_notification_arns.clone(),
        };
        let tags = crate::state::stored_tags(input.tags.as_deref().unwrap_or_default());
        let rollback_configuration =
            crate::state::stored_rollback_configuration(input.rollback_configuration.as_ref());
        let deployment_config =
            crate::state::stored_deployment_config(input.deployment_config.as_ref());
        let stored_change_set_type = if imports_resources {
            change_set_type::IMPORT
        } else if creates_the_stack {
            change_set_type::CREATE
        } else {
            change_set_type::UPDATE
        };
        let submitted_attributes = crate::state::SubmittedChangeSet {
            change_set_type: stored_change_set_type,
            description: input.description.as_deref(),
            template_body: &template_body,
            parameters: &parameters,
            capabilities: &capabilities,
            resource_types: &resource_types,
            role_arn: input.role_arn.as_deref(),
            rollback_configuration: rollback_configuration.as_ref(),
            notification_arns: &notification_arns,
            tags: &tags,
            on_stack_failure: input.on_stack_failure.as_deref(),
            include_nested_stacks: input.include_nested_stacks.unwrap_or(false),
            import_existing_resources: input.import_existing_resources.unwrap_or(false),
            deployment_mode: input.deployment_mode.as_deref(),
            client_token: input.client_token.as_deref(),
        };

        // A change set of this stack that this very request already created: "Specify this
        // token if you plan to retry requests so that CloudFormation knows that you're not
        // attempting to create another change set with the same name."
        if let Some(token) = &input.client_token {
            let replayed = target_stack_id.as_deref().and_then(|stack_id| {
                store
                    .change_sets_of(stack_id)
                    .find(|change_set| {
                        change_set.client_token.as_deref() == Some(token.as_str())
                            && change_set.change_set_name == input.change_set_name
                    })
                    .map(|change_set| {
                        (change_set.change_set_id.clone(), change_set.stack_id.clone())
                    })
            });
            if let Some(replayed) = replayed {
                return Ok(CreateChangeSetOutput {
                    id: Some(replayed.0),
                    stack_id: Some(replayed.1),
                });
            }
        }
        // "The name must be unique among all change sets that are associated with the
        // specified stack." What a name the stack already carries answers depends on WHAT
        // the request submits under it, not on the name alone: the same attributes name the
        // same change set, so the call is answered with the change set that is already
        // there, and differing attributes cannot be created under that name — a refusal
        // that names the attribute that differs, so a client can tell an idempotent
        // re-submit from a conflicting one.
        if let Some(stack_id) = target_stack_id.as_deref() {
            let existing_change_set = store
                .change_sets_of(stack_id)
                .find(|change_set| change_set.change_set_name == input.change_set_name)
                .map(|change_set| {
                    (
                        change_set.change_set_id.clone(),
                        change_set.stack_id.clone(),
                        change_set.mismatching_attribute(&submitted_attributes),
                    )
                });
            if let Some((change_set_id, stack_id, mismatch)) = existing_change_set {
                return match mismatch {
                    None => Ok(CreateChangeSetOutput {
                        id: Some(change_set_id),
                        stack_id: Some(stack_id),
                    }),
                    Some(attribute) => {
                        Err(CreateChangeSetErrorModeled::AlreadyExistsException(
                            AlreadyExistsException {
                                message: Some(format!(
                                    "ChangeSet {} cannot be created due to a mismatch with \
                                     existing attribute {attribute}",
                                    input.change_set_name
                                )),
                            },
                        )
                        .into())
                    }
                };
            }
            if store.change_sets_of(stack_id).count()
                >= crate::state::MAX_CHANGE_SETS_PER_STACK
            {
                return Err(CreateChangeSetErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(format!(
                            "Limit for change sets exceeded: no more than {} change sets can \
                             exist for one stack",
                            crate::state::MAX_CHANGE_SETS_PER_STACK
                        )),
                    },
                )
                .into());
            }
        }
        if store.change_sets.len() >= crate::state::MAX_CHANGE_SETS {
            return Err(CreateChangeSetErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "Limit for change sets exceeded: no more than {} change sets can \
                         exist in this account and Region",
                        crate::state::MAX_CHANGE_SETS
                    )),
                },
            )
            .into());
        }

        // Validated and within quota: from here the call allocates.
        // A resource the plan would ADOPT — one it names as already existing, or one it
        // would CREATE under a name that is already taken — is adoptable only while NO stack
        // manages that name: a resource another stack owns is that stack's, so adopting it
        // would take over a resource CloudFormation manages elsewhere. Such a plan fails
        // naming the stack that holds the resource, and the stack this plan is for keeps the
        // status it had, in review with nothing provisioned.
        //
        // `ImportExistingResources` is what asks for an ADDITION to be adopted instead
        // ("CloudFormation imports it into the stack instead of creating it"); an IMPORT
        // change set already reports what it adopts.
        let adopting = input.import_existing_resources == Some(true);
        let adoptable: Vec<(&str, &str)> = changes
            .iter()
            .filter(|change| {
                change.action == crate::state::change_action::IMPORT
                    || (adopting && change.action == crate::state::change_action::ADD)
            })
            .filter_map(|change| {
                let physical = if change.action == crate::state::change_action::IMPORT {
                    change.physical_resource_id.as_deref()
                } else {
                    // An addition has no identifier of its own yet: the one it would be
                    // created under is what the existence check asked about.
                    taken_names
                        .iter()
                        .find(|taken| taken.logical_id == change.logical_resource_id)
                        .map(|taken| taken.physical_resource_id.as_str())
                }?;
                Some((change.resource_type.as_str(), physical))
            })
            .collect();
        let managed_elsewhere = adoptable.iter().find_map(|(resource_type, physical)| {
            crate::state::physical_name::managing(
                store.stacks.iter(),
                &region,
                resource_type,
                physical,
            )
            .map(|owner| {
                crate::state::physical_name::already_exists_in_stack(physical, &owner.stack_id)
            })
        });
        // ...and a resource an IMPORT adopts has to BE there: the plan reports the stack as
        // managing a resource that already exists, so one the service that owns the type does
        // not hold — asked above, by reading the identifier the request named — is no resource
        // to import at all, and the change set fails rather than offering a plan whose
        // execution would record a resource nobody has. A type nothing here owns is not read
        // and so establishes neither, and a request that turned the reads off reports nothing
        // missing: an unanswered read is not evidence of absence.
        let missing_import = held_imports.as_ref().and_then(|held| {
            import_candidates
                .iter()
                .filter(|candidate| early_validation::read_back(&candidate.resource_type))
                .find(|candidate| {
                    !held
                        .iter()
                        .any(|read_back| read_back.logical_id == candidate.logical_id)
                })
                .map(|candidate| {
                    early_validation::not_found(
                        &candidate.resource_type,
                        &candidate.physical_resource_id,
                    )
                })
        });
        if adopting && managed_elsewhere.is_none() {
            crate::state::resource_import::adopt_existing(&mut changes, &taken_names);
        }
        // What the PRE-DEPLOYMENT validation above makes of this plan: a resource it would
        // CREATE under an identifier an owning service already holds fails the change set,
        // which is then not offered for execution at all. A resource the plan ADOPTS is no
        // longer one it would create, so nothing of it is left for that check to fail — and
        // a plan stopped by an owner above failed on that, not on this check.
        let conflicting = managed_elsewhere
            .is_none()
            .then(|| early_validation::failing_addition(&changes, &taken_names).cloned())
            .flatten();
        let change_set_id = crate::state::new_change_set_id(&region, &input.change_set_name);
        // The id of the operation creating this change set is: every event the creation
        // records carries it, so the events of one change-set creation are reportable as
        // that operation's ([`crate::state::StoredOperation`]).
        let operation_id = crate::state::new_operation_id();
        let stack_id = match target_stack_id {
            Some(stack_id) => stack_id,
            None => {
                let stack_id = crate::state::new_stack_id(&region, &input.stack_name);
                let mut stack = StoredStack::new(
                    stack_id.clone(),
                    input.stack_name.clone(),
                    // "CloudFormation creates a stack with a unique stack ID, but no
                    // template or resources": the template is the change set's until it is
                    // executed.
                    None,
                    now,
                );
                stack.status = status::REVIEW_IN_PROGRESS.to_string();
                stack.status_reason = Some(crate::state::USER_INITIATED.to_string());
                // The Region the stack lives in is the one this call was signed for, as it
                // is for a stack `CreateStack` creates.
                stack.region = region.clone();
                // ...and so are the Amazon SNS topics the change set names: the change set
                // that brings the stack into existence carries the stack's configuration,
                // so `DescribeStacks` echoes them from here on and the events of the stack
                // are published to them.
                stack.notification_arns = notification_arns.clone();
                stack.record_event(
                    now,
                    status::REVIEW_IN_PROGRESS,
                    Some(crate::state::USER_INITIATED.to_string()),
                    Some(operation_id.clone()),
                    None,
                );
                store.stacks.push(stack);
                stack_id
            }
        };
        // `Stack$ChangeSetId` is the change set that last acted on the stack. A CREATE
        // change set is the only thing that has acted on the stack it created.
        if let Some(stack) = store.resolve_mut(&region, &stack_id) {
            if creates_the_stack {
                stack.change_set_id = Some(change_set_id.clone());
                // On the stack this change set brings into existence, creating the change
                // set is the only operation performed so far, which is what the model's
                // `CREATE_CHANGESET` operation type names. On a stack that ALREADY exists
                // a change set changes nothing until it is executed, so that stack's last
                // operation stays the create or update that last acted on it.
                stack.record_change_set_operation(
                    crate::state::operation_type::CREATE_CHANGESET,
                    &operation_id,
                    Some(change_set_id.clone()),
                    now,
                );
            }
        }

        // "To troubleshoot Early Validation errors, use the DescribeEvents API for detailed
        // failure information": the detail is an event about the resource the validation
        // failed on, carrying what is wrong with it — which identifier of which type is
        // already taken — so a client reading the stack's events finds the resource rather
        // than only the summary the change set reports.
        if let Some(conflicting) = &conflicting {
            if let Some(stack) = store.resolve_mut(&region, &stack_id) {
                stack.record_validation_failure(
                    now,
                    &conflicting.logical_id,
                    &conflicting.resource_type,
                    crate::state::StoredValidationFailure {
                        name: early_validation::NAME_CONFLICT_VALIDATION.to_string(),
                        status: early_validation::VALIDATION_FAILED.to_string(),
                        reason: early_validation::already_exists(
                            &conflicting.resource_type,
                            &conflicting.physical_resource_id,
                        ),
                        path: early_validation::resource_path(&conflicting.logical_id),
                    },
                    Some(operation_id.clone()),
                    input.client_token.clone(),
                );
            }
        }

        // "CloudFormation starts creating the change set": it is answered CREATE_PENDING
        // and settles once the clock makes the terminal status due. A change set that would
        // change nothing settles to FAILED instead, with the reason CloudFormation gives —
        // and so does one whose template has no processed stage here, with the reason that
        // says what could not be applied, so a plan nobody could compute is never offered
        // for execution.
        let settles_to = match &unprocessable {
            Some(reason) => (
                change_set_status::FAILED,
                execution_status::UNAVAILABLE,
                Some(format!(
                    "The template's transforms could not be applied: {reason}"
                )),
            ),
            // A name this plan would ADOPT that another stack already manages is not this
            // plan's to take: the change set fails naming the stack that holds the resource,
            // and that stack keeps it.
            None if managed_elsewhere.is_some() => (
                change_set_status::FAILED,
                execution_status::UNAVAILABLE,
                managed_elsewhere.clone(),
            ),
            // ...and a resource to import that the owning service does not hold is no
            // resource to adopt at all.
            None if missing_import.is_some() => (
                change_set_status::FAILED,
                execution_status::UNAVAILABLE,
                missing_import.clone(),
            ),
            // A plan the pre-deployment validation did not pass is not offered for
            // execution: the change set fails with the validation's own reason, and the
            // stack it was created for keeps the status it had — a CREATE change set's stack
            // stays in review with nothing provisioned.
            None if conflicting.is_some() => (
                change_set_status::FAILED,
                execution_status::UNAVAILABLE,
                Some(early_validation::failed_reason(&[
                    early_validation::RESOURCE_EXISTENCE_CHECK,
                ])),
            ),
            None if changes.is_empty() => (
                change_set_status::FAILED,
                execution_status::UNAVAILABLE,
                Some(crate::state::NO_CHANGES_REASON.to_string()),
            ),
            None => (
                change_set_status::CREATE_COMPLETE,
                execution_status::AVAILABLE,
                None,
            ),
        };
        let stack_name = store
            .resolve(&region, &stack_id)
            .map(|stack| stack.stack_name.clone())
            .unwrap_or_else(|| input.stack_name.clone());
        // "The drift status of the stack when the change set was created": read off the
        // stack now, so a later detection on the stack does not rewrite what this change
        // set reports.
        let stack_drift_status = store
            .resolve(&region, &stack_id)
            .map_or_else(
                || crate::state::DRIFT_STATUS_NOT_CHECKED.to_string(),
                |stack| stack.drift_status.clone(),
            );
        store.change_sets.push(StoredChangeSet {
            stack_drift_status,
            change_set_id: change_set_id.clone(),
            change_set_name: input.change_set_name.clone(),
            stack_id: stack_id.clone(),
            stack_name,
            change_set_type: stored_change_set_type.to_string(),
            description: input.description.clone(),
            template_body,
            processed_template,
            parameters,
            capabilities,
            tags,
            notification_arns,
            role_arn: input.role_arn.clone(),
            rollback_configuration,
            deployment_config,
            deployment_mode: input.deployment_mode.clone(),
            resource_types,
            on_stack_failure: input.on_stack_failure.clone(),
            include_nested_stacks: input.include_nested_stacks.unwrap_or(false),
            import_existing_resources: input.import_existing_resources.unwrap_or(false),
            client_token: input.client_token.clone(),
            creation_time: now,
            status: change_set_status::CREATE_PENDING.to_string(),
            status_reason: None,
            execution_status: execution_status::UNAVAILABLE.to_string(),
            pending: Some(crate::state::PendingChangeSetStatus {
                status: Some(settles_to.0.to_string()),
                status_reason: settles_to.2,
                execution_status: Some(settles_to.1.to_string()),
                due_at: now + crate::state::STATUS_TRANSITION_DELAY,
                // Creating a change set is CloudFormation's own work on the plan, not a
                // stack operation, so its conclusion waits for nothing but its own clock.
                awaits_the_stack: false,
            }),
            // Nothing has executed this plan, so there is nothing of it to discard yet.
            plan_discarded: false,
            changes,
            // The Hook invocations CloudFormation would plan while creating this change
            // set: none, because nothing here can activate a Hook or configure one to
            // target an operation (see `state::change_set_hooks`), so there is no Hook for
            // a resource of this plan to be evaluated against.
            hooks: Vec::new(),
        });

        Ok(CreateChangeSetOutput {
            id: Some(change_set_id),
            stack_id: Some(stack_id),
        })
    }
}
