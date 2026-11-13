// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudformation-cloudformation service.
//!
//! One store holds every stack this emulator has ever created, in creation order, so
//! `CreateStack` / `UpdateStack` / `DeleteStack` write exactly the rows
//! `DescribeStacks` / `ListStacks` / `GetTemplate` / `DescribeStackEvents` /
//! `DescribeStackResources` / `ListStackResources` read back.
//! A deleted stack is NOT dropped: CloudFormation keeps a deleted stack's summary,
//! template, events and resources addressable by its stack id (`ListStacks`: "Summary
//! information for stacks that have been deleted is kept for 90 days"; `GetTemplate`:
//! "You can get the template for running or deleted stacks"; `DescribeStackResources`:
//! "For deleted stacks, DescribeStackResources returns resource information for up to 90
//! days after the stack has been deleted"), so the single `stacks` vector carries
//! live and deleted stacks alike and `StoredStack::is_live` is what separates them.
//!
//! ## One namespace per Region
//!
//! A stack belongs to the Region the call that created it was signed for
//! (`StoredStack::region`), and the store is read THROUGH that Region: a stack name is
//! "unique in the Region in which you are creating the stack", so the same name in a second
//! Region is a second stack and both coexist, while a read signed for one Region resolves
//! nothing another Region holds. Everything that resolves a stack goes through
//! [`ComAmazonawsCloudformationCloudformationState::resolve`] /
//! [`ComAmazonawsCloudformationCloudformationState::live_stacks`] /
//! [`ComAmazonawsCloudformationCloudformationState::stacks_in`], which all take that Region,
//! so no operation can read across it by accident.
//!
//! Beside the stacks it holds every CHANGE SET, which is a persisted plan for one stack:
//! `CreateChangeSet` writes the template and configuration it would apply plus the
//! resource-level change list it computed, `DescribeChangeSet` / `GetTemplate` read them
//! back, and `ExecuteChangeSet` is what copies the plan onto the stack. A change set lives
//! as long as its stack does — CloudFormation deletes a stack's change sets with the
//! stack, which
//! [`ComAmazonawsCloudformationCloudformationState::settle`] enforces.
//!
//! ## Cross-stack references
//!
//! A stack's outputs that declare an `Export` name are what `ListExports` reports, and the
//! other half of that relation lives on the stack too: [`StoredStack::imports`] is the
//! exported output values the stack's deployed template resolved a `Fn::ImportValue`
//! against, so [`ComAmazonawsCloudformationCloudformationState::importing_stacks`] is what
//! `ListImports` answers from and
//! [`ComAmazonawsCloudformationCloudformationState::export_in_use`] is what FAILS a
//! deletion that would remove an export another stack still imports. Both halves are
//! derived from the stacks themselves rather than from an index of their own, so an export
//! and its importers appear when a deployment completes and go when the stack goes.
//!
//! An export is not the only way one stack reads another's output: `Fn::GetStackOutput`
//! reads one by the STACK and the output key, so it needs no `Export` at all
//! ([`stack_outputs`]). That reference is resolved where the template is deployed and KEPT
//! by the reading stack ([`StoredStack::stack_outputs`]) rather than re-read, which is what
//! makes the value the reader reports its own deployment's.
//!
//! ## Bounds
//!
//! Every growth site in this store is bounded, because a caller chooses both how many
//! records it writes and how big each one is: [`MAX_STACKS`] live stacks per Region,
//! [`MAX_LIVE_STACKS`] across every Region and [`MAX_CONCURRENT_RESOURCES`] resources
//! across them (past any of those `CreateStack` answers the modelled
//! `LimitExceededException`, and the per-Region stack ceiling and the resource ceiling are
//! what `DescribeAccountLimits` reports as this account's limits — see
//! [`ComAmazonawsCloudformationCloudformationState::account_limits`]),
//! [`MAX_DELETED_STACKS_RETAINED`] completed deletions (the oldest deletion is reclaimed
//! past that, which is this emulator's stand-in for CloudFormation's 90-day retention of
//! deleted-stack summaries), [`MAX_EVENTS_PER_STACK`] events per stack (oldest-first),
//! one [`timeline`] step per resource transition the operation in flight recorded, which
//! is retired with that operation,
//! one rollback plan per stack with an update in flight and one unreverted set per stack
//! whose rollback failed ([`OwedRollback`], each one entry per resource that update
//! declared, both retired with the operation that holds them),
//! [`MAX_SIGNALS_PER_STACK`] recorded resource signals per stack (oldest-first),
//! [`MAX_CLIENT_REQUEST_TOKENS_PER_STACK`] client request tokens remembered per stack
//! (oldest-first),
//! [`TEMPLATE_BODY_MAX_BYTES`] per stored template body (the documented `TemplateBody`
//! maximum, which the model carries no `@length` for), [`MAX_CHANGE_SETS_PER_STACK`]
//! change sets per stack and [`MAX_CHANGE_SETS`] across the store (past either
//! `CreateChangeSet` answers the modelled `LimitExceededException`),
//! [`template::MAX_EXPANDED_ENTRIES`] / [`template::MAX_EXPANDED_BYTES`] /
//! [`template::MAX_EXPANSION_DEPTH`] on what a template's transforms may expand to (the
//! expansion is multiplicative, so the submitted body's own bound does not bound it),
//! [`MAX_EVENT_PROPERTIES_BYTES`] of `ResourceProperties` per event and
//! [`MAX_EVENT_PROPERTIES_BYTES_PER_STACK`] across one stack's log (oldest-first, since
//! that document is caller-sized as well as caller-counted),
//! [`PAGE_SIZE`] items per paginated response and [`MAX_DESCRIBED_STACK_RESOURCES`]
//! resources per `DescribeStackResources` response (the documented cap on that
//! unpaginated read), [`hook_results::MAX_HOOK_RESULTS`] recorded Hook invocation results
//! each carrying at most [`hook_results::MAX_ANNOTATIONS_PER_RESULT`] annotations (both
//! oldest-first),
//! [`change_set_hooks::MAX_CHANGE_SET_HOOKS`] Hook invocations planned for one change set,
//! [`stack_sets::MAX_STACK_SETS`] stack sets with
//! [`stack_sets::MAX_INSTANCES_PER_STACK_SET`] stack instances and
//! [`stack_sets::MAX_OPERATIONS_PER_STACK_SET`] recorded operations each (all
//! oldest-first),
//! [`handler_progress::MAX_HANDLER_INVOCATIONS`] resource-handler invocations with a
//! progress record (oldest-first),
//! [`resource_scans::MAX_RESOURCE_SCANS`] recorded resource scans (oldest-first), each
//! one carrying the resources this store already holds plus at most
//! [`resource_inventory::MAX_INVENTORIED_RESOURCES`] read from the services that own them,
//! and so bounded by the ceilings above,
//! [`stack_refactors::MAX_STACK_REFACTORS`] stack refactors (oldest-first), each one carrying
//! at most [`stack_refactors::MAX_STACKS_PER_REFACTOR`] stack definitions and
//! [`stack_refactors::MAX_RESOURCE_MAPPINGS_PER_REFACTOR`] resource mappings,
//! and [`registry::MAX_PUBLISHED_VERSIONS`] versions published onto one public extension
//! record (oldest-first), the registry's one caller-counted growth site.
//!
//! Reading a stack's template body and evaluating what it declares is bounded by the
//! [`template`] module's own caps (nodes, depth, the documented per-template quotas and
//! one evaluation budget shared by every expression of one deployment).
//!
//! ## Status transitions the clock owes
//!
//! CloudFormation answers `CreateStack` / `UpdateStack` / `DeleteStack` while the
//! operation is still in progress and settles the stack a moment later. Each call
//! therefore records a [`PendingStatus`] with the instant its terminal status becomes
//! due, and every operation calls [`ComAmazonawsCloudformationCloudformationState::settle`]
//! first: a caller that reads straight after the write still sees `*_IN_PROGRESS`, and
//! one that reads after the due instant sees the terminal status and the event that
//! carries it. Nothing sleeps, and no read invents a transition the clock had not
//! already made due.
//!
//! ## The template pipeline, the resources it declares, and their events
//!
//! A stack deploys the PROCESSED stage of its template, not the submitted body:
//! [`template::process`] computes that stage (see the transform section of that module for
//! which transforms this service can apply and what it answers for the ones it cannot),
//! and `GetTemplate` serves whichever stage the request asks for.
//!
//! A CHANGE SET is planned and executed against that same stage: `CreateChangeSet`
//! computes it once and stores it beside the submitted body
//! ([`StoredChangeSet::processed_template`]), so the plan a caller reviews, the template
//! `GetTemplate` serves for that change set and the deployment `ExecuteChangeSet` carries
//! out are one document rather than three readings of one body.
//!
//! A change set's own creation is asynchronous the same way ("After the CreateChangeSet
//! call successfully completes, CloudFormation starts creating the change set"), so it
//! carries a [`PendingChangeSetStatus`]: it is answered `CREATE_PENDING` and settles to
//! `CREATE_COMPLETE` — or to `FAILED`, when the plan it computed is empty — which is what
//! `ExecuteChangeSet` waits for.
//!
//! What a create or update DEPLOYS — the resources that stage declares and the outputs
//! evaluated over them (see [`template`]) — rides on that same clock: the call computes
//! the deployment ([`StoredStack::deploy_template`]) and `settle` installs it with the
//! terminal status, so a stack always reports the resources and outputs of the template
//! that finished deploying. An operation that could NOT create a resource it declares
//! reaches a FAILED terminal status instead ([`StoredStack::fail_transition`]), on the same
//! clock: a failed create resolves no outputs and reports no identifier for what was never
//! created, and a failed update leaves the template in force alone.
//!
//! The operation's WALK over those resources rides the same clock, one step at a time: a
//! stack operation acts on the template's resources in dependency order, and each
//! resource's `*_IN_PROGRESS` event falls due when the walk SCHEDULED it and its terminal
//! event when it FINISHED ([`timeline`]). `settle` is what records each of them at that
//! instant, so `DescribeStackEvents` reports a deployment as far as it has got — resources
//! with no dependency between them in flight together, a dependent only after its
//! dependency, and a wait condition for as long as it waits — rather than the whole
//! resource set at the single instant the operation settles.
//!
//! CREATING those resources in the services that own them is [`provisioning`], which the
//! deploying operation drives with no lock held — and which leaves the stack
//! `CREATE_FAILED` with the owning service's own refusal as the reason rather than
//! reporting a success whose resources do not exist.
//!
//! A stack operation therefore owes more than its own two events. As each resource
//! transitions in the service that owns it, `provisioning` records that transition
//! ([`provisioning::ResourceTransition`]) and the operation plans it onto the stack's walk
//! ([`StoredStack::schedule_resource_events`]), from which [`StoredStack::settle`] appends
//! it to the event log at the instant the walk reached it — the resource's logical id, its
//! type, the properties it was deployed with and the owning service's own refusal where it
//! failed —
//! so each of those events sits between the stack's opening and terminal ones, and all of
//! them carry one token ("All events triggered by a given stack operation are assigned the
//! same client request token"). A resource the operation does not act on reports nothing,
//! and a template this service cannot read declares nothing at all, so an operation
//! deploying one acts on no resource rather than reporting creations or deletions nobody
//! asked about.
//!
//! A resource's record here and the resource ITSELF are two things: `provisioning` asks
//! the owning service to create every type it has a provisioner for, so a
//! `CREATE_COMPLETE` for an `AWS::SSM::Parameter` means CloudFormation recorded the
//! resource AND that service really holds it ([`StoredResource::materialised`] is what says
//! which). A declared type with no provisioner is recorded and nothing is claimed of any
//! service for it: the record then states what the template declares, never that something
//! was created elsewhere.
//!
//! The expansion of a transform is bounded too: see [`template::MAX_EXPANDED_ENTRIES`],
//! [`template::MAX_EXPANDED_BYTES`] and [`template::MAX_EXPANSION_DEPTH`]. A handler
//! computes it BEFORE it takes the store lock, so one caller's template never blocks
//! another caller's request; a stage already computed is read from the store rather than
//! recomputed, and NO read path expands a template.
//!
//! ## The extension registry
//!
//! Beside the stacks and their change sets the store holds this account and Region's
//! EXTENSION REGISTRY — the resource types, modules and Hooks it has registered or
//! activated, and the versions of each (see [`registry`]). It is a store of its own, not a
//! stack's: nothing in a registry record settles on the stack clock, and `DescribeType`
//! reads it the way `DescribeStacks` reads the stacks. The one transition a registry record
//! owes is a clock of its own: `TestType` starts the contract tests of one extension
//! version and the conclusion they reach becomes due [`STATUS_TRANSITION_DELAY`] later, so
//! `DescribeType` reports `IN_PROGRESS` to a caller reading straight after the call and the
//! conclusion to one reading after that instant (see [`registry::PendingTypeTests`]).
//!
//! It also holds the REGISTRATION REQUESTS of that registry
//! ([`registry::StoredTypeRegistration`]): starting one is answered with the registration
//! token it is addressable by, so the request is a record beside the version it registers —
//! `ListTypeRegistrations` lists those tokens and `DescribeTypeRegistration` reads one
//! request back. A request rides a clock of its own too, answered `IN_PROGRESS` and
//! concluded [`STATUS_TRANSITION_DELAY`] later, and the set is bounded at
//! [`registry::MAX_TYPE_REGISTRATIONS`], oldest reclaimed first.
//!
//! ## The publisher registration
//!
//! Beside the registry it holds this ACCOUNT's extension-publisher registration
//! ([`publisher`]), which `RegisterPublisher` writes and `DescribePublisher` reads back.
//! There is at most one of it — a second registration replaces the record rather than
//! adding one, so it is not a growth site — and it is NOT namespaced by Region: the
//! publisher id it carries "applies to your account in all Amazon Web Services Regions".
//!
//! ## Drift detection
//!
//! It also holds every DRIFT DETECTION a caller has started ([`StoredDriftDetection`]):
//! `DetectStackDrift` records one and hands back its id, [`drift`] is what compares each
//! covered resource against the service that owns it, and
//! `DescribeStackDriftDetectionStatus` reads the detection's own status back. That status
//! rides the stack clock — answered `DETECTION_IN_PROGRESS` and concluded
//! [`DRIFT_DETECTION_DELAY`] later, the way AWS answers a detection that is still running
//! — while what the check FOUND is recorded as of the check
//! ([`ComAmazonawsCloudformationCloudformationState::apply_drift_result`]) onto the stack
//! (`Stack$DriftInformation`) and each resource it covered
//! (`StackResource$DriftInformation`), where every stack read reports it.
//!
//! ## Generated templates
//!
//! Beside those it holds this account and Region's GENERATED TEMPLATES
//! ([`generated_template`]): `CreateGeneratedTemplate` records one and hands back its id,
//! and `DescribeGeneratedTemplate` reads the progress of its generation back. The
//! generation rides a clock of its own — answered in flight and concluded
//! [`generated_template::GENERATION_DELAY`] later — which is also what makes the modelled
//! cap on CONCURRENT generations transient: it counts the records still in a pending or
//! in-progress status, so it lifts as those conclude. `DeleteGeneratedTemplate` removes a
//! record outright — nothing in that feature reports a generated template that was deleted,
//! so no read keeps one addressable the way a deleted stack's summary is kept.
//!
//! ## Stack refactors
//!
//! It also holds this account and Region's STACK REFACTORS ([`stack_refactors`]):
//! `CreateStackRefactor` records one across the stacks it names and hands back its id, and
//! `DescribeStackRefactor` reads the refactor's own status back. Its creation rides a clock
//! of its own — answered `CREATE_IN_PROGRESS` with nothing executable and concluded
//! [`STATUS_TRANSITION_DELAY`] later — and the conclusion is computed once, by the call that
//! creates the refactor, against the stacks this store held at that instant.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

/// Creating the resources a stack's template declares in the services that own them, and
/// deleting them when the stack is deleted.
pub mod provisioning;
/// Reading a stack's template body, and evaluating the resources and outputs it
/// declares.
pub mod template;

/// The order a template body declares the entries of one of its sections in, which is part
/// of what a client reads back about the template.
pub mod declaration_order;

/// What a submitted template is held to before a stack operation acts on it: the
/// template-validation rules CloudFormation applies synchronously, and the refusal each
/// one answers.
pub mod template_validation;

/// What a parameter value in force on a stack operation is held to: the constraints the
/// template's own declaration of that parameter states.
pub mod parameter_constraints;

/// The resource types a stack operation may work with: the `ResourceTypes` a request states,
/// and the template it is weighed against.
pub mod resource_type_allow_list;

/// The pre-deployment validation a stack operation's resources are held to: the property
/// values a resource type's own contract admits.
pub mod property_validation;

/// What a client reads ABOUT a template before it deploys one: the capabilities its
/// declarations require, the resource types it defines and the properties an import of each
/// of those types takes.
pub mod template_summary;

/// The primary identifier of a resource type: the properties whose values identify one
/// resource of that type.
pub mod primary_identifier;

/// The `AWS::Serverless-2016-10-31` (AWS SAM) transform: the resources that macro's
/// expansion puts in place of a template's `AWS::Serverless::*` declarations, applied
/// before anything reads the template.
pub mod sam;

/// The `AWS::Include` transform: the Amazon S3 snippet an `Fn::Transform` names, read and
/// spliced into the template by the operation that submits it, before anything is
/// provisioned.
pub mod include;

/// A template submitted BY REFERENCE: the Amazon S3 object a `TemplateURL` addresses, read
/// so the operation goes on exactly as it would with an inline `TemplateBody`.
pub mod template_url;

/// Computing a change set: comparing the stack's template and parameters with the ones a
/// change set submits, and the resource-level change list that comparison answers.
pub mod change_plan;

/// The stack policy in force on a stack: which of an update's planned resource actions it
/// permits, and how a denied one is reported.
pub mod stack_policy;

/// The `{{resolve:...}}` dynamic references a resource's properties carry: the store each
/// one is resolved against as the resource is deployed, and the resource failure a
/// reference that store cannot satisfy is.
pub mod dynamic_references;

/// The template parameters whose value lives in AWS Systems Manager Parameter Store: the
/// name each one addresses, and the value a deployment reads for it.
pub mod ssm_parameters;

/// One entry per resource type this emulator really creates: which service owns it, and
/// the create and delete calls that make its resources exist.
mod provisioner;

/// Detecting whether a stack's resources still match the configuration its template
/// declared, by reading each one from the service that owns it.
pub mod drift;

/// Which resource TYPES a template may declare at all: the catalogue CloudFormation
/// publishes, and the refusal a template declaring anything else is held to.
pub mod resource_types;

/// The extension registry: the registered and activated extensions this account and
/// Region holds, and the versions of each.
pub mod registry;

/// This account's extension-publisher registration: what `RegisterPublisher` writes and
/// `DescribePublisher` reads back.
pub mod publisher;

/// The resource schema the registry holds for a resource type: the document that defines
/// which properties the type has.
pub mod resource_schema;

/// The Hook invocation results this account and Region holds, which `ListHookResults`
/// reports summaries of.
pub mod hook_results;

/// The resource scans this account and Region holds, which `ListResourceScans` reports
/// summaries of.
pub mod resource_scans;

/// The resources this account holds in one Region as the services that OWN them report
/// them, which is what a resource scan reads beside this service's own stack records.
pub mod resource_inventory;

/// Which exported output values another live stack IMPORTS, and what a stack operation
/// that would withdraw one answers.
pub mod exports;

/// Reading one OUTPUT of another stack of the same Region, which is what the
/// `Fn::GetStackOutput` function this service resolves answers.
pub mod stack_outputs;

/// The Hook invocations planned for one change set, which `DescribeChangeSetHooks`
/// reports.
pub mod change_set_hooks;

/// The PRE-DEPLOYMENT validation a change set is held to before it is offered for
/// execution: what `DisableValidation` turns off, and the `StatusReason` a change set that
/// did not pass it carries.
pub mod early_validation;

/// Which stack already manages a physical resource name, and the refusal a create under a
/// name that is taken earns.
pub mod physical_name;

/// Bringing resources that ALREADY EXIST under a stack's management: what an IMPORT change
/// set plans and what `ImportExistingResources` adopts.
pub mod resource_import;

/// The progress a resource handler reports for one handler invocation, which
/// `RecordHandlerProgress` records against the bearer token that invocation was issued.
pub mod handler_progress;

/// The generated templates this account and Region holds: what `CreateGeneratedTemplate`
/// starts and `DescribeGeneratedTemplate` reads the progress of.
pub mod generated_template;

/// The stack refactors this account and Region holds: what `CreateStackRefactor` plans
/// across several stacks and `DescribeStackRefactor` reads the status of.
pub mod stack_refactors;

/// The resource-signal waits of one stack operation: the barrier an
/// `AWS::CloudFormation::WaitCondition` or a `CreationPolicy`'s `ResourceSignal` puts in a
/// create or an update, and the signals `SignalResource` releases it with.
pub mod wait_conditions;

/// The rollback TRIGGERS of one stack operation: the CloudWatch alarms a
/// `RollbackConfiguration` names, and the rollback an alarm in `ALARM` forces.
pub mod rollback_triggers;

/// The stabilisation of one stack operation: the resources whose owning service has
/// answered the create but does not report the resource ready yet, and the ones whose
/// delete it has answered while still holding them, which hold the operation open until it
/// reports otherwise.
pub mod stabilising;

/// Receiving a custom resource provider's answer: the one write the `ResponseURL` this
/// service publishes for a provider request performs on this store.
pub mod provider_responses;

/// Carrying out what a landed provider answer owes — the requests the operation behind it
/// still has to publish — without waiting for the next client call.
pub mod provider_driver;

/// The permissions a stack operation runs under: the identity policies of the stack's
/// service role, which decide the calls a deployment makes in the services that own the
/// stack's resources.
pub mod service_role;

/// The stack sets this account and Region holds, and the stack instances each one carries,
/// which `ListStackInstances` reports summaries of, plus the operations performed on each,
/// which `ListStackSetOperations` reports summaries of.
pub mod stack_sets;

/// TRUSTED ACCESS with Organizations: the organization-wide setting the StackSets
/// organizations-access operations write and read back, and who may call them as a delegated
/// administrator.
pub mod organizations_access;

/// Rolling one stack-set `UPDATE` out over the stack instances it acts on: the half
/// `UpdateStackSet` and `UpdateStackInstances` share.
pub mod stack_set_rollout;

/// The STACK SET status-change events this service publishes onto the account's default
/// EventBridge bus: one per change of a stack set, of a stack-set operation and of a stack
/// instance ([`stack_set_events`]).
pub mod stack_set_events;

/// The rule a CALLER's `Tags` are held to: the tag keys AWS reserves for itself, which no
/// operation that takes a `Tags` list admits from a request.
pub mod stack_tags;

/// How this service holds the FREE TEXT a caller hands it to store: as ASCII, so a codepoint
/// outside ASCII reads back as one `?`.
pub mod ascii_text;
pub use ascii_text::ascii_text;

/// The TIMELINE of one stack operation's walk over a stack's resources: when each resource
/// was scheduled and when it finished, which is what the per-resource `StackEvent`s of that
/// operation are stamped with.
pub mod timeline;

/// The STACK EVENT NOTIFICATIONS a stack's `NotificationARNs` owe: the Amazon SNS message
/// published for every `StackEvent` of a stack that names topics.
pub mod stack_notifications;

/// The constraints a stack IDENTIFIER a request names is held to, checked before any lookup
/// ([`stack_identifier`]).
pub mod stack_identifier;
pub use stack_identifier::{
    refuse_malformed_identifier, STACK_IDENTIFIER_PATTERN, STACK_NAME_OR_ID_PATH, STACK_NAME_PATH,
};

/// How long after the call that starts it a stack's terminal status becomes due.
///
/// Chosen (CloudFormation's real stack operations take as long as the resources do, so
/// there is no documented value to honour): long enough that a caller acting straight
/// after a create, update or delete observes the `*_IN_PROGRESS` status and the state
/// errors that go with it, short enough that a test can wait for it.
pub const STATUS_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The longest this emulator holds ONE stack operation open waiting for ANOTHER SERVICE of
/// this emulator to report a resource ready, or gone.
///
/// Chosen, exactly as [`STATUS_TRANSITION_DELAY`] is, and the ceiling of the waits whose end
/// nothing published states: the one for a resource whose owning service has not reported it
/// ready, or gone ([`stabilising::CREATE_STABILISATION_WINDOW`],
/// [`stabilising::REMOVAL_STABILISATION_WINDOW`]). Long enough that every span the services
/// of this emulator owe a resource falls due inside it many times over; short enough that a
/// resource nothing will ever bring up fails the operation inside a client's poll loop
/// instead of outliving it, so every status a client polls reaches a terminal value.
///
/// It is WALL-CLOCK, so "many times over" has to hold on the slowest host the emulator runs
/// on and not only on an idle one: a resource whose coming-up runs work of its own through
/// this emulator — a rotation that invokes a function four times, a nested stack that deploys
/// a stack — takes as long as that work takes, and that work is itself slower on a machine
/// that is busy. The value is therefore a multiple of the longest such chain rather than a
/// tight fit around it; a resource no service will ever bring up still fails well inside a
/// client's poll loop.
///
/// The wait for the success signals a resource's DECLARATION asks for is not bounded by
/// this: that span is the one the template stated, so it runs for exactly as long as it asked
/// ([`wait_conditions::WaitConditionBarrier::deadline`]) and gives up at the deadline its own
/// `Timeout` names.
pub const MAX_HELD_WAIT: Duration = Duration::from_secs(90);

/// The region an unsigned request is served as. The emulator's own default; a signed
/// request is served in the region of its SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`awsService.arnNamespace` in the model).
pub const ARN_NAMESPACE: &str = "cloudformation";

/// The resource type CloudFormation reports on a stack-level event.
pub const STACK_RESOURCE_TYPE: &str = "AWS::CloudFormation::Stack";

/// The capability a caller acknowledges to create or update a stack directly from a
/// template that declares a transform.
///
/// `CreateStackInput$Capabilities`: "Some template contain macros. [...] If your stack
/// template contains one or more macros, and you choose to create a stack directly from
/// the processed template, without first reviewing the resulting changes in a change set,
/// you must acknowledge this capability. This includes the AWS::Include and AWS::Serverless
/// transforms, which are macros hosted by CloudFormation."
pub const CAPABILITY_AUTO_EXPAND: &str = "CAPABILITY_AUTO_EXPAND";

/// The reason CloudFormation records on the second `CREATE_IN_PROGRESS` event of a
/// resource — the one that reports the create request reached the resource's provisioner.
pub const RESOURCE_CREATION_INITIATED: &str = "Resource creation Initiated";

/// `StackDriftStatus` for a stack no drift detection has run against yet:
/// "CloudFormation hasn't checked if the stack differs from its expected template
/// configuration."
pub const DRIFT_STATUS_NOT_CHECKED: &str = "NOT_CHECKED";

/// `StackDriftStatus` for a stack whose "actual configuration matches its expected
/// template configuration".
pub const DRIFT_STATUS_IN_SYNC: &str = "IN_SYNC";

/// `StackDriftStatus` for a stack that "differs from its expected template
/// configuration".
pub const DRIFT_STATUS_DRIFTED: &str = "DRIFTED";

/// `StackDriftStatus` for a stack CloudFormation "could not run drift detection for a
/// resource in".
pub const DRIFT_STATUS_UNKNOWN: &str = "UNKNOWN";

/// The `StackResourceDriftStatus` values this service leaves a resource's drift result in
/// (all from the model's `StackResourceDriftStatus` enum, documented member by member on
/// `StackResourceDriftInformation$StackResourceDriftStatus`).
pub mod resource_drift_status {
    /// "The resource's actual configuration matches its expected configuration."
    pub const IN_SYNC: &str = "IN_SYNC";
    /// "The resource differs from its expected configuration."
    pub const MODIFIED: &str = "MODIFIED";
    /// "The resource differs from its expected configuration in that it has been
    /// deleted."
    pub const DELETED: &str = "DELETED";
    /// "CloudFormation has not checked if the resource differs from its expected
    /// configuration. Any resources that do not currently support drift detection have a
    /// status of NOT_CHECKED."
    pub const NOT_CHECKED: &str = "NOT_CHECKED";
    /// The resource's check could not be run at all.
    pub const UNKNOWN: &str = "UNKNOWN";
}

/// The model's `StackDriftDetectionStatus`: the status of one drift-detection operation.
pub mod drift_detection_status {
    /// "The stack drift detection operation has not yet concluded."
    pub const DETECTION_IN_PROGRESS: &str = "DETECTION_IN_PROGRESS";
    /// "The stack drift detection operation has successfully completed for all resources
    /// in the stack that support drift detection."
    pub const DETECTION_COMPLETE: &str = "DETECTION_COMPLETE";
    /// "The stack drift detection operation has failed for at least one resource in the
    /// stack."
    pub const DETECTION_FAILED: &str = "DETECTION_FAILED";
}

/// How long after `DetectStackDrift` the detection's own completion becomes due.
///
/// Chosen ("DetectStackDrift can take up to several minutes, depending on the number of
/// resources contained within the stack", so there is no documented value to honour):
/// non-zero, so a caller polling `DescribeStackDriftDetectionStatus` straight after the
/// call still observes `DETECTION_IN_PROGRESS`, and short enough that the poll after it
/// sees the result.
pub const DRIFT_DETECTION_DELAY: Duration = Duration::from_millis(250);

/// The most drift-detection results this service retains, oldest reclaimed first.
///
/// `DetectStackDrift`: "CloudFormation generates new results, with a new drift detection
/// ID, each time this operation is run. However, the number of drift results
/// CloudFormation retains for any given stack, and for how long, may vary." A caller
/// chooses how many detections it runs, so the store bounds them by count. Chosen, far
/// above any client workflow.
pub const MAX_DRIFT_DETECTIONS: usize = 200;

/// `DeploymentConfig$Mode` for a stack operation that did not state one.
///
/// `DeploymentConfigMode`: "STANDARD — Use the standard deployment behavior, ensuring
/// resources are ready to serve traffic before completing the operation. This is the
/// default. You do not need to specify this value explicitly."
pub const DEPLOYMENT_MODE_STANDARD: &str = "STANDARD";

/// `DeploymentConfig$Mode` for an operation that completes once the configuration is
/// applied.
///
/// `DeploymentConfigMode`: "EXPRESS — Complete the stack operation when resource
/// configuration is applied, without waiting for resources to become ready to serve
/// traffic. Resources continue becoming ready in the background."
pub const DEPLOYMENT_MODE_EXPRESS: &str = "EXPRESS";

/// The most LIVE stacks this emulator holds.
///
/// CloudFormation meters stacks per Region and models `LimitExceededException` on
/// `CreateStack` for exactly that case ("The quota for the resource has already been
/// reached. For information about resource and stack limitations, see CloudFormation
/// quotas"); the documented default stack quota is 2,000 per Region and is adjustable
/// through Service Quotas, so this is that default as this emulator's own ceiling. It is
/// what bounds the live half of the store: one record, one template body and one event
/// log per live stack.
pub const MAX_STACKS: usize = 2_000;

/// The most resources the account's stacks hold at once.
///
/// CloudFormation meters concurrent resources per account — it is one of the three
/// per-account limits `AccountLimit` documents ("Number of concurrent resources") — and the
/// documented default quota is 8,500, adjustable through Service Quotas, so this is that
/// default as this emulator's own ceiling: the number `DescribeAccountLimits` advertises as
/// `ConcurrentResourcesLimit` is the one a write is really measured against. It is what
/// bounds the resource records the store holds across its stacks, and `CreateStack` — the
/// operation that brings new resources into the account and the one that models
/// `LimitExceededException` for a quota already reached — is what refuses a create past it.
pub const MAX_CONCURRENT_RESOURCES: usize = 8_500;

/// The most LIVE stacks this store holds across EVERY Region together.
///
/// [`MAX_STACKS`] is the documented quota, and it is metered per Region — and the Region a
/// request is served in comes from its own credential scope, so the Region is a value the
/// CALLER chooses. A per-Region quota alone would therefore bound only one factor of the
/// store's growth: a caller signing for one Region string after another could keep creating
/// stacks indefinitely. This is the ceiling on the PRODUCT, past which `CreateStack` answers
/// the same modelled `LimitExceededException`.
///
/// It is deliberately a FLAT count that does not mention the Region: whatever a caller signs
/// for, and however many distinct Region strings it invents, this store holds at most this
/// many live stacks — one record, one bounded template body
/// ([`TEMPLATE_BODY_MAX_BYTES`]) and one bounded event log ([`MAX_EVENTS_PER_STACK`])
/// each — so the Region cannot multiply anything. Chosen, like every count here: a small
/// multiple of the per-Region quota, so no single Region's documented quota is cut short, and
/// far above any client workflow.
pub const MAX_LIVE_STACKS: usize = 5 * MAX_STACKS;

/// The most stacks in `DELETE_COMPLETE` this emulator keeps.
///
/// CloudFormation keeps a deleted stack's summary, template and events addressable by
/// stack id for 90 days and then reclaims them. This emulator has no 90-day clock worth
/// emulating, so it bounds the same retention by COUNT instead: past this many completed
/// deletions the oldest deletion is reclaimed, so a caller that creates and deletes in a
/// loop cannot grow the store without limit. Chosen, like every count here; it sits far
/// above any client workflow.
pub const MAX_DELETED_STACKS_RETAINED: usize = 200;

/// The most events one stack retains, oldest evicted first.
///
/// CloudFormation bounds a stack's event history by age, not by count; a caller here can
/// drive an unbounded number of updates against one stack, so the emulator bounds the log
/// by count. Chosen, and deliberately above the worst case of ONE operation — a stack at
/// the documented [`template::MAX_RESOURCES_PER_TEMPLATE`] quota owes `2 + 3 × 500` =
/// 1,502 events for a single create — with the rest as headroom for the operations whose
/// events a client is reading beside it, so eviction never discards part of the operation
/// it is reading.
pub const MAX_EVENTS_PER_STACK: usize = 4 * template::MAX_RESOURCES_PER_TEMPLATE;

/// The most operations one stack remembers, oldest evicted first.
///
/// A client chooses how many operations it drives against one stack, so the record of
/// them is bounded exactly as the event log above is — and generously above it in the
/// sense that matters: the log evicts an operation's record no sooner than it evicts that
/// operation's events, since one operation records at least two of them.
pub const MAX_OPERATIONS_PER_STACK: usize = MAX_EVENTS_PER_STACK / 2;

/// The most bytes of `StackEvent$ResourceProperties` ONE event carries.
///
/// That member is the properties document of one resource, which is caller-sized: it comes
/// out of the template body, whose own ceiling ([`TEMPLATE_BODY_MAX_BYTES`]) a transform's
/// expansion may multiply ([`template::MAX_EXPANDED_BYTES`]). An event is a DIAGNOSTIC, so
/// past this much the event reports no properties rather than a truncated document that is
/// no longer the document (the member is optional, and a cut-off blob would not parse).
/// Chosen: comfortably above the property sets of real resource declarations, and small
/// enough that [`MAX_EVENTS_PER_STACK`] of them is bounded in megabytes rather than
/// hundreds.
pub const MAX_EVENT_PROPERTIES_BYTES: usize = 4 * 1024;

/// The most bytes of `ResourceProperties` ONE stack's whole event log retains.
///
/// The count bound above is what keeps the log's ENTRIES bounded; this is what keeps its
/// BYTES bounded, because a caller chooses how big each retained properties document is as
/// well as how many events it causes. Past it the oldest events are evicted until the log
/// fits, exactly as the count bound evicts. The product of the two factors is therefore
/// this constant and not `MAX_EVENTS_PER_STACK × MAX_EVENT_PROPERTIES_BYTES`.
pub const MAX_EVENT_PROPERTIES_BYTES_PER_STACK: usize = 256 * 1024;

/// The most resource signals one stack retains, oldest evicted first.
///
/// `SignalResource` is a write a client drives: "each signal requires a different unique
/// ID" (`SignalResourceInput$UniqueId`), so the number of DISTINCT signals one stack can
/// be sent is unbounded and the store bounds what it keeps. Chosen: far more than the
/// signals any creation policy waits for.
pub const MAX_SIGNALS_PER_STACK: usize = 1_000;

/// The most client request tokens one stack remembers, oldest evicted first.
///
/// A token is what names ONE operation on a stack, so the number of DISTINCT tokens a
/// stack can be sent is as unbounded as the number of operations a client drives against
/// it, and the store bounds what it keeps. Chosen: far more operations than one stack takes
/// in a session, so a token a client could plausibly retry under is still remembered.
/// Eviction trades the refusal away with the record — a token reclaimed after this many
/// later operations is new to the stack again — which is the deliberate cost of keeping
/// one stack's memory bounded however many operations it is sent.
pub const MAX_CLIENT_REQUEST_TOKENS_PER_STACK: usize = 1_000;

/// The most bytes of template body this service stores.
///
/// `CreateStackInput$TemplateBody`: "Structure that contains the template body with a
/// minimum length of 1 byte and a maximum length of 51,200 bytes." The model carries only
/// `@length(min: 1)`, so the handler is what enforces the documented maximum.
pub const TEMPLATE_BODY_MAX_BYTES: usize = 51_200;

/// The most bytes of template this service READS, whatever member carried it.
///
/// [`TEMPLATE_BODY_MAX_BYTES`] is the quota on a body a request SUBMITS for deployment; this
/// is the size of the biggest template CloudFormation reads at all — "the URL must point to a
/// template (max size: 1 MB)" (`TemplateURL`), the same ceiling the documented quotas put on a
/// template in an Amazon S3 object. An operation that only REPORTS on a template reads a body
/// up to this size: the quota it is not held to is the one on submitting a template for
/// deployment.
pub const TEMPLATE_MAX_BYTES: usize = 1_048_576;

/// How many items one page of a paginated read carries.
///
/// CloudFormation pages `DescribeStacks`, `ListStacks` and `DescribeStackEvents` by
/// response SIZE ("If the output exceeds 1 MB in size"), which is not a count a client can
/// depend on, so this emulator pages by a fixed count instead — chosen, and the thing that
/// keeps one response bounded however large the store is.
pub const PAGE_SIZE: usize = 100;

/// The most resources one `DescribeStackResources` response carries.
///
/// `DescribeStackResources`: "Only the first 100 resources will be returned. If your
/// stack has more resources than this, you should use ListStackResources instead." The
/// operation is not `@paginated`, so this documented cap is the whole of one answer —
/// a stack may declare up to [`template::MAX_RESOURCES_PER_TEMPLATE`] of them.
pub const MAX_DESCRIBED_STACK_RESOURCES: usize = 100;

/// `Err(message)` for a `TemplateBody` past its documented maximum, naming the member and
/// the constraint it failed.
pub fn validate_template_body(template_body: &str) -> Result<(), String> {
    validate_template_body_within(template_body, TEMPLATE_BODY_MAX_BYTES)
}

/// [`validate_template_body`] under the caller's OWN ceiling on the body, naming that ceiling
/// as the constraint the value failed.
///
/// The maximum SIZE of a body is a quota of the API that takes it rather than a property of
/// the template format ([`TEMPLATE_MAX_BYTES`]), so an operation held to a ceiling of its own
/// states that one.
pub fn validate_template_body_within(
    template_body: &str,
    max_bytes: usize,
) -> Result<(), String> {
    if template_body.len() > max_bytes {
        return Err(template_body_too_long_within(Some(template_body), max_bytes));
    }
    Ok(())
}

/// The refusal for a request that carries the template it submits BOTH by value and by
/// reference, as the service words it.
///
/// A template reaches an operation one of two ways — the request's own `TemplateBody` or a
/// `TemplateURL` naming the object that holds it — and a request stating both states two
/// templates, so there is no one body to act on. The sentence is the one the recorded
/// refusals of this condition carry (`ValidateTemplate` and `CreateChangeSet` answer it
/// identically), and it is shared by every operation whose input carries exactly those two
/// members, since the condition is a property of the REQUEST SHAPE rather than of one
/// operation. (An operation that also takes `UsePreviousTemplate` states the three-way
/// condition its own documentation states for every OTHER way of not naming exactly one
/// source, [`ONE_TEMPLATE_SOURCE`].)
pub const BOTH_TEMPLATE_SOURCES: &str = "You cannot specify both Template URL and Template Body.";

/// `Err(message)` for a request that names the template it submits BOTH by value and by
/// reference — the RULE every operation whose input carries that pair is held to, in one
/// place, so neither the condition nor its wording is restated per operation.
pub fn validate_one_template_source(
    template_body: Option<&str>,
    template_url: Option<&str>,
) -> Result<(), String> {
    if template_body.is_some() && template_url.is_some() {
        return Err(BOTH_TEMPLATE_SOURCES.to_string());
    }
    Ok(())
}

/// The refusal a template body past [`TEMPLATE_BODY_MAX_BYTES`] is held to, in the one
/// place that words it: a body submitted by REFERENCE is held to the same ceiling, and the
/// two cannot drift apart about what it is ([`template_url`]).
///
/// A constraint-violation message is BUILT FROM THE REQUEST — `1 validation error detected:
/// Value '<value>' at '<member>' failed to satisfy constraint: <requirement>` — so the
/// member's own value is part of the refusal and not prose: the message is the only place
/// the service reports the offending input back. A template body is the one member of this
/// service whose value is caller-SIZED, so what the message quotes is its leading
/// [`QUOTED_TEMPLATE_PREFIX`] characters followed by the total it elided
/// ([`quoted_template_body`]), rather than a response as large as the request.
/// `submitted` carries the bytes where the refusal is made on a body this service HAS; a
/// body refused by its reported length alone, before any of it was read, states the
/// constraint without a value it never held.
pub fn template_body_too_long(submitted: Option<&str>) -> String {
    template_body_too_long_within(submitted, TEMPLATE_BODY_MAX_BYTES)
}

/// [`template_body_too_long`] naming the ceiling the body was held to, for an operation whose
/// ceiling is one of its own ([`TEMPLATE_MAX_BYTES`]).
pub fn template_body_too_long_within(submitted: Option<&str>, max_bytes: usize) -> String {
    let value = match submitted {
        Some(body) => format!("'{}' ", quoted_template_body(body)),
        None => String::new(),
    };
    format!(
        "1 validation error detected: Value {value}at 'templateBody' failed to satisfy \
         constraint: Member must have length less than or equal to {max_bytes}"
    )
}

/// How much of a template body a refusal quotes before the marker naming how much of it was
/// left out.
///
/// A constraint message's echoed value is BOUNDED wherever the member it is about is one a
/// caller sizes: the framework's own constraint walk quotes a prefix of a long value rather
/// than the value (`vendor/smithy-schema-server/src/constraints.rs`), and the shared rule for
/// what such a message echoes says a service whose member is larger than the default ceiling
/// states its own ceiling (`service_common::validation`). This is that ceiling for the one
/// member of this service a caller sizes: the leading characters of the document the recorded
/// refusals of an over-long `TemplateBody` quote, and a constant, so the refusal's size does
/// not follow the request's.
pub const QUOTED_TEMPLATE_PREFIX: usize = 100;

/// The template body as a refusal QUOTES it: itself where it is no longer than
/// [`QUOTED_TEMPLATE_PREFIX`], else that much of its leading text followed by a marker
/// naming the number of characters the value holds in all.
///
/// A template body is the one member of this service whose value a caller SIZES, and the
/// value is quoted so the caller can see what was refused — which its leading text and its
/// total length answer, while a message as large as the request would make the refusal a
/// second copy of the template. What it keeps is cut at a separator where the kept text has
/// one (so the quote ends on a whole word of the document, which is how a recorded refusal of
/// a body written with separators quotes it), else at the character the budget runs out at,
/// which is what a body written as one unbroken run of characters is quoted by.
pub fn quoted_template_body(body: &str) -> String {
    let characters = body.chars().count();
    if characters <= QUOTED_TEMPLATE_PREFIX {
        return body.to_string();
    }
    let kept: String = body.chars().take(QUOTED_TEMPLATE_PREFIX).collect();
    // Cut back to just past the last separator the kept text carries, keeping it: a quote
    // that ends mid-word says less about the document than one that ends at a boundary of
    // it. The separator's own width is what the cut steps over, so a multi-byte one is
    // never cut inside.
    let separator = kept
        .char_indices()
        .rfind(|(_, character)| character.is_whitespace());
    let kept = match separator {
        Some((at, character)) => kept[..at + character.len_utf8()].to_string(),
        None => kept,
    };
    format!("{kept}...<{characters} chars total>")
}

/// `CAPABILITY_AUTO_EXPAND` when the template names a macro the request has not
/// acknowledged, and nothing when it has (or when there is no macro to acknowledge).
///
/// `CreateStackInput$Capabilities` requires the acknowledgement for a template that
/// "contains one or more macros", and both `CreateStack` and `UpdateStack` model
/// `InsufficientCapabilitiesException` for the request that omits it. The IAM
/// acknowledgements (`CAPABILITY_IAM` / `CAPABILITY_NAMED_IAM`) are a separate rule about
/// a template's RESOURCES and are not decided here.
///
/// A template CONTAINS a macro in either of the two ways one is invoked: the `Transform`
/// section naming it for the whole template, and the `Fn::Transform` intrinsic naming one
/// for part of it. Which templates those are is decided in the ONE place that owns the
/// question ([`template_summary::requires_auto_expand`]), so the acknowledgement a request
/// is held to is the capability the template REPORTS requiring.
pub fn missing_auto_expand_capability(
    template_body: &str,
    acknowledged: &[String],
) -> Vec<String> {
    let declares_transform = template_summary::requires_auto_expand(template_body);
    let acknowledged_auto_expand = acknowledged
        .iter()
        .any(|capability| capability == CAPABILITY_AUTO_EXPAND);
    if declares_transform && !acknowledged_auto_expand {
        return vec![CAPABILITY_AUTO_EXPAND.to_string()];
    }
    Vec::new()
}

/// `Err(message)` for a template body past one of the documented per-template quotas —
/// the rule every operation that SUBMITS a template is held to, before anything is
/// created.
///
/// Those quotas are the ceilings `DescribeAccountLimits` advertises for this account (see
/// [`ComAmazonawsCloudformationCloudformationState::account_limits`]), so a body past one
/// is refused rather than accepted and then deployed with none of what it declares:
/// an advertised ceiling a write is not held to is no ceiling. `Ok(())` for a body this
/// service's reader will not read at all, which is a template-FORM question the submitting
/// operation answers on its own terms.
pub fn validate_template_quotas(template_body: &str) -> Result<(), String> {
    validate_template_quotas_within(template_body, TEMPLATE_BODY_MAX_BYTES)
}

/// [`validate_template_quotas`] under the caller's OWN ceiling on the body
/// ([`template::quota_violation_within`]): the quotas are the same whatever carried the body,
/// and the ceiling is the one the operation reading it is held to.
pub fn validate_template_quotas_within(
    template_body: &str,
    max_bytes: usize,
) -> Result<(), String> {
    match template::quota_violation_within(template_body, max_bytes) {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// `Err(message)` for a template body declaring a resource type CloudFormation does not
/// recognise — the rule every operation that SUBMITS a template is held to, before
/// anything is created ([`resource_types`] is where the recognised set lives).
///
/// `Ok(())` for a body this service's reader will not read at all: a body that is neither
/// JSON nor YAML, or one past a documented per-template quota, declares no resource to
/// hold to the catalogue and is answered by the template-format rules instead.
pub fn validate_resource_types(template_body: &str) -> Result<(), String> {
    match template::parse(template_body) {
        Some(template) => resource_types::refuse_unrecognized(template_body, &template),
        None => Ok(()),
    }
}

/// `Err(message)` for a template declaring a resource type the request's own
/// `ResourceTypes` does not cover — the bound every operation that states that member is
/// held to, before anything is created ([`resource_type_allow_list`] is where the rule
/// lives).
///
/// `Ok(())` for a body this service's reader will not read at all: a body it cannot read
/// declares no type to hold to the list, and is answered by the template-format rules
/// instead.
pub fn validate_allowed_resource_types(
    template_body: &str,
    allowed: &[String],
) -> Result<(), String> {
    match template::parse(template_body) {
        Some(template) => match resource_type_allow_list::refusal(&template, allowed) {
            Some(message) => Err(message),
            None => Ok(()),
        },
        None => Ok(()),
    }
}

/// `Err(message)` for a submitted template body CloudFormation refuses outright — the
/// template-validation rules every operation that SUBMITS a body is held to, before
/// anything is created ([`template_validation`] is where they live).
///
/// `processed` is the stage the submitting operation computed for this body, which is the
/// document the semantic rules read: a template whose transforms this service cannot apply
/// has none, and is left to the stack's own failure rather than refused here.
pub fn validate_submitted_template(
    template_body: &str,
    processed: &template::Processed,
    parameters: &BTreeMap<String, String>,
    region: &str,
) -> Result<(), String> {
    match template_validation::refusal(template_body, processed, parameters, region) {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// `Err(message)` when the parameter SET a stack operation resolved does not match the one
/// its template declares — the rule every operation that SUBMITS a body for deployment is
/// held to, before anything is created or changed
/// ([`parameter_constraints::set_refusal`] is where it lives).
///
/// `in_force` is the set the operation resolved ([`parameters_in_force`]) and `requested` the
/// request's own `Parameters` member: a declared parameter needs a value in force from
/// SOMEWHERE (the request, its own `Default`, or — on an update — the stack's current value),
/// while naming a parameter the template does not declare is something only THIS request can
/// do.
pub fn validate_parameter_set(
    template_body: &str,
    in_force: &[StoredParameter],
    requested: Option<&[crate::ops::types::Parameter]>,
) -> Result<(), String> {
    let resolved: ::std::collections::BTreeSet<&str> = in_force
        .iter()
        .map(|parameter| parameter.key.as_str())
        .collect();
    let stated: ::std::collections::BTreeSet<&str> = requested
        .unwrap_or_default()
        .iter()
        .filter_map(|parameter| parameter.parameter_key.as_deref())
        .collect();
    match parameter_constraints::set_refusal(template_body, &resolved, &stated) {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// `Err(message)` for a template body `ValidateTemplate` refuses — the template-validation
/// rules a body submitted only to be VALIDATED is held to.
///
/// "Validates a specified template" is a verdict on the DOCUMENT, so the rules it applies are
/// the ones the document decides on its own: the pass is the same list in the same order as
/// the one a submitted body is held to, minus what resolving the template's intrinsics would
/// decide ([`template_validation`] owns both readings, so they cannot drift apart).
pub fn validate_template_for_validation(
    template_body: &str,
    processed: &template::Processed,
    parameters: &BTreeMap<String, String>,
    region: &str,
) -> Result<(), String> {
    match template_validation::validation_refusal(template_body, processed, parameters, region) {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// How CloudFormation words the status of a stack whose failed create it is unwinding: the
/// failure the create reported, and then the clause saying the rollback was asked for.
///
/// `CreateStackInput$OnFailure` documents `ROLLBACK` as the default behaviour of a failed
/// create, so the rollback is something the REQUEST asked for — which is what the service
/// says when it reports one. The clause goes where the failure's own sentence leaves room
/// for it, whether that sentence is the resource-failure summary
/// ([`resource_failure_summary`], which ends with its separating space) or a reason of this
/// service's own.
///
/// Worded once, and public so a suite asserting a rolled-back stack's reason reads the same
/// wording rather than spelling a second copy of it.
pub fn rollback_requested(reason: &str) -> String {
    let stated = reason.trim_end();
    format!(
        "{}. Rollback requested by user.",
        stated.strip_suffix('.').unwrap_or(stated)
    )
}

/// The message CloudFormation answers when a mutating stack operation presents a
/// `ClientRequestToken` an operation on that stack already claimed — the
/// `TokenAlreadyExistsException` refusal ("A client request token already exists").
///
/// Worded once and built from the token the request sent, so the operations that enforce
/// the rule cannot drift apart.
pub fn token_already_exists_message(client_request_token: &str) -> String {
    format!(
        "The client request token {client_request_token} has already been used for this \
         stack. Retry the request with a different client request token"
    )
}

/// `Some(message)` when `client_request_token` is one an operation on `stack` already
/// claimed, which is the `TokenAlreadyExistsException` refusal that request owes;
/// `None` for a token new to this stack, and for a request that sent none.
///
/// The rule every mutating stack operation is held to, before the stack is touched: the
/// token "a unique identifier for this request" names identifies ONE operation on ONE stack
/// ([`StoredStack::client_request_token_claim`] is what remembers which), so presenting it
/// for a second operation is the duplicate submission the modelled error names.
pub fn reused_client_request_token(
    stack: &StoredStack,
    client_request_token: Option<&str>,
) -> Option<String> {
    let token = client_request_token?;
    stack.client_request_token_claim(token)?;
    Some(token_already_exists_message(token))
}

/// `None` when `capabilities` — the request's own `Capabilities` — acknowledges everything
/// the submitted template requires; otherwise the `InsufficientCapabilitiesException`
/// message that refuses the request.
///
/// The rule every operation that SUBMITS a template is held to, before anything is
/// created: a template declaring an IAM resource requires that resource's capability to be
/// acknowledged, and one giving such a resource a custom name requires the named form
/// ([`template::required_capabilities`] is where the requirement is computed). A body this
/// service's reader will not read at all declares no resource to require a capability, so
/// it is answered by the template-format rules instead — the same shape as
/// [`validate_resource_types`].
pub fn insufficient_capabilities(template_body: &str, capabilities: &[String]) -> Option<String> {
    let parsed = template::parse(template_body)?;
    let missing = template::missing_capabilities(&parsed, capabilities);
    (!missing.is_empty()).then(|| template::insufficient_capabilities_message(&missing))
}

/// The `LimitName` values CloudFormation meters per account, as the model documents them
/// on `AccountLimit$Name` ("Values: ConcurrentResourcesLimit | StackLimit |
/// StackOutputsLimit") and `AccountLimit` itself lists them ("Number of concurrent
/// resources", "Number of stacks", "Number of stack outputs").
pub mod account_limit_name {
    /// "Number of concurrent resources": [`MAX_CONCURRENT_RESOURCES`](super::MAX_CONCURRENT_RESOURCES).
    pub const CONCURRENT_RESOURCES: &str = "ConcurrentResourcesLimit";
    /// "Number of stacks": [`MAX_STACKS`](super::MAX_STACKS).
    pub const STACK: &str = "StackLimit";
    /// "Number of stack outputs": the outputs one template may declare,
    /// [`template::MAX_OUTPUTS_PER_TEMPLATE`](super::template::MAX_OUTPUTS_PER_TEMPLATE).
    pub const STACK_OUTPUTS: &str = "StackOutputsLimit";
}

/// `StackStatus` values this service puts a stack into (all from the model's
/// `StackStatus` enum).
pub mod status {
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    /// The terminal status of a create this service could not carry out — a template whose
    /// transforms it cannot apply, or one its reader will not read.
    pub const CREATE_FAILED: &str = "CREATE_FAILED";
    pub const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
    pub const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
    /// The terminal status of an update this service could not carry out, for the same
    /// reasons [`CREATE_FAILED`] covers on a create.
    pub const UPDATE_FAILED: &str = "UPDATE_FAILED";
    pub const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
    pub const DELETE_COMPLETE: &str = "DELETE_COMPLETE";
    pub const DELETE_FAILED: &str = "DELETE_FAILED";
    /// The status a stack takes while the resources of a create that FAILED are being
    /// removed again, which is what CloudFormation does unless the request asked it not to
    /// (`CreateStackInput$OnFailure`: "ROLLBACK — Roll back the stack creation. This is the
    /// default").
    pub const ROLLBACK_IN_PROGRESS: &str = "ROLLBACK_IN_PROGRESS";
    /// The terminal status of a create that was rolled back.
    pub const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";
    /// The terminal status of a rollback that could not remove what the failed create had
    /// already made.
    pub const ROLLBACK_FAILED: &str = "ROLLBACK_FAILED";
    /// The status a stack takes while the changes of an UPDATE that failed are being put
    /// back: "if the stack update fails, CloudFormation rolls back the stack to the previous
    /// stack configuration".
    pub const UPDATE_ROLLBACK_IN_PROGRESS: &str = "UPDATE_ROLLBACK_IN_PROGRESS";
    /// The terminal status of an update that was rolled back.
    pub const UPDATE_ROLLBACK_COMPLETE: &str = "UPDATE_ROLLBACK_COMPLETE";
    /// The terminal status of an update rollback that could not put a resource back: "a
    /// stack goes into the UPDATE_ROLLBACK_FAILED state when CloudFormation can't roll back
    /// all changes after a failed stack update" (`ContinueUpdateRollback`), which is the one
    /// status that operation resumes from.
    pub const UPDATE_ROLLBACK_FAILED: &str = "UPDATE_ROLLBACK_FAILED";
    /// The status a stack takes while an update that has applied every change is removing
    /// the resources it REPLACED: "UPDATE_COMPLETE_CLEANUP_IN_PROGRESS — the stack update
    /// was successful, and CloudFormation is deleting the old resources" — so the update's
    /// own work is done and the stack is not settled until the replaced resources are gone.
    pub const UPDATE_COMPLETE_CLEANUP_IN_PROGRESS: &str = "UPDATE_COMPLETE_CLEANUP_IN_PROGRESS";
    /// The same phase of a ROLLBACK: "UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS —
    /// CloudFormation is cleaning up the resources after the rollback", so the revert itself
    /// has landed and the stack is not settled until what the failed operation created is
    /// gone ([`StoredStack::record_rollback_cleanup`](super::StoredStack)).
    pub const UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS: &str =
        "UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS";
    /// The status of a stack a `CREATE` change set brought into existence and nothing has
    /// executed yet (`CreateChangeSetInput$ChangeSetType`: "If you create a change set for
    /// a new stack, CloudFormation creates a stack with a unique stack ID, but no template
    /// or resources. The stack will be in the REVIEW_IN_PROGRESS state until you execute
    /// the change set").
    pub const REVIEW_IN_PROGRESS: &str = "REVIEW_IN_PROGRESS";
    /// The status a stack takes while an IMPORT change set's resources are being adopted.
    pub const IMPORT_IN_PROGRESS: &str = "IMPORT_IN_PROGRESS";
    /// The terminal status of a completed resource import. Nothing in this service puts a
    /// stack into it yet; it is named because it is one of the statuses a stack is STABLE
    /// in ([`super::is_stable_status`]).
    pub const IMPORT_COMPLETE: &str = "IMPORT_COMPLETE";
    /// The terminal status of an import that was rolled back, stable for the same reason
    /// [`IMPORT_COMPLETE`] is.
    pub const IMPORT_ROLLBACK_COMPLETE: &str = "IMPORT_ROLLBACK_COMPLETE";
}

/// Whether a stack operation that left the stack in `status` CARRIED THROUGH the change it
/// was applying.
///
/// The completed create, update and import are the statuses an operation reaches having made
/// the change it set out to make; a failure, and the rollback of one, are statuses the stack
/// reaches with that change NOT applied. What the EXECUTION of a change set ends in follows
/// this: executing a change set is the stack operation it starts, so the model's
/// `EXECUTE_COMPLETE` belongs to one whose operation carried the change through and
/// `EXECUTE_FAILED` to one whose did not ([`StoredChangeSet::settle`]).
pub fn operation_carried_through(status: &str) -> bool {
    matches!(
        status,
        status::CREATE_COMPLETE | status::UPDATE_COMPLETE | status::IMPORT_COMPLETE
    )
}

/// Whether a stack carrying `status` is in a STABLE state — one a rollback can put it back
/// to.
///
/// `RollbackStack` says which those are: "a last known stable state includes any status in a
/// *_COMPLETE", and then lists them — the completed create, the completed update, the
/// completed update rollback and the two completed import statuses. Every other status is a
/// stack that was never settled in a configuration a later operation could return it to: an
/// operation still in flight, a failure, and a deletion.
pub fn is_stable_status(status: &str) -> bool {
    matches!(
        status,
        status::CREATE_COMPLETE
            | status::UPDATE_COMPLETE
            | status::UPDATE_ROLLBACK_COMPLETE
            | status::IMPORT_COMPLETE
            | status::IMPORT_ROLLBACK_COMPLETE
    )
}

/// `ResourceStatus` values this service leaves a stack's resource in, and records a
/// `StackEvent` for (all from the model's `ResourceStatus` enum).
///
/// A stack operation walks the template's resources, so each one goes through the status
/// that OPENS its transition and then the one that ends it: what the stack's own events say
/// about the operation as a whole, these say about one resource of it
/// ([`provisioning::ResourceTransition`]).
pub mod resource_status {
    /// The status a resource the stack's create is still working on reports: the one that
    /// OPENS its create — the one a signal it receives while that create is in flight is
    /// recorded under, and the one a wait condition the operation is waiting AT is in
    /// ([`wait_conditions`](super::wait_conditions)).
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    /// The status of a resource whose creation the service that owns it refused, or a
    /// FAILURE signal ended.
    pub const CREATE_FAILED: &str = "CREATE_FAILED";
    /// [`CREATE_IN_PROGRESS`]'s counterpart while the stack's UPDATE is in flight.
    pub const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
    pub const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
    /// The status of a resource whose update the service that owns it refused, or a FAILURE
    /// signal ended.
    pub const UPDATE_FAILED: &str = "UPDATE_FAILED";
    /// [`CREATE_IN_PROGRESS`]'s counterpart while the resource is being DELETED.
    pub const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
    pub const DELETE_COMPLETE: &str = "DELETE_COMPLETE";
    /// The status of a resource the service that owns it refused to delete.
    pub const DELETE_FAILED: &str = "DELETE_FAILED";
    /// The status of a resource a deletion left in place rather than deleting: the one a
    /// FORCE deletion ABANDONED after its owning service refused to delete it
    /// ([`provisioning::OnRefusal::Abandon`](super::provisioning::OnRefusal)). The resource
    /// is still there, which is what distinguishes this from [`DELETE_COMPLETE`].
    pub const DELETE_SKIPPED: &str = "DELETE_SKIPPED";
    /// The status of a resource an IMPORT brought under the stack's management: it was
    /// neither created nor changed, it is now the stack's ([`resource_import`]).
    pub const IMPORT_COMPLETE: &str = "IMPORT_COMPLETE";
    /// [`CREATE_IN_PROGRESS`]'s counterpart while a resource is being PUT BACK the way it
    /// was: the individual rollback of a resource whose change the service that owns it
    /// refused, which CloudFormation performs whatever becomes of the stack.
    pub const UPDATE_ROLLBACK_IN_PROGRESS: &str = "UPDATE_ROLLBACK_IN_PROGRESS";
    /// The status of a resource that was put back: it carries the configuration in force
    /// before the operation whose change it failed.
    pub const UPDATE_ROLLBACK_COMPLETE: &str = "UPDATE_ROLLBACK_COMPLETE";
    /// The status of a resource the service that owns it would not put back.
    pub const UPDATE_ROLLBACK_FAILED: &str = "UPDATE_ROLLBACK_FAILED";
}

/// The `DeletionPolicy` values the resource attribute takes: "With the `DeletionPolicy`
/// attribute you can preserve, and in some cases, backup a resource when its stack is
/// deleted" — `Delete`, `Retain` and `Snapshot`, `Delete` being the default a declaration
/// that states none gets.
///
/// The same three values `UpdateReplacePolicy` takes for the OLD resource of a REPLACEMENT
/// ([`change_plan::policy_action`]), which is a separate attribute and governs a separate
/// event: this one is read where a resource the stack manages is REMOVED — the stack's own
/// deletion, and a deployment that no longer declares the resource.
pub mod deletion_policy {
    /// "CloudFormation deletes the resource and all its content if applicable during stack
    /// deletion" — what a declaration stating no policy gets.
    pub const DELETE: &str = "Delete";
    /// "CloudFormation keeps the resource without deleting the resource or its contents
    /// when its stack is deleted."
    pub const RETAIN: &str = "Retain";
    /// "For resources that support snapshots, CloudFormation creates a snapshot for the
    /// resource before deleting it."
    pub const SNAPSHOT: &str = "Snapshot";
}

/// `DeletionMode` values (the model's `DeletionMode` enum): "specifies the deletion mode for
/// the stack" (`DeleteStackInput$DeletionMode`).
pub mod deletion_mode {
    /// "Use the standard behavior. Specifying this value is the same as not specifying this
    /// parameter."
    pub const STANDARD: &str = "STANDARD";
    /// "Delete the stack if it's stuck in a DELETE_FAILED state due to resource deletion
    /// failure."
    pub const FORCE_DELETE_STACK: &str = "FORCE_DELETE_STACK";
}

/// What the teardown of a stack deleted under `deletion_mode` does with a resource its
/// owning service refuses to delete.
///
/// `FORCE_DELETE_STACK` is the documented escape hatch for a stack "stuck in a DELETE_FAILED
/// state due to resource deletion failure", so under it such a resource is ABANDONED and the
/// stack's deletion still completes; every other mode — including the absent one, which
/// "is the same as not specifying this parameter" — keeps the refusal as the stack's failure.
pub fn on_refused_resource_deletion(deletion_mode: Option<&str>) -> provisioning::OnRefusal {
    match deletion_mode {
        Some(deletion_mode::FORCE_DELETE_STACK) => provisioning::OnRefusal::Abandon,
        _ => provisioning::OnRefusal::Stop,
    }
}

/// `ResourceSignalStatus` values (the model's `ResourceSignalStatus` enum): "The status of
/// the signal, which is either success or failure" (`SignalResourceInput$Status`).
pub mod resource_signal_status {
    pub const SUCCESS: &str = "SUCCESS";
    pub const FAILURE: &str = "FAILURE";
}

/// `ChangeSetStatus` values this service puts a change set into (all from the model's
/// `ChangeSetStatus` enum).
pub mod change_set_status {
    pub const CREATE_PENDING: &str = "CREATE_PENDING";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    pub const FAILED: &str = "FAILED";
}

/// `ExecutionStatus` values this service puts a change set into (all from the model's
/// `ExecutionStatus` enum).
pub mod execution_status {
    /// The change set is not (yet, or ever) executable: its creation has not completed, or
    /// it failed.
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
    pub const AVAILABLE: &str = "AVAILABLE";
    pub const EXECUTE_IN_PROGRESS: &str = "EXECUTE_IN_PROGRESS";
    pub const EXECUTE_COMPLETE: &str = "EXECUTE_COMPLETE";
    /// The status of a change set whose execution was attempted and did not go through:
    /// the stack policy in force refused one of the actions its plan would have taken, so
    /// the plan was not carried out and the change set is no longer executable.
    pub const EXECUTE_FAILED: &str = "EXECUTE_FAILED";
}

/// `ChangeSetType` values (the model's `ChangeSetType` enum).
pub mod change_set_type {
    pub const CREATE: &str = "CREATE";
    pub const UPDATE: &str = "UPDATE";
    pub const IMPORT: &str = "IMPORT";
}

/// `ChangeAction` values this service computes (from the model's `ChangeAction` enum).
pub mod change_action {
    pub const ADD: &str = "Add";
    pub const MODIFY: &str = "Modify";
    pub const REMOVE: &str = "Remove";
    /// The action on a resource a change set ADOPTS rather than creates
    /// ([`super::resource_import`]).
    pub const IMPORT: &str = "Import";
}

/// `OperationType` values this service records for a stack operation (all from the
/// model's `OperationType` enum), one per call that acts on the stack.
pub mod operation_type {
    pub const CREATE_STACK: &str = "CREATE_STACK";
    pub const UPDATE_STACK: &str = "UPDATE_STACK";
    pub const DELETE_STACK: &str = "DELETE_STACK";
    pub const CREATE_CHANGESET: &str = "CREATE_CHANGESET";
    /// The rollback of a stack whose last operation failed, which `RollbackStack` is the
    /// call that asks for.
    pub const ROLLBACK: &str = "ROLLBACK";
    /// The RESUMPTION of a rollback that could not be carried through, which
    /// `ContinueUpdateRollback` is the call that asks for.
    pub const CONTINUE_ROLLBACK: &str = "CONTINUE_ROLLBACK";
}

/// The statuses a stack operation itself is in (the model's `BeaconStackOperationStatus`
/// enum), as against the `StackStatus` of the stack the operation acts on: the attempt is
/// in flight until one of its transitions settles it, and what settles it says whether it
/// went through.
pub mod operation_status {
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    pub const SUCCEEDED: &str = "SUCCEEDED";
    pub const FAILED: &str = "FAILED";
}

/// The `OperationType` of the stack operation that moves a stack into `in_progress`, or
/// `None` for a status no stack operation of this service opens.
pub fn operation_type_of(in_progress: &str) -> Option<&'static str> {
    match in_progress {
        status::CREATE_IN_PROGRESS => Some(operation_type::CREATE_STACK),
        status::UPDATE_IN_PROGRESS => Some(operation_type::UPDATE_STACK),
        status::DELETE_IN_PROGRESS => Some(operation_type::DELETE_STACK),
        _ => None,
    }
}

/// The `Change$Type` every change this service computes carries: the model's `ChangeType`
/// enum has exactly one value, and a change set here describes resources.
pub const CHANGE_TYPE_RESOURCE: &str = "Resource";

/// The reason CloudFormation records on the event that opens a stack operation.
pub const USER_INITIATED: &str = "User Initiated";

/// The suffix every status of an operation that has STARTED and not concluded carries —
/// the `*_IN_PROGRESS` statuses of [`status`] and of [`resource_status`]. A stack whose
/// operation is still in flight is never LEFT in one of them, which is what a reader of a
/// resource's current status tests against ([`StoredStack::record_rollback_resource_statuses`]).
pub const IN_PROGRESS_SUFFIX: &str = "_IN_PROGRESS";

/// The suffix every status of a transition that did NOT go through carries — the
/// `*_FAILED` statuses of [`status`] and of [`resource_status`]. What a transition in one
/// of them says about the attempt it belongs to is that the attempt failed, which is how
/// an operation's own status ([`operation_status`]) and a FAILED event are both read off
/// it.
pub const FAILED_SUFFIX: &str = "_FAILED";

/// The `StatusReason` CloudFormation puts on a change set that would change nothing —
/// the message it answers for a change set whose computed change list is empty.
pub const NO_CHANGES_REASON: &str = "The submitted information didn't contain changes. \
                                     Submit different information to create a change set.";

/// The most change sets one stack holds.
///
/// CloudFormation meters change sets per stack and models `LimitExceededException` on
/// `CreateChangeSet` for exactly that case; the documented default quota is 20 change sets
/// per stack and is adjustable through Service Quotas, so this is that default as this
/// emulator's own ceiling.
pub const MAX_CHANGE_SETS_PER_STACK: usize = 20;

/// The most change sets this service holds across every stack.
///
/// The per-stack ceiling alone would let one caller hold `MAX_STACKS` × that many stored
/// templates, so the store carries a second, global bound on the same records. Chosen,
/// like every count here, and far above any client workflow.
pub const MAX_CHANGE_SETS: usize = 2_000;

/// The most DISCARDED change sets this service keeps.
///
/// Deleting a change set that has been executed discards its plan and keeps the record of
/// that execution ([`StoredChangeSet::plan_discarded`]), so that record outlives the call
/// that discarded it. The count of them is bounded here — oldest reclaimed first
/// ([`ComAmazonawsCloudformationCloudformationState::reclaim_discarded_change_sets`]) — so a
/// caller repeating create, execute and delete cannot fill the store with records nothing
/// reads any more. Chosen, like every count here, and far above any client workflow.
pub const MAX_DISCARDED_CHANGE_SETS_RETAINED: usize = 200;

/// A resolved stack parameter: the key and the value in force on the stack.
///
/// `Parameter$UsePreviousValue` is a request-time instruction, not stored state — it is
/// resolved against the stored value when an update is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredParameter {
    pub key: String,
    pub value: String,
    /// Whether the TEMPLATE in force declares this parameter `NoEcho` — "whether to mask
    /// the parameter value to prevent it from being displayed in the console, command line
    /// tools, or API" ([`template::DeclaredParameter::no_echo`]).
    ///
    /// Recorded where the parameter set is resolved against a template body
    /// ([`parameters_in_force`], [`record_no_echo`]) rather than re-read on every
    /// describe: a read must not re-parse a caller-sized body under the store lock, and
    /// the declaration that governs the value is the one it was submitted against.
    ///
    /// The VALUE is still stored as submitted — a `Ref` to the parameter has to resolve to
    /// what the stack was deployed with — and the mask is applied where CloudFormation
    /// echoes the parameter back ([`parameter_views`]).
    pub no_echo: bool,
    /// The value Parameter Store holds under the NAME this parameter's value addresses,
    /// for a parameter the template declares with an SSM-typed `Type`
    /// ([`ssm_parameters`]): "the parameter value is the name of a Systems Manager
    /// parameter, and the value that CloudFormation uses is the value of that parameter".
    ///
    /// `None` for every other parameter, which carries its own value — and that is exactly
    /// what `Parameter$ResolvedValue` reports ("Read-only. The value that corresponds to a
    /// Systems Manager parameter key"), so the member is absent for a parameter that
    /// addresses no parameter key.
    ///
    /// Resolved where the deployment runs (`CreateStack`, `UpdateStack`), against the store
    /// as it stood then: [`StoredParameter::value_in_force`] is what every expression the
    /// template builds from the parameter is evaluated against.
    pub resolved_value: Option<String>,
}

impl StoredParameter {
    /// The value the stack's own expressions resolve this parameter to: the value Parameter
    /// Store held for an SSM-typed parameter, and the parameter's own value otherwise.
    ///
    /// The ONE reading of that rule, so a `Ref`, an `Fn::Sub` and a resource property all
    /// carry the same value.
    pub fn value_in_force(&self) -> &str {
        self.resolved_value.as_deref().unwrap_or(&self.value)
    }
}

/// The values a stack's expressions are evaluated against, keyed by `ParameterKey`:
/// each parameter's [`StoredParameter::value_in_force`].
///
/// The one projection of a stored parameter set onto an evaluation scope, so a deploying
/// operation and a later read of the same stack resolve a parameter identically.
pub fn parameter_value_map(parameters: &[StoredParameter]) -> BTreeMap<String, String> {
    parameters
        .iter()
        .map(|parameter| (parameter.key.clone(), parameter.value_in_force().to_string()))
        .collect()
}

/// One resource a stack's template declares, as this emulator records it.
///
/// The physical resource id and the attributes are what CloudFormation's documented
/// return values for that resource type give it (see [`template::provision`]), unless the
/// resource was really CREATED in the service that owns it
/// ([`provisioning`](crate::state::provisioning)) — then they are what that service
/// answered and `materialised` records it. They are what `Ref` and `Fn::GetAtt` resolve to
/// when the stack's outputs are evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResource {
    pub logical_id: String,
    pub resource_type: String,
    pub physical_resource_id: String,
    /// The attributes `Fn::GetAtt` reads off this resource, keyed by attribute name.
    pub attributes: BTreeMap<String, String>,
    /// The content of the `Metadata` resource attribute the template declared for this
    /// resource, as JSON — what `StackResourceDetail$Metadata` reports ("The content of
    /// the `Metadata` attribute declared for the resource"). `None` for a resource whose
    /// declaration carries no `Metadata`, since the member is optional and this emulator
    /// reports what the template declared rather than an invented value.
    ///
    /// Captured where the template is DEPLOYED ([`template::provision`]), not on a read:
    /// a read must not re-parse a caller-sized body under the store lock, and the value
    /// belongs to the template that finished deploying rather than to the one in force.
    pub metadata: Option<String>,
    /// The resource's declared `Properties` with every intrinsic function EVALUATED — the
    /// request the owning service's create API takes
    /// ([`provisioning`](crate::state::provisioning)). `Null` for a resource that declared
    /// none. This is also what `StackEvent$ResourceProperties` reports ("BLOB of the
    /// properties used to create the resource"), via [`StoredResource::properties_blob`].
    pub properties: ::serde_json::Value,
    /// Whether the resource was CREATED in the service that owns it, so the stack's
    /// deletion has to delete it there. `false` for a resource type this emulator has no
    /// provisioner for: nothing was created for it anywhere (see
    /// [`provisioning`](crate::state::provisioning)).
    pub materialised: bool,
    /// The `DeletionPolicy` resource attribute the template declared for this resource, as
    /// written — what CloudFormation does with the physical resource when the stack is
    /// deleted or the resource is removed from it ([`deletion_policy`]).
    ///
    /// `None` for a declaration that carries none, which is the documented default
    /// ([`deletion_policy::DELETE`]). Captured where the template is DEPLOYED
    /// ([`template::provision`]), beside the `Metadata` attribute and for the same reason:
    /// the attribute that governs the resource is the one of the template that deployed it,
    /// and a read must not re-parse a caller-sized body to find it.
    pub deletion_policy: Option<String>,
}

impl StoredResource {
    /// `StackEvent$ResourceProperties`: "BLOB of the properties used to create the
    /// resource" — the properties this resource was provisioned with, as the JSON document
    /// its events report, and `None` for a resource whose template declared none (there is
    /// no blob to report rather than an empty one).
    ///
    /// Serialised by [`template::properties_blob`], which writes the members in KEY order:
    /// `serde_json`'s own map order depends on a feature any crate in the build may enable,
    /// and an event's text must not depend on how the workspace was compiled.
    pub fn properties_blob(&self) -> Option<String> {
        template::properties_blob(&self.properties)
    }
}

/// One resource a stack has DEPLOYED: what the template declared it as, plus the
/// resource-level status the stack operation that last touched it left it in.
///
/// A resource only reaches a status by way of an operation COMPLETING against it, so
/// these two members are filled in by [`StoredStack::settle`] — never by a read — and a
/// resource a completed operation left exactly as it found it keeps the status and the
/// instant it already carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployedResource {
    pub resource: StoredResource,
    /// The model's `ResourceStatus`, as `StackResource$ResourceStatus` reports it
    /// ("Current status of the resource").
    pub status: String,
    /// `StackResource$Timestamp`: "Time the status was updated" — the instant the stack
    /// operation that left this resource in `status` completed.
    pub status_timestamp: SystemTime,
    /// `StackResource$ResourceStatusReason`: "Success/failure message associated with the
    /// resource."
    ///
    /// `None` for a resource whose status speaks for itself, which a completed create or
    /// update is. A resource whose operation has been STARTED and not finished carries the
    /// message CloudFormation reports for one — see [`initiated_reason`] — since that is
    /// what says the work on it is under way rather than done.
    pub status_reason: Option<String>,
    /// `StackResourceDriftInformation$StackResourceDriftStatus`: this resource's own drift
    /// result, as the last drift detection that COMPLETED over it left it
    /// ([`drift`]). [`resource_drift_status::NOT_CHECKED`] until one has.
    ///
    /// A deployment that re-created or changed the resource leaves a new record, so the
    /// result belongs to the resource as it is now; one a deployment left untouched keeps
    /// the result it already carried.
    pub drift_status: String,
    /// `StackResourceDriftInformation$LastCheckTimestamp`: "when CloudFormation last
    /// checked if the resource had drifted from its expected configuration".
    pub drift_checked_at: Option<SystemTime>,
    /// The COMPARISON the last drift detection carried out over this resource: the expected
    /// and actual configurations it read and the differences it found
    /// ([`drift::ResourceDriftDetail`]), which is what `DescribeStackResourceDrifts` reports
    /// per resource.
    ///
    /// `None` for a resource no check was carried out for — one nothing has checked yet,
    /// and one whose type drift detection is not supported for here, which is exactly the
    /// set `DescribeStackResourceDrifts` leaves out ("resources that haven't yet been
    /// checked for drift aren't included"; "resources that don't currently support drift
    /// detection aren't checked, and so not included").
    pub drift_detail: Option<drift::ResourceDriftDetail>,
}

impl DeployedResource {
    /// A resource record as a stack operation installs it: with the status that operation
    /// left it in, and no drift result, since nothing has checked it for drift.
    pub fn new(resource: StoredResource, status: &str, at: SystemTime) -> Self {
        Self {
            resource,
            status: status.to_string(),
            status_timestamp: at,
            status_reason: None,
            drift_status: resource_drift_status::NOT_CHECKED.to_string(),
            drift_checked_at: None,
            drift_detail: None,
        }
    }
}

/// One evaluated stack output: an `Outputs` entry of the stack's template with its
/// `Value` resolved against the stack's parameters and resources.
///
/// `value` is `None` for a declared output whose value this emulator cannot evaluate —
/// the output exists because the template declares it, and `Output$OutputValue` is
/// optional, so an un-evaluable output is reported without a value rather than with an
/// invented one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOutput {
    pub key: String,
    pub value: Option<String>,
    pub description: Option<String>,
    pub export_name: Option<String>,
}

/// One stack operation — "a discrete change attempt on the stack": the id the call that
/// opened it handed back, the `OperationType` it carries, the span it ran for and how it
/// ended.
///
/// `Stack$LastOperations` reports the id and the type of the operation that last acted on
/// the stack ([`StoredStack::last_operation`]); the span and the status are what an
/// operation's own events are reported against, since every event one operation causes
/// carries its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOperation {
    pub operation_id: String,
    pub operation_type: String,
    /// The change set this operation belongs to, for an operation a change set caused;
    /// `None` for one a call made directly against the stack.
    pub change_set_id: Option<String>,
    /// The instant the call that opened the operation was served.
    pub started_at: SystemTime,
    /// The instant its last transition settled, or `None` while it is still in flight.
    pub ended_at: Option<SystemTime>,
    /// [`operation_status`]: how the attempt stands.
    pub status: String,
}

impl StoredOperation {
    /// The operation a call opens: in flight, with nothing ended yet.
    fn opened(
        operation_type: &str,
        operation_id: &str,
        change_set_id: Option<String>,
        at: SystemTime,
    ) -> Self {
        Self {
            operation_id: operation_id.to_string(),
            operation_type: operation_type.to_string(),
            change_set_id,
            started_at: at,
            ended_at: None,
            status: operation_status::IN_PROGRESS.to_string(),
        }
    }
}

/// A stack tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTag {
    pub key: String,
    pub value: String,
}

/// `DeploymentConfig` as supplied on the call that last set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDeploymentConfig {
    pub mode: Option<String>,
    pub disable_rollback: Option<bool>,
}

/// One `RollbackTrigger` of a stack's rollback configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRollbackTrigger {
    pub arn: String,
    pub trigger_type: String,
}

/// `RollbackConfiguration` as supplied on the call that last set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRollbackConfiguration {
    pub triggers: Vec<StoredRollbackTrigger>,
    pub monitoring_time_in_minutes: Option<i32>,
}

/// The PRE-DEPLOYMENT validation failure one event reports, for an event that is a
/// VALIDATION result rather than a resource transition: "Validation errors - Failures from
/// CloudFormation Early Validations", which is the class `DescribeEvents` reports under
/// `EventType` `VALIDATION_ERROR` and the detail a failed change set's `StatusReason` sends a
/// client to that call for.
///
/// Such an event is about a resource NOTHING did anything to — the plan was not executed —
/// so it carries no resource status of its own ([`StoredEvent::resource_status`] is empty on
/// one) and is not part of the resource-transition stream `DescribeStackEvents` reports.
#[derive(Debug, Clone)]
pub struct StoredValidationFailure {
    /// `OperationEvent$ValidationName`: "the name of the validation that was performed".
    pub name: String,
    /// `OperationEvent$ValidationStatus`, from the model's `ValidationStatus` enum.
    pub status: String,
    /// `OperationEvent$ValidationStatusReason`: "additional information about the validation
    /// status" — what about the resource failed the validation.
    pub reason: String,
    /// `OperationEvent$ValidationPath`: "the path within the resource where the validation
    /// was applied" — the element of the submitted template the validation ran on, named by
    /// the validation that ran ([`early_validation::resource_path`],
    /// [`property_validation::properties_path`]).
    pub path: String,
}

/// One stack event, appended (never overwritten) by the operation that caused it.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event_id: String,
    pub stack_id: String,
    pub stack_name: String,
    pub operation_id: Option<String>,
    pub logical_resource_id: String,
    pub physical_resource_id: Option<String>,
    pub resource_type: String,
    pub timestamp: SystemTime,
    pub resource_status: String,
    pub resource_status_reason: Option<String>,
    /// `StackEvent$ResourceProperties`: "BLOB of the properties used to create the
    /// resource" — carried by a resource-level event, absent on the stack's own.
    pub resource_properties: Option<String>,
    pub client_request_token: Option<String>,
    /// The validation failure this event reports, for an event that is one
    /// ([`StoredValidationFailure`]); `None` for every event that reports a resource or
    /// stack transition.
    pub validation: Option<StoredValidationFailure>,
    /// Whether the status this event reports was reached because a PRE-DEPLOYMENT VALIDATION
    /// failed the operation — the stack's own failure event for a stage that ran before any
    /// resource was asked for, which is the only event that can report it.
    pub failed_validation: bool,
    /// Whether this event still OWES a notification, and to which topic list: the digest of
    /// the stack's `NotificationARNs` as they stood when the event was recorded
    /// ([`stack_notifications::topics_digest`]), taken off the event as the notification is
    /// claimed for publishing. `None` is "nothing owed" — the stack carried no topics then,
    /// or the message has been claimed already — so no event is ever published twice.
    ///
    /// A DIGEST and not the list: the stack holds the one copy of its own topics, and an ARN
    /// is caller text whose length the model does not bound, so what the event log retains
    /// per event is eight bytes.
    pub owed_notification: Option<u64>,
}

/// The `DetailedStatus` of an event that reports a PRE-DEPLOYMENT validation failure, from
/// the model's `DetailedStatus` enum: "template validation failed because of invalid
/// properties in the template".
pub const DETAILED_STATUS_VALIDATION_FAILED: &str = "VALIDATION_FAILED";

/// The detailed status one recorded event reports, or `None` for an event that reports none —
/// `StackEvent$DetailedStatus` and `OperationEvent$DetailedStatus`, "an optional field that
/// contains information about the detailed status of the stack event".
///
/// Stated ONCE for every reader of an event: the `DescribeStackEvents` and `DescribeEvents`
/// views and the Amazon SNS notification a stack's `NotificationARNs` receive
/// ([`stack_notifications::body_of`]) report the same event's detailed status rather than
/// deciding it three times.
pub fn detailed_status_of(event: &StoredEvent) -> Option<&'static str> {
    (event.validation.is_some() || event.failed_validation)
        .then_some(DETAILED_STATUS_VALIDATION_FAILED)
}

/// The bytes of one event that a CALLER sized: its `ResourceProperties` document. Every
/// other member is an identifier, a status or a timestamp this service minted, so this is
/// what [`StoredStack::push_event`] meters the log in.
fn event_properties_bytes(event: &StoredEvent) -> usize {
    event
        .resource_properties
        .as_ref()
        .map(String::len)
        .unwrap_or_default()
}

/// One resource signal `SignalResource` recorded against a stack.
///
/// Kept per `UniqueId`, because that is what identifies a signal: "If you send multiple
/// signals to a single resource (such as signaling a wait condition), each signal requires
/// a different unique ID" (`SignalResourceInput$UniqueId`) — so the same unique id sent
/// again is the SAME signal rather than a second one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResourceSignal {
    /// The logical id the signal names, whether or not the stack's template declares a
    /// resource under it: CloudFormation takes the signal either way.
    pub logical_resource_id: String,
    pub unique_id: String,
    /// `SUCCESS` or `FAILURE` ([`resource_signal_status`]).
    pub status: String,
    pub received_at: SystemTime,
}

/// The terminal status a stack operation is headed for, and the instant it is due.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub status: String,
    /// The `StackStatusReason` that status carries, for an operation that is headed for a
    /// FAILED one (see [`StoredStack::fail_pending_transition`], and
    /// [`StoredStack::fail_transition`] for a failure discovered while the operation is in
    /// flight): a failure has to say what failed, and a success states no reason.
    pub status_reason: Option<String>,
    /// The instant the call that opened this operation was served — what the operation's
    /// own walk over the stack's resources is measured from ([`timeline`]).
    pub started_at: SystemTime,
    pub due_at: SystemTime,
    pub operation_id: Option<String>,
    pub client_request_token: Option<String>,
}

/// The resources and outputs an in-flight stack operation will leave the stack with,
/// once the clock makes its terminal status due.
///
/// CloudFormation reports a stack's resources and outputs as of the template that
/// finished deploying, so a create or update computes them when it starts and they take
/// effect when it completes.
#[derive(Debug, Clone, Default)]
pub struct PendingDeployment {
    pub resources: Vec<StoredResource>,
    pub outputs: Vec<StoredOutput>,
    /// The exported output values this deployment resolved its template's
    /// `Fn::ImportValue` references against, keyed by export name
    /// ([`template::imported_export_names`]). This is the stack's entry in the cross-stack
    /// reference index `ListImports` reports, so it takes effect when the deployment
    /// completes, exactly as the resources and outputs beside it do.
    pub imports: BTreeMap<String, String>,
    /// The export names this deployment's template imports that NOTHING published in the
    /// account and Region it runs in ([`template::imported_export_names`] minus the names
    /// `imports` above carries), in the order the template names them.
    ///
    /// "The intrinsic function Fn::ImportValue returns the value of an output exported by
    /// another stack": a name no live stack of that Region exports has no value to return,
    /// so the deployment cannot be carried out at all — which is what
    /// [`StoredStack::unresolved_cross_stack_failure`] words for the operation driving it.
    pub unresolved_imports: Vec<String>,
    /// The other stacks' output values this deployment's template READ with an
    /// `Fn::GetStackOutput`, keyed by the reference that read one
    /// ([`stack_outputs::Values`]). Resolved where the template is deployed and KEPT by the
    /// stack, so the reading stack goes on reporting the value as of its own deployment —
    /// which is why it travels with the deployment the way the imports above do.
    pub stack_outputs: stack_outputs::Values,
    /// The output references this deployment's template reads that NO live stack of its
    /// account and Region carries, in the order the template reads them.
    ///
    /// Such a reference has no value to return, so the deployment cannot be carried out at
    /// all — which is what [`StoredStack::unresolved_cross_stack_failure`] words for the
    /// operation driving it, as it does for an unresolved import.
    pub unresolved_stack_outputs: Vec<stack_outputs::Reference>,
    /// The resource-signal waits the template being deployed declares
    /// ([`wait_conditions::declared`]), each with the resources whose creation waits on
    /// it. Computed where the template is READ, outside the store lock;
    /// [`StoredStack::begin_transition`] is what opens a barrier for each one the
    /// operation has to wait at.
    pub wait_conditions: Vec<wait_conditions::DeclaredWaitCondition>,
    /// The logical ids this operation has ACTED ON: every resource it planned a transition
    /// for ([`StoredStack::schedule_resource_events`]).
    ///
    /// CloudFormation reports a resource once the operation's work on it has STARTED, so
    /// this is the set whose records survive an operation that disposed of the deployment
    /// — a declaration the walk never reached has nothing to report.
    pub attempted: BTreeSet<String>,
    /// The direct dependency relation of the template being deployed
    /// ([`template::direct_dependencies`]): what each resource it declares waits for.
    /// Read where the template is READ, outside the store lock, and installed as the
    /// stack's own when the deployment completes — it is what orders the deployment's
    /// walk over the resources in time ([`timeline`]).
    pub dependencies: BTreeMap<String, BTreeSet<String>>,
}

/// What the ROLLBACK of the UPDATE in flight would have to put back in the services that
/// own the stack's resources, held against the chance that the update fails with no call of
/// its own to compute it.
///
/// An update held at a wait condition has already asked the owning services for everything
/// it wanted ([`provisioning::reconcile`], driven by the handler), and the thing that can
/// still end it is its wait running out — which falls due on the CLOCK. CloudFormation
/// "rolls back the stack to the previous stack configuration" when an update fails,
/// whichever way it failed, so the work that rollback owes is computed by the handler that
/// knows what the update changed, carried here, and performed by the next operation that
/// can reach those services ([`settle_due_rollbacks`]).
///
/// One record per stack with an update in flight, carrying one entry per resource that
/// update declared ([`MAX_CONCURRENT_RESOURCES`] across the store), retired with the
/// operation that holds it.
#[derive(Debug, Clone)]
pub struct OwedRollback {
    /// The resources in force before the update — the configuration to put back.
    pub previous: Vec<StoredResource>,
    /// What the update left in the owning services, which the revert runs against.
    pub applied: Vec<StoredResource>,
}

/// The stack's OWN record of what is deployed on it: the configuration members an update
/// replaces when it is applied, as distinct from the resources the owning services hold.
///
/// CloudFormation "rolls back the stack to the previous stack configuration" when an update
/// fails, and the configuration is the whole revision the last operation that COMPLETED
/// installed — not only its resources: the template the stack carries (with the processed
/// stage computed over it) and every member a client reads back as the stack's
/// (`Stack$Parameters`, `Stack$Tags`, `Stack$Capabilities`, `Stack$NotificationARNs`,
/// `Stack$RoleARN`, `Stack$RollbackConfiguration`, `Stack$DeploymentConfig`). The values a
/// REJECTED update submitted are discarded with the rest of that revision, so the
/// configuration in force before the update is snapshotted by the handler that applies one
/// ([`StoredStack::hold_previous_configuration`]) and put back when the rollback opens
/// ([`StoredStack::begin_update_rollback`]).
///
/// Not part of it: the per-operation instructions a caller gives
/// (`DisableRollback`, `RetainExceptOnCreate`) and the stack policy a request installs,
/// which say what to do with THIS operation rather than what the stack deploys, and the
/// timestamps and operation records, which report that the update was attempted.
#[derive(Debug, Clone)]
pub struct StackConfiguration {
    pub template_body: Option<String>,
    pub template_description: Option<String>,
    pub processed_template: template::Processed,
    /// The body that stage was computed over: the stored body with any `AWS::Include`
    /// snippet spliced in ([`StoredStack::included_template_body`]).
    pub processed_template_body: Option<String>,
    pub parameters: Vec<StoredParameter>,
    pub tags: Vec<StoredTag>,
    pub notification_arns: Vec<String>,
    pub capabilities: Vec<String>,
    pub role_arn: Option<String>,
    pub rollback_configuration: Option<StoredRollbackConfiguration>,
    pub deployment_config: Option<StoredDeploymentConfig>,
    pub resource_types: Vec<String>,
}

/// What a ROLLBACK a client ASKED for ([`StoredStack::begin_requested_rollback`]) has to do
/// in the services that own the stack's resources.
///
/// `RollbackStack` "rolls back the specified stack to the last known stable state", and
/// "will delete a stack if it doesn't contain a last known stable state": which of the two
/// this is depends on whether the stack HAS such a state, and the work itself is the
/// caller's to drive with the store unlocked — exactly as a failed operation's own unwind is.
#[derive(Debug, Clone)]
pub enum RequestedRollback {
    /// The stack holds no last known stable state, so there is nothing to go back TO: the
    /// resources the operation that failed had provisioned are removed again
    /// ([`provisioning::unwind`]) and the stack settles in [`status::ROLLBACK_COMPLETE`].
    Discard(Vec<StoredResource>),
    /// The stack holds one: the configuration in force before the failed operation is
    /// declared again in those services, against what that operation left there
    /// ([`provisioning::reconcile`]), and the stack settles in
    /// [`status::UPDATE_ROLLBACK_COMPLETE`].
    Revert(OwedRollback),
}

/// One client request token an operation on a stack was started under, with the operation
/// that claimed it.
///
/// `ClientRequestToken` is "a unique identifier for this request", and the modelled
/// `TokenAlreadyExistsException` ("A client request token already exists") is what a second
/// operation presenting one already claimed is refused with — so a stack has to remember
/// which of its operations claimed which token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRequestToken {
    pub token: String,
    /// The `*_IN_PROGRESS` status of the operation that claimed the token, which is what
    /// says WHICH operation it names: a repeat of that same operation under it is the retry
    /// the member is documented for, and any other operation presenting it is refused. Set
    /// by the FIRST operation to claim the token and never rewritten.
    pub opened: String,
    /// The id of the operation that claimed it, as the call that started that operation
    /// answered: a retry under this token re-answers that same operation rather than
    /// minting a second one.
    pub operation_id: Option<String>,
}

/// One stack, as this emulator persists it: the record, the status transitions it lives
/// through, and the helpers those transitions use.
pub mod stack;
mod failure_summary;
pub use failure_summary::*;

/// Which stack a stack is NESTED IN, derived from the records the store already holds
/// ([`stack_nesting`]).
mod stack_nesting;
/// Resolving the service role a stack operation acts as, against the account's own IAM
/// records ([`service_role_trust`]).
mod service_role_trust;
mod pagination;
pub use pagination::*;
pub use service_role_trust::*;

/// The CONFIGURATION one stack operation runs under: its `DeploymentConfig` and its
/// `RollbackConfiguration` ([`operation_configuration`]).
mod operation_configuration;
/// The change-set RECORD and the decisions that read its statuses ([`change_set`]).
mod change_set;
/// The identifiers this service mints — a stack id, an operation id, a change set id and the
/// rest — each in the form CloudFormation gives that kind of id ([`identifiers`]).
mod identifiers;
pub use identifiers::*;
pub use change_set::*;
pub use operation_configuration::*;

pub use stack_nesting::Nesting;

pub use stack::StoredStack;

/// The `Name` a resource's own declaration asks for, read by `provisioning` as well as by
/// the deployment that computes it ([`stack::declared_name`]).
pub(crate) use stack::declared_name;

/// The status an operation that cannot be carried out leaves the stack in, read by
/// `wait_conditions` as well as by the transitions themselves ([`stack::failed_status_of`]).
pub(crate) use stack::failed_status_of;

/// The change set as this store carries it: the resource-level change list and the
/// property-level detail behind each entry, which [`change_plan::computed_changes`]
/// computes once where the change set is created.
pub use change_plan::{StackRevision, StoredChangeDetail, StoredResourceChange};

/// Whether two parameter sets state the same values: `Parameters` is a SET keyed by
/// `ParameterKey`, so two requests that list the same pairs in two orders submit the same
/// parameters.
fn same_parameters(stored: &[StoredParameter], submitted: &[StoredParameter]) -> bool {
    fn by_key(parameters: &[StoredParameter]) -> BTreeMap<&str, &str> {
        parameters
            .iter()
            .map(|parameter| (parameter.key.as_str(), parameter.value.as_str()))
            .collect()
    }
    by_key(stored) == by_key(submitted)
}

/// Whether two tag sets state the same tags, keyed by `Key` for the same reason
/// [`same_parameters`] keys by `ParameterKey`.
fn same_tags(stored: &[StoredTag], submitted: &[StoredTag]) -> bool {
    fn by_key(tags: &[StoredTag]) -> BTreeMap<&str, &str> {
        tags.iter()
            .map(|tag| (tag.key.as_str(), tag.value.as_str()))
            .collect()
    }
    by_key(stored) == by_key(submitted)
}

/// The attributes one `CreateChangeSet` request submits, resolved the way they would be
/// STORED: what [`StoredChangeSet::mismatching_attribute`] compares a re-created name
/// against.
#[derive(Debug, Clone, Copy)]
pub struct SubmittedChangeSet<'a> {
    pub change_set_type: &'a str,
    pub description: Option<&'a str>,
    /// The template the change set would put on the stack, which for a request reusing the
    /// stack's template is that template.
    pub template_body: &'a str,
    /// The parameter values the change set would apply, already resolved against the ones
    /// the stack carries (`UsePreviousValue`).
    pub parameters: &'a [StoredParameter],
    pub capabilities: &'a [String],
    pub resource_types: &'a [String],
    pub role_arn: Option<&'a str>,
    pub rollback_configuration: Option<&'a StoredRollbackConfiguration>,
    pub notification_arns: &'a [String],
    pub tags: &'a [StoredTag],
    pub on_stack_failure: Option<&'a str>,
    pub include_nested_stacks: bool,
    pub import_existing_resources: bool,
    pub deployment_mode: Option<&'a str>,
    /// `ClientToken`: "a unique identifier for this CreateChangeSet request", which is the
    /// attribute that says WHICH request a change set under this name was created by.
    pub client_token: Option<&'a str>,
}

/// One drift-detection operation on one stack: what `DetectStackDrift` started and
/// `DescribeStackDriftDetectionStatus` reads back.
///
/// The detection's own outcome is asynchronous ("DetectStackDrift can take up to several
/// minutes"), so a record is answered `DETECTION_IN_PROGRESS` and carries the outcome the
/// clock owes in [`Self::pending`]; [`ComAmazonawsCloudformationCloudformationState::settle`]
/// is what materialises it, onto this record and onto the stack and resources it describes.
#[derive(Debug, Clone)]
pub struct StoredDriftDetection {
    /// `StackDriftDetectionId`, the id the call handed back.
    pub detection_id: String,
    /// The stack the detection ran against, by stack id: "the ID of the stack".
    pub stack_id: String,
    /// `Timestamp`: "time at which the stack drift detection operation was initiated".
    pub initiated_at: SystemTime,
    /// [`drift_detection_status`].
    pub detection_status: String,
    pub detection_status_reason: Option<String>,
    /// The stack-level `StackDriftStatus` the detection found, once it has completed.
    /// `None` while it is in progress: "this value will be NULL for any stack drift
    /// detection operation that has not yet concluded".
    pub stack_drift_status: Option<String>,
    /// `DriftedStackResourceCount`: "this is NULL until the drift detection operation
    /// reaches a status of DETECTION_COMPLETE".
    pub drifted_resource_count: Option<i32>,
    /// The outcome computed for this detection, and the instant reporting it is due.
    pub pending: Option<PendingDriftOutcome>,
}

/// What a drift detection found, held until the clock makes reporting the detection's own
/// conclusion due.
#[derive(Debug, Clone)]
pub struct PendingDriftOutcome {
    /// [`drift_detection_status`]: `DETECTION_COMPLETE`, or `DETECTION_FAILED` where an
    /// owning service's refusal left a resource unchecked.
    pub detection_status: String,
    pub detection_status_reason: Option<String>,
    pub stack_drift_status: String,
    pub drifted_resource_count: i32,
    pub due_at: SystemTime,
}

/// Per-service state for com-amazonaws-cloudformation-cloudformation. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudformationCloudformationState {
    /// Every stack, in creation order, so `DescribeStacks` and `ListStacks` are
    /// deterministic. Includes deleted stacks (see this module's docs).
    pub stacks: Vec<StoredStack>,
    /// Every change set of every stack, in creation order, so `DescribeChangeSet` reads
    /// back exactly what `CreateChangeSet` wrote. A change set lives as long as its stack
    /// does: CloudFormation deletes a stack's change sets with the stack, which is what
    /// [`Self::settle`] enforces here.
    pub change_sets: Vec<StoredChangeSet>,
    /// This account and Region's extension registry, in the order its records entered it,
    /// so `DescribeType` reads back exactly what a registry write wrote (see
    /// [`registry`]). A registry record has nothing to do with a stack's lifecycle, so it
    /// is never reclaimed with one; the one transition the clock owes it is the conclusion
    /// of the contract tests `TestType` started, which [`Self::settle`] installs.
    ///
    /// A caller adds a record under a name of its own choosing — an activation's
    /// `TypeNameAlias`, a publication's type name — so the set is bounded at
    /// [`registry::MAX_EXTENSIONS`] records, oldest reclaimed first, at every write that
    /// adds one ([`Self::publish_extension_version`] and `ActivateType`), with
    /// [`registry::MAX_PUBLISHED_VERSIONS`] versions and
    /// [`registry::MAX_TYPE_CONFIGURATIONS`] configurations inside one record.
    pub extensions: Vec<registry::StoredExtension>,
    /// This account's extension-publisher registration, once `RegisterPublisher` has
    /// written one (see [`publisher`]). The publisher id "applies to your account in all
    /// Amazon Web Services Regions", so there is at most ONE such record and it is not
    /// namespaced by Region; it belongs to no stack, so it is neither settled nor
    /// reclaimed with one.
    pub publisher: Option<publisher::StoredPublisher>,
    /// Whether this account is enrolled as an Amazon Web Services Marketplace seller, which
    /// is what `RegisterPublisher` requires of its caller
    /// ([`publisher::MarketplaceSellerEnrollment`]).
    ///
    /// Not a record any operation writes: the enrollment is made outside this API, so it is a
    /// property of the emulated account — `false` for an account nobody enrolled, which is
    /// the default, and [`publisher::MARKETPLACE_SELLER_ENV`] is how an operator says this
    /// emulated account is one of the accounts that already enrolled.
    pub marketplace_seller: publisher::MarketplaceSellerEnrollment,
    /// Every drift-detection operation this service has run, in the order they were
    /// started, so `DescribeStackDriftDetectionStatus` reads back the result of the id
    /// `DetectStackDrift` handed out. Bounded at [`MAX_DRIFT_DETECTIONS`], oldest
    /// reclaimed first.
    pub drift_detections: Vec<StoredDriftDetection>,
    /// Every Hook invocation result this account and Region has recorded, in invocation
    /// order, so `ListHookResults` pages over them deterministically (see
    /// [`hook_results`]). An invocation result belongs to the account rather than to a
    /// stack, so it is neither settled nor reclaimed with one; it is bounded at
    /// [`hook_results::MAX_HOOK_RESULTS`], oldest reclaimed first.
    pub hook_results: Vec<hook_results::StoredHookResult>,
    /// Every generated template, in creation order, so `DescribeGeneratedTemplate` reads
    /// back exactly what `CreateGeneratedTemplate` wrote (see [`generated_template`]). A
    /// generated template is a record of its own rather than a stack's, so it is neither
    /// settled on the stack clock nor reclaimed with a stack; its own generation clock is
    /// what [`Self::settle`] advances, and
    /// [`generated_template::MAX_GENERATED_TEMPLATES`] is what bounds the list.
    pub generated_templates: Vec<generated_template::StoredGeneratedTemplate>,
    /// Every stack set this account holds, in creation order, so a read of one stack set's
    /// instances is deterministic (see [`stack_sets`]). A stack set is a record of its own
    /// rather than a stack's, so it is neither settled on the stack clock nor reclaimed
    /// with a stack; [`stack_sets::MAX_STACK_SETS`] is what bounds the list.
    pub stack_sets: Vec<stack_sets::StoredStackSet>,
    /// Every resource scan this account and Region has started, in the order the scans were
    /// started, so `ListResourceScans` pages over them deterministically (see
    /// [`resource_scans`]). A scan belongs to the account rather than to a stack, so it is
    /// never reclaimed with one; the one transition the clock owes it is its own completion,
    /// which [`Self::settle`] materialises, and it is bounded at
    /// [`resource_scans::MAX_RESOURCE_SCANS`], oldest reclaimed first.
    pub resource_scans: Vec<resource_scans::StoredResourceScan>,
    /// Every resource-handler invocation this account and Region has a progress record for,
    /// in invocation order, so `RecordHandlerProgress` judges a report against the status
    /// the invocation its bearer token names holds (see [`handler_progress`]). An
    /// invocation belongs to no stack, so it is neither settled on the stack clock nor
    /// reclaimed with one; it is bounded at [`handler_progress::MAX_HANDLER_INVOCATIONS`],
    /// oldest reclaimed first.
    pub handler_invocations: Vec<handler_progress::StoredHandlerInvocation>,
    /// Every extension REGISTRATION REQUEST this account and Region holds, in the order the
    /// requests were started, so `ListTypeRegistrations` pages over them deterministically
    /// (see [`registry::StoredTypeRegistration`]). A registration belongs to the registry
    /// rather than to a stack, so it is never reclaimed with one; the one transition the
    /// clock owes it is its own conclusion, which [`Self::settle`] materialises, and it is
    /// bounded at [`registry::MAX_TYPE_REGISTRATIONS`], oldest reclaimed first.
    pub type_registrations: Vec<registry::StoredTypeRegistration>,
    /// Every stack refactor this account and Region holds, in creation order, so
    /// `DescribeStackRefactor` reads back exactly what `CreateStackRefactor` wrote (see
    /// [`stack_refactors`]). A refactor is a record of its own rather than a stack's, so it
    /// is neither settled on the stack clock nor reclaimed with a stack; the one transition
    /// the clock owes it is its own creation concluding, which [`Self::settle`] materialises,
    /// and it is bounded at [`stack_refactors::MAX_STACK_REFACTORS`], oldest reclaimed first.
    pub stack_refactors: Vec<stack_refactors::StoredStackRefactor>,
    /// Whether trusted access between StackSets and Organizations is activated, which
    /// `ActivateOrganizationsAccess` / `DeactivateOrganizationsAccess` write and
    /// `DescribeOrganizationsAccess` reads back (see [`organizations_access`]).
    ///
    /// A property of the ORGANIZATION rather than of a Region's stacks, so there is one of
    /// these and it is not namespaced by Region; it belongs to no stack, so it is neither
    /// settled nor reclaimed with one.
    pub organizations_access: organizations_access::TrustedAccess,
    /// How this emulator is reached for the work a LANDED provider answer owes, remembered
    /// from the last stack operation that provisioned anything (see [`provider_driver`]).
    ///
    /// Not a record of anything a client created: a cloneable handle to this emulator's own
    /// in-process transport and this service's identity, which is what lets the plane that
    /// receives a provider's answer publish the request that answer owes. `None` until a
    /// stack operation has provisioned something, which is before any answer can arrive.
    pub provisioning_reach: Option<provider_driver::ProvisioningReach>,
}

impl ComAmazonawsCloudformationCloudformationState {
    /// Materialise every status transition the clock has made due, then enforce the
    /// retention bound on completed deletions. Called first by every operation, so no
    /// caller can observe a stack that should have settled already, or one the retention
    /// policy had already reclaimed.
    pub fn settle(&mut self, now: SystemTime) {
        for stack in &mut self.stacks {
            stack.settle(now);
        }
        // A change set's creation is a stack operation of its own, and the change set is
        // what carries the status that creation reached: an operation whose change set has
        // just settled concluded at the instant that settlement was due.
        let mut settled_change_sets: Vec<(String, String, String, SystemTime)> = Vec::new();
        // What each stack is doing, now that the stacks above have settled: a change set's
        // EXECUTION is the stack operation it started, so it is neither concluded while that
        // operation runs nor concluded in a status the operation's own outcome contradicts
        // ([`StoredChangeSet::settle`]). Disjoint fields: the map borrows `stacks`, the loop
        // mutates `change_sets`.
        let progress: ::std::collections::HashMap<&str, StackProgress> = self
            .stacks
            .iter()
            .map(|stack| {
                (
                    stack.stack_id.as_str(),
                    StackProgress {
                        operation_in_flight: stack.pending.is_some(),
                        carried_through: operation_carried_through(&stack.status),
                    },
                )
            })
            .collect();
        for change_set in &mut self.change_sets {
            // A change set whose stack this store no longer holds has no operation to wait
            // for: whatever its own clock owed is applied.
            let stack = progress
                .get(change_set.stack_id.as_str())
                .copied()
                .unwrap_or(StackProgress {
                    operation_in_flight: false,
                    carried_through: true,
                });
            if let Some(at) = change_set.settle(now, stack) {
                settled_change_sets.push((
                    change_set.stack_id.clone(),
                    change_set.change_set_id.clone(),
                    change_set.status.clone(),
                    at,
                ));
            }
        }
        for (stack_id, change_set_id, settled_status, at) in settled_change_sets {
            if let Some(stack) = self
                .stacks
                .iter_mut()
                .find(|stack| stack.stack_id == stack_id)
            {
                stack.conclude_change_set_operation(&change_set_id, &settled_status, at);
            }
        }
        self.settle_drift_detections(now);
        for template in &mut self.generated_templates {
            template.settle(now);
        }
        for extension in &mut self.extensions {
            extension.settle(now);
        }
        // A registration request is the registry's other asynchronous operation: the call
        // that starts one is answered with a token while the registration is still running,
        // so the conclusion the clock has made due is installed here rather than invented by
        // the read that reports it.
        for registration in &mut self.type_registrations {
            registration.settle(now);
        }
        for scan in &mut self.resource_scans {
            scan.settle(now);
        }
        // A refactor's own creation is asynchronous too: the call is answered with an id
        // while CloudFormation is still working out which stacks and resources the refactor
        // affects, so the conclusion the clock has made due is installed here rather than
        // invented by the read that reports it.
        for refactor in &mut self.stack_refactors {
            refactor.settle(now);
        }
        for stack_set in &mut self.stack_sets {
            stack_set.settle(now);
        }
        self.reclaim_retired_stacks();
        self.reclaim_orphaned_change_sets();
    }

    /// The rollbacks the stacks of this store owe the services that own their resources
    /// right now, taken for the caller that is about to perform them
    /// ([`settle_due_rollbacks`]).
    ///
    /// A rollback lands here when the operation it belongs to failed ON THE CLOCK — a wait
    /// condition whose wait ran out — because the store cannot call another service while
    /// it holds its own lock. Taking it is what lets the caller make those calls with the
    /// lock released, and the stack's own terminal status stays held back until the caller
    /// has recorded what they answered.
    pub fn take_due_rollbacks(&mut self) -> Vec<DueRollback> {
        self.stacks
            .iter_mut()
            .filter_map(|stack| {
                let rollback = stack.take_owed_rollback()?;
                Some(DueRollback {
                    region: stack.region.clone(),
                    stack_id: stack.stack_id.clone(),
                    provenance: stack.provenance(),
                    // The role the rollback's own calls are made under, which is the one in
                    // force on the stack ([`service_role`]).
                    role_arn: stack.role_arn.clone(),
                    // What this rollback does with the `DeletionPolicy` of a resource the
                    // failed operation had created ([`provisioning::Policies`]).
                    policies: stack.rolled_back_resource_policies(),
                    rollback,
                })
            })
            .collect()
    }

    /// The generated template `name_or_id` addresses in the request's Region, if this store
    /// holds it: `GeneratedTemplateName` is "the name or Amazon Resource Name (ARN) of a
    /// generated template", and nothing outside that Region is addressable.
    pub fn resolve_generated_template(
        &self,
        region: &str,
        name_or_id: &str,
    ) -> Option<&generated_template::StoredGeneratedTemplate> {
        self.generated_templates
            .iter()
            .find(|template| template.is_addressed_by(region, name_or_id))
    }

    /// [`Self::resolve_generated_template`], for a caller that mutates the record it
    /// resolved.
    pub fn resolve_generated_template_mut(
        &mut self,
        region: &str,
        name_or_id: &str,
    ) -> Option<&mut generated_template::StoredGeneratedTemplate> {
        self.generated_templates
            .iter_mut()
            .find(|template| template.is_addressed_by(region, name_or_id))
    }

    /// The generated templates of one Region, in creation order: the set a name has to be
    /// unique within and the one its quotas are metered over.
    pub fn generated_templates_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a generated_template::StoredGeneratedTemplate> {
        self.generated_templates
            .iter()
            .filter(move |template| template.region == region)
    }

    /// How many generations of one Region are in a pending or in-progress status right
    /// now — the set [`generated_template::MAX_CONCURRENT_GENERATIONS`] meters.
    ///
    /// Counted off the stored statuses, so it is accurate only once [`Self::settle`] has
    /// retired the generations the clock already completed; every caller settles first.
    pub fn generations_in_flight(&self, region: &str) -> usize {
        self.generated_templates_in(region)
            .filter(|template| template.is_in_flight())
            .count()
    }

    /// The resource scan `resource_scan_id` addresses in the request's Region, if this store
    /// holds it: a scan is addressed by its own ARN, and nothing outside that Region is
    /// addressable.
    pub fn resolve_resource_scan(
        &self,
        region: &str,
        resource_scan_id: &str,
    ) -> Option<&resource_scans::StoredResourceScan> {
        self.resource_scans
            .iter()
            .find(|scan| scan.is_addressed_by(region, resource_scan_id))
    }

    /// The scan of one Region that is still running, if any: the set the modelled
    /// `ResourceScanInProgressException` meters, since "only one can be run at a time for an
    /// account in a Region".
    ///
    /// Read off the stored statuses, so it is accurate only once [`Self::settle`] has retired
    /// the scans the clock already concluded; every caller settles first.
    pub fn resource_scan_in_flight(
        &self,
        region: &str,
    ) -> Option<&resource_scans::StoredResourceScan> {
        self.resource_scans
            .iter()
            .find(|scan| scan.region == region && scan.is_in_flight())
    }

    /// The RUNNING scan of one Region a retry under `client_request_token` addresses: the
    /// token is "a unique identifier for this StartResourceScan request", so a retry while the
    /// scan it started is still running means that scan rather than a second one.
    ///
    /// Scoped to the scan still in flight: once it has concluded there is no request in flight
    /// to be retrying, so the same token starts the next scan instead of answering an old
    /// scan's id forever.
    pub fn resource_scan_by_token(
        &self,
        region: &str,
        client_request_token: &str,
    ) -> Option<&resource_scans::StoredResourceScan> {
        self.resource_scans.iter().find(|scan| {
            scan.region == region
                && scan.is_in_flight()
                && scan.client_request_token.as_deref() == Some(client_request_token)
        })
    }

    /// The resources of this account and Region a scan READS: every resource of every live
    /// stack of that Region that still exists, in stack order and then in the order the
    /// stack's template declared them, so a scan's rows are deterministic.
    ///
    /// A record a deployment left at `DELETE_COMPLETE` is left out: the stack goes on naming
    /// what became of that resource, while nothing of it exists to be found by a scan — the
    /// same set [`Self::current_resource_count`] counts as occupied.
    pub fn scannable_resources(&self, region: &str) -> Vec<resource_scans::FoundResource> {
        self.live_stacks(region)
            .flat_map(|stack| {
                // The stack is what TAGGED the resources it deployed, so the tags on each row
                // are read through the stack's own provenance.
                let provenance = stack.provenance();
                stack
                    .resources
                    .iter()
                    .filter(|deployed| deployed.status != resource_status::DELETE_COMPLETE)
                    .map(|deployed| resource_scans::FoundResource {
                        resource_type: deployed.resource.resource_type.clone(),
                        physical_resource_id: deployed.resource.physical_resource_id.clone(),
                        logical_resource_id: deployed.resource.logical_id.clone(),
                        stack_id: stack.stack_id.clone(),
                        // The primary identifier of the resource as it is when the scan
                        // READS it, so what a read reports is the row the scan took.
                        resource_identifier: resource_scans::primary_identifier(
                            &deployed.resource,
                        ),
                        tags: resource_scans::resource_tags(&deployed.resource, &provenance),
                    })
                    .collect::<Vec<resource_scans::FoundResource>>()
            })
            .collect()
    }

    /// Materialise every drift-detection STATUS whose due instant has passed.
    ///
    /// What the clock owes here is the detection operation's own status: AWS answers
    /// `DetectStackDrift` while the detection is still running
    /// ("DetectStackDrift can take up to several minutes"), so
    /// `DescribeStackDriftDetectionStatus` answers `DETECTION_IN_PROGRESS` with no result
    /// until this makes the conclusion due — and only ever an outcome already computed, so
    /// nothing is advanced because it was polled.
    ///
    /// The RESULT itself is not owed by the clock: the comparison against each owning
    /// service is what `DetectStackDrift` performs, and
    /// [`Self::apply_drift_result`] records what it found on the stack and its resources
    /// as of that check.
    fn settle_drift_detections(&mut self, now: SystemTime) {
        for detection in &mut self.drift_detections {
            if detection
                .pending
                .as_ref()
                .is_none_or(|pending| pending.due_at > now)
            {
                continue;
            }
            let pending = detection.pending.take().expect("checked just above");
            detection.detection_status = pending.detection_status;
            detection.detection_status_reason = pending.detection_status_reason;
            detection.stack_drift_status = Some(pending.stack_drift_status);
            detection.drifted_resource_count = Some(pending.drifted_resource_count);
        }
    }

    /// Record what a drift detection FOUND on the stack it ran against and on each
    /// resource it covered.
    ///
    /// `Stack$DriftInformation` is a statement about the stack as of its last check —
    /// "status of the stack's actual configuration compared to its expected template
    /// configuration" — not the status of a detection operation, so it carries the result
    /// of the check that ran rather than waiting on the detection record's own conclusion.
    /// `checked_at` is the instant the detection was initiated, which is what both
    /// `LastCheckTimestamp` members report.
    ///
    /// A resource of the stack the detection did NOT cover keeps the result it already
    /// carried: a filtered detection reports on what it checked and says nothing about the
    /// rest.
    pub fn apply_drift_result(
        &mut self,
        stack_id: &str,
        checked_at: SystemTime,
        stack_drift_status: &str,
        results: &[drift::ResourceDrift],
    ) {
        {
            let Some(stack) = self
                .stacks
                .iter_mut()
                .find(|stack| stack.stack_id == stack_id)
            else {
                // The stack was deleted while its detection was running: there is nothing
                // left to report the result on, and the detection record keeps it.
                return;
            };
            stack.drift_status = stack_drift_status.to_string();
            stack.drift_last_check = Some(checked_at);
        }
        for result in results {
            self.apply_resource_drift_result(stack_id, checked_at, result);
        }
    }

    /// Record what a drift check found on ONE resource, on that resource.
    ///
    /// This is the per-resource half of [`Self::apply_drift_result`] on its own, and the
    /// whole of what a check of a SINGLE resource may conclude: `Stack$DriftInformation` is
    /// "the status of the stack's actual configuration compared to its expected template
    /// configuration", a statement about every resource of the stack, so a check that looked
    /// at one resource leaves it as the last check over the whole stack left it rather than
    /// re-deciding it from the one resource it read.
    ///
    /// A `stack_id` or a logical id the store no longer holds records nothing: the resource
    /// was deleted while the check was running, and there is nothing left to report on.
    pub fn apply_resource_drift_result(
        &mut self,
        stack_id: &str,
        checked_at: SystemTime,
        result: &drift::ResourceDrift,
    ) {
        let Some(stack) = self
            .stacks
            .iter_mut()
            .find(|stack| stack.stack_id == stack_id)
        else {
            return;
        };
        if let Some(deployed) = stack
            .resources
            .iter_mut()
            .find(|deployed| deployed.resource.logical_id == result.logical_id)
        {
            deployed.drift_status = result.status.clone();
            deployed.drift_checked_at = Some(checked_at);
            deployed.drift_detail = result.detail.clone();
        }
    }

    /// Record a drift detection that has just been started, and hand back nothing: the
    /// caller already minted the id it answers with. The oldest record is reclaimed past
    /// [`MAX_DRIFT_DETECTIONS`], so a caller cannot grow this store without limit.
    pub fn record_drift_detection(&mut self, detection: StoredDriftDetection) {
        while self.drift_detections.len() >= MAX_DRIFT_DETECTIONS {
            self.drift_detections.remove(0);
        }
        self.drift_detections.push(detection);
    }

    /// Record one Hook invocation result. The oldest record is reclaimed past
    /// [`hook_results::MAX_HOOK_RESULTS`], and the oldest annotation of the record past
    /// [`hook_results::MAX_ANNOTATIONS_PER_RESULT`], so neither growth site can grow
    /// without limit.
    ///
    /// This is the ONLY way a record enters the store: a Hook invocation is what produces
    /// one, and [`hook_results`] is where what can and cannot produce one is written down.
    pub fn record_hook_result(&mut self, mut result: hook_results::StoredHookResult) {
        let excess = result
            .annotations
            .len()
            .saturating_sub(hook_results::MAX_ANNOTATIONS_PER_RESULT);
        result.annotations.drain(..excess);
        while self.hook_results.len() >= hook_results::MAX_HOOK_RESULTS {
            self.hook_results.remove(0);
        }
        self.hook_results.push(result);
    }

    /// Record one resource scan. The oldest record is reclaimed past
    /// [`resource_scans::MAX_RESOURCE_SCANS`], so the store cannot grow without limit.
    ///
    /// This is the ONLY way a scan enters the store: starting a scan is what creates one,
    /// and [`resource_scans`] is where what writes one is written down.
    pub fn record_resource_scan(&mut self, scan: resource_scans::StoredResourceScan) {
        while self.resource_scans.len() >= resource_scans::MAX_RESOURCE_SCANS {
            self.resource_scans.remove(0);
        }
        self.resource_scans.push(scan);
    }

    /// Record one stack refactor. The oldest record is reclaimed past
    /// [`stack_refactors::MAX_STACK_REFACTORS`], so the store cannot grow without limit.
    ///
    /// This is the ONLY way a refactor enters the store: `CreateStackRefactor` is what
    /// creates one, and [`stack_refactors`] is where what writes one is written down.
    pub fn record_stack_refactor(&mut self, refactor: stack_refactors::StoredStackRefactor) {
        while self.stack_refactors.len() >= stack_refactors::MAX_STACK_REFACTORS {
            self.stack_refactors.remove(0);
        }
        self.stack_refactors.push(refactor);
    }

    /// The stack refactor `stack_refactor_id` addresses in the request's Region, if this
    /// store holds it: a refactor is addressed by the id `CreateStackRefactor` handed back,
    /// and nothing outside that Region is addressable.
    pub fn resolve_stack_refactor(
        &self,
        region: &str,
        stack_refactor_id: &str,
    ) -> Option<&stack_refactors::StoredStackRefactor> {
        self.stack_refactors
            .iter()
            .find(|refactor| refactor.is_addressed_by(region, stack_refactor_id))
    }

    /// What one stack a refactor NAMES looks like in this store right now: its id, if this
    /// Region holds a live stack of that name, and the logical resource ids it currently
    /// holds.
    ///
    /// The one place a refactor resolves a stack name, so the ids it reports and the moves it
    /// validates read the same stacks ([`stack_refactors::plan`] is what judges them).
    /// Accurate only once [`Self::settle`] has installed the deployments the clock already
    /// completed; every caller settles first.
    pub fn covered_stack(
        &self,
        region: &str,
        stack_name: &str,
    ) -> stack_refactors::CoveredStack {
        let live = self
            .resolve(region, stack_name)
            .filter(|stack| stack.is_live());
        stack_refactors::CoveredStack {
            stack_name: stack_name.to_string(),
            stack_id: live.map(|stack| stack.stack_id.clone()),
            logical_resource_ids: live
                .map(|stack| {
                    stack
                        .current_resources()
                        .into_iter()
                        .map(|resource| resource.logical_id)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// The scans of one Region, NEWEST FIRST: `ListResourceScans` "list[s] the resource
    /// scans from newest to oldest", so the order is the records' own `StartTime` rather
    /// than the order they entered the store.
    ///
    /// Accurate only once [`Self::settle`] has materialised the completions the clock
    /// already made due; every caller settles first.
    pub fn resource_scans_newest_first(
        &self,
        region: &str,
    ) -> Vec<&resource_scans::StoredResourceScan> {
        let mut scans: Vec<&resource_scans::StoredResourceScan> = self
            .resource_scans
            .iter()
            .rev()
            .filter(|scan| scan.region == region)
            .collect();
        // A stable sort over the reversed store: scans that started at the same instant keep
        // the most recently recorded one first, so the whole order is "newest to oldest".
        scans.sort_by_key(|scan| ::std::cmp::Reverse(scan.start_time));
        scans
    }

    /// The ONE recorded Hook invocation result this Region holds under `hook_result_id`,
    /// or `None` when no record of it carries that id: a result is addressed by "the
    /// unique identifier (ID) of the Hook invocation result", within the Region the
    /// reading request was signed for, the way every other read of this service is.
    pub fn hook_result(
        &self,
        region: &str,
        hook_result_id: &str,
    ) -> Option<&hook_results::StoredHookResult> {
        self.hook_results
            .iter()
            .find(|result| result.region == region && result.has_id(hook_result_id))
    }

    /// Record one resource-handler invocation. The oldest record is reclaimed past
    /// [`handler_progress::MAX_HANDLER_INVOCATIONS`], so the store cannot grow without
    /// limit.
    ///
    /// This is the ONLY way a record enters the store: invoking a handler is what issues
    /// the bearer token one is addressed by, and [`handler_progress`] is where what can and
    /// cannot issue one is written down.
    pub fn record_handler_invocation(
        &mut self,
        invocation: handler_progress::StoredHandlerInvocation,
    ) {
        while self.handler_invocations.len() >= handler_progress::MAX_HANDLER_INVOCATIONS {
            self.handler_invocations.remove(0);
        }
        self.handler_invocations.push(invocation);
    }

    /// Record the progress a resource handler reports for the invocation its `BearerToken`
    /// names, and hand back `Err(message)` for a report this invocation cannot take —
    /// which is the WHOLE of what `RecordHandlerProgress` decides (see
    /// [`handler_progress`]): a token naming no invocation of this Region, a
    /// `CurrentOperationStatus` that is not the status the invocation holds, and an
    /// `OperationStatus` that does not [`handler_progress::follows`] it. A refused report
    /// writes nothing; an accepted one leaves the invocation holding the reported status.
    pub fn record_handler_progress(
        &mut self,
        region: &str,
        bearer_token: &str,
        current_operation_status: Option<&str>,
        operation_status: &str,
    ) -> Result<(), String> {
        let Some(invocation) = self
            .handler_invocations
            .iter_mut()
            .find(|invocation| invocation.is_addressed_by(region, bearer_token))
        else {
            return Err(handler_progress::unknown_bearer_token(bearer_token));
        };
        // The conditional check comes first: `CurrentOperationStatus` is what the report
        // asserts the invocation holds, so a report that is wrong about that is refused
        // whatever it goes on to report.
        if let Some(asserted) = current_operation_status {
            if asserted != invocation.operation_status {
                return Err(handler_progress::operation_status_check_failed(
                    &invocation.operation_status,
                    asserted,
                ));
            }
        }
        if !handler_progress::follows(&invocation.operation_status, operation_status) {
            return Err(handler_progress::invalid_state_transition(
                &invocation.operation_status,
                operation_status,
            ));
        }
        invocation.operation_status = operation_status.to_string();
        Ok(())
    }

    /// The stack set `name_or_id` addresses in the request's Region, if this store holds it:
    /// `StackSetName` is "the name or unique ID of the StackSet", and nothing outside that
    /// Region is addressable — nor is a stack set a deletion has already taken, whose summary
    /// stays for `ListStackSets` alone (see [`stack_sets::StoredStackSet::is_addressed_by`]).
    pub fn resolve_stack_set(
        &self,
        region: &str,
        name_or_id: &str,
    ) -> Option<&stack_sets::StoredStackSet> {
        self.stack_sets
            .iter()
            .find(|stack_set| stack_set.is_addressed_by(region, name_or_id))
    }

    /// Record one stack set, replacing the stack set of the same name in the same Region if
    /// this store already holds one — a stack set's name is unique within the administrator
    /// account and Region. The oldest record is reclaimed past
    /// [`stack_sets::MAX_STACK_SETS`], so the store cannot grow without limit.
    ///
    /// This is the ONLY way a stack set enters the store, and [`stack_sets`] is where what
    /// writes one is written down.
    pub fn record_stack_set(&mut self, stack_set: stack_sets::StoredStackSet) {
        if let Some(held) = self.stack_sets.iter_mut().find(|held| {
            held.region == stack_set.region && held.stack_set_name == stack_set.stack_set_name
        }) {
            *held = stack_set;
            return;
        }
        // Reclaim enough of the oldest records to leave room for this one.
        if self.stack_sets.len() >= stack_sets::MAX_STACK_SETS {
            let excess = self.stack_sets.len() - stack_sets::MAX_STACK_SETS + 1;
            self.stack_sets.drain(..excess);
        }
        self.stack_sets.push(stack_set);
    }

    /// Every stack set of one Region, in creation order: the set `ListStackSets` answers
    /// from, since a stack set belongs to the Region the call that created it was signed
    /// for and nothing outside that Region is addressable (see [`stack_sets`]).
    pub fn stack_sets_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a stack_sets::StoredStackSet> {
        self.stack_sets
            .iter()
            .filter(move |stack_set| stack_set.region == region)
    }

    /// [`Self::resolve_stack_set`], for a caller that mutates the stack set it resolved.
    pub fn resolve_stack_set_mut(
        &mut self,
        region: &str,
        name_or_id: &str,
    ) -> Option<&mut stack_sets::StoredStackSet> {
        self.stack_sets
            .iter_mut()
            .find(|stack_set| stack_set.is_addressed_by(region, name_or_id))
    }

    /// The exported output values in force in one Region, keyed by export name.
    ///
    /// "Lists all exported output values in the account and Region in which you call this
    /// action" — the same set `ListExports` reports, which is what a deployment's
    /// `Fn::ImportValue` resolves against: every output of a LIVE stack of that Region that
    /// declares an `Export` name and whose value this service evaluated. An export name two
    /// stacks publish cannot happen on AWS ("export names must be unique within a Region"),
    /// and the first stack that entered the store wins here.
    pub fn exported_values(&self, region: &str) -> BTreeMap<String, String> {
        let mut exports = BTreeMap::new();
        for stack in self.live_stacks(region) {
            for output in &stack.outputs {
                let (Some(name), Some(value)) = (&output.export_name, &output.value) else {
                    continue;
                };
                exports
                    .entry(name.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        exports
    }

    /// Every output value a deployment in one Region can read with an `Fn::GetStackOutput`,
    /// keyed by the reference that reads one ([`stack_outputs::values_in`]).
    ///
    /// Read where a template is DEPLOYED, exactly as [`Self::exported_values`] is: that
    /// function reads another stack's output by the stack and the output key rather than by
    /// an export name, so what it resolves against is every output of every live stack of
    /// the account and Region the deployment runs in.
    pub fn stack_output_values(&self, region: &str) -> stack_outputs::Values {
        stack_outputs::values_in(self.live_stacks(region))
    }

    /// The names of the live stacks of one Region that IMPORT `export_name`, in creation
    /// order.    ///
    /// `ListImports` "lists all stacks that are importing an exported output value": a
    /// stack imports one when the template it deployed resolved a `Fn::ImportValue` for
    /// that name ([`StoredStack::imports`]), so it enters the index when its deployment
    /// completes and leaves it when the stack is deleted.
    pub fn importing_stacks(&self, region: &str, export_name: &str) -> Vec<String> {
        self.live_stacks(region)
            .filter(|stack| stack.imports.contains_key(export_name))
            .map(|stack| stack.stack_name.clone())
            .collect()
    }

    /// The first export of `stack` another live stack of the same Region still imports, as
    /// the pair (export name, importing stack name).
    ///
    /// An exported output value that is in use cannot be removed: deleting the stack that
    /// publishes it would break every stack that imported it, so the deletion CloudFormation
    /// took is CANCELLED ([`StoredStack::begin_canceled_deletion`]). `None` when nothing the
    /// stack exports is imported, which is when its deletion is free to proceed.
    pub fn export_in_use(&self, stack: &StoredStack) -> Option<(String, String)> {
        for output in &stack.outputs {
            let Some(export_name) = &output.export_name else {
                continue;
            };
            let importer = self
                .live_stacks(&stack.region)
                .find(|other| {
                    other.stack_id != stack.stack_id && other.imports.contains_key(export_name)
                })
                .map(|other| other.stack_name.clone());
            if let Some(importer) = importer {
                return Some((export_name.clone(), importer));
            }
        }
        None
    }

    /// The drift detection with this `StackDriftDetectionId`, if this store still holds it.
    pub fn drift_detection(&self, detection_id: &str) -> Option<&StoredDriftDetection> {
        self.drift_detections
            .iter()
            .find(|detection| detection.detection_id == detection_id)
    }

    /// The same record, for the call that fills in the outcome it computed.
    pub fn drift_detection_mut(
        &mut self,
        detection_id: &str,
    ) -> Option<&mut StoredDriftDetection> {
        self.drift_detections
            .iter_mut()
            .find(|detection| detection.detection_id == detection_id)
    }

    /// Drop the change sets whose stack is no longer live.
    ///
    /// Not a mutation a caller asked for: a change set describes changes to a stack, so it
    /// is gone the moment that stack's deletion completes — which was owed at the same
    /// instant whether or not anybody called. It is also what keeps the change-set store
    /// from outliving the stacks it counts against.
    fn reclaim_orphaned_change_sets(&mut self) {
        if self.change_sets.is_empty() {
            return;
        }
        // Disjoint fields: the live-id set borrows `stacks`, the retain mutates
        // `change_sets`.
        let live: ::std::collections::HashSet<&str> = self
            .stacks
            .iter()
            .filter(|stack| stack.is_live())
            .map(|stack| stack.stack_id.as_str())
            .collect();
        self.change_sets
            .retain(|change_set| live.contains(change_set.stack_id.as_str()));
        self.reclaim_discarded_change_sets();
    }

    /// Drop the oldest DISCARDED change sets past
    /// [`MAX_DISCARDED_CHANGE_SETS_RETAINED`].
    ///
    /// Retention, not a mutation a caller asked for: a discarded plan is over the bound the
    /// moment the delete that pushed the count past it was answered, whether or not anybody
    /// called. All that is left of such a record is the execution it carries
    /// ([`StoredChangeSet::plan_discarded`]), so this is what keeps a caller that creates,
    /// executes and discards change sets in a loop from filling the store with them. Plans
    /// nothing discarded are never touched.
    fn reclaim_discarded_change_sets(&mut self) {
        let discarded = self
            .change_sets
            .iter()
            .filter(|change_set| change_set.plan_discarded)
            .count();
        if discarded <= MAX_DISCARDED_CHANGE_SETS_RETAINED {
            return;
        }
        let mut over = discarded - MAX_DISCARDED_CHANGE_SETS_RETAINED;
        // `change_sets` is in creation order, so the oldest discarded records are the ones
        // this walk reaches first.
        self.change_sets.retain(|change_set| {
            if over > 0 && change_set.plan_discarded {
                over -= 1;
                return false;
            }
            true
        });
    }

    /// The change sets of one stack, in creation order: what `ListChangeSets` reports and
    /// what [`MAX_CHANGE_SETS_PER_STACK`] bounds.
    pub fn change_sets_of<'a>(
        &'a self,
        stack_id: &'a str,
    ) -> impl Iterator<Item = &'a StoredChangeSet> {
        self.change_sets
            .iter()
            .filter(move |change_set| change_set.stack_id == stack_id && !change_set.plan_discarded)
    }

    /// Resolve the `ChangeSetName` input member, which CloudFormation accepts as either a
    /// change set name or its ARN: an ARN names the change set on its own, while a name is
    /// resolved among the change sets of the stack the request names.
    ///
    /// Scoped to the Region the request was signed for, like the stacks the change sets
    /// belong to ([`Self::resolve`]): a change set of another Region's stack names nothing
    /// here.
    pub fn resolve_change_set(
        &self,
        region: &str,
        name_or_id: &str,
        stack_name_or_id: Option<&str>,
    ) -> Option<&StoredChangeSet> {
        self.change_set_position(region, name_or_id, stack_name_or_id)
            .map(|index| &self.change_sets[index])
    }

    /// [`Self::resolve_change_set`], for a caller that mutates the change set it resolved.
    pub fn resolve_change_set_mut(
        &mut self,
        region: &str,
        name_or_id: &str,
        stack_name_or_id: Option<&str>,
    ) -> Option<&mut StoredChangeSet> {
        self.change_set_position(region, name_or_id, stack_name_or_id)
            .map(|index| &mut self.change_sets[index])
    }

    /// [`Self::resolve_change_set`] for `ExecuteChangeSet`, which also addresses a change
    /// set whose PLAN has been discarded.
    ///
    /// Deleting a change set whose execution had already been carried out discards the plan
    /// a read reports, but the execution it performed stays the stack's history
    /// ([`StoredChangeSet::plan_discarded`]): a call asking for that change set to be
    /// executed again is therefore refused on the execution status it is in, where a read of
    /// the discarded plan answers the modelled not-found.
    ///
    /// A LIVE change set always wins: the name of a discarded plan is free again — nothing
    /// lists or reads it — so a caller may create another change set under it, and that new
    /// plan is the one this call is about. The discarded record answers only where no plan of
    /// that name is there to execute.
    pub fn resolve_executed_change_set(
        &self,
        region: &str,
        name_or_id: &str,
        stack_name_or_id: Option<&str>,
    ) -> Option<&StoredChangeSet> {
        self.change_set_position_among(region, name_or_id, stack_name_or_id, false)
            .or_else(|| {
                self.change_set_position_among(region, name_or_id, stack_name_or_id, true)
            })
            .map(|index| &self.change_sets[index])
    }

    fn change_set_position(
        &self,
        region: &str,
        name_or_id: &str,
        stack_name_or_id: Option<&str>,
    ) -> Option<usize> {
        self.change_set_position_among(region, name_or_id, stack_name_or_id, false)
    }

    /// [`Self::change_set_position`] among the records whose plan is there (`discarded` =
    /// `false`) or among the records whose plan was discarded (`discarded` = `true`), which
    /// only [`Self::resolve_executed_change_set`] asks for.
    ///
    /// The LAST matching record answers, so where one name has been used again the most
    /// recent change set under it is the one addressed.
    fn change_set_position_among(
        &self,
        region: &str,
        name_or_id: &str,
        stack_name_or_id: Option<&str>,
        discarded: bool,
    ) -> Option<usize> {
        let wanted = |change_set: &StoredChangeSet| change_set.plan_discarded == discarded;
        // The change set's own ARN identifies it; a stack named beside it must be the
        // stack it belongs to, or the pair names nothing.
        if let Some(index) = self
            .change_sets
            .iter()
            .rposition(|change_set| change_set.change_set_id == name_or_id && wanted(change_set))
        {
            let change_set = &self.change_sets[index];
            // The change set is this Region's only where the stack it plans changes for
            // is.
            self.resolve(region, &change_set.stack_id)?;
            let stack_matches = stack_name_or_id.is_none_or(|stack| {
                stack == change_set.stack_id || stack == change_set.stack_name
            });
            return stack_matches.then_some(index);
        }
        // A bare name is resolved within one stack ("If you specify the name of a change
        // set, you must also specify the stack name").
        let stack_name_or_id = stack_name_or_id?;
        let stack_id = self.resolve(region, stack_name_or_id)?.stack_id.clone();
        self.change_sets.iter().rposition(|change_set| {
            change_set.stack_id == stack_id
                && change_set.change_set_name == name_or_id
                && wanted(change_set)
        })
    }

    /// Drop the oldest completed deletions past [`MAX_DELETED_STACKS_RETAINED`].
    ///
    /// Retention, not a mutation a caller asked for: a deleted stack is over the bound the
    /// moment the deletion that pushed the count past it completed, whether or not anybody
    /// called. Live stacks are never touched.
    fn reclaim_retired_stacks(&mut self) {
        // Early-out before allocating anything: all but the rare call that completes a
        // deletion past the bound stops here.
        let retired_count = self.stacks.iter().filter(|stack| !stack.is_live()).count();
        if retired_count <= MAX_DELETED_STACKS_RETAINED {
            return;
        }
        let mut retired: Vec<(SystemTime, usize)> = self
            .stacks
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.is_live())
            .map(|(index, stack)| (stack.deletion_time.unwrap_or(stack.creation_time), index))
            .collect();
        // Oldest deletion first, so the rows reclaimed are the ones furthest past the
        // retention bound.
        retired.sort_by_key(|(deleted_at, index)| (*deleted_at, *index));
        let over = retired.len() - MAX_DELETED_STACKS_RETAINED;
        let reclaimed: ::std::collections::HashSet<usize> =
            retired.into_iter().take(over).map(|(_, index)| index).collect();
        // Rebuilt by index rather than driven by a counter beside `retain`, so nothing
        // depends on the order `retain` happens to visit elements in.
        self.stacks = ::std::mem::take(&mut self.stacks)
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !reclaimed.contains(index))
            .map(|(_, stack)| stack)
            .collect();
    }

    /// Resolve the `StackName` input member, which CloudFormation accepts as either a
    /// stack name or a stack id: a stack id matches any stack, live or deleted, while a
    /// name matches only a live one.
    ///
    /// `region` is the Region the request was signed for, and nothing OUTSIDE it is
    /// resolved: a stack lives "in the Region in which you are creating the stack", so the
    /// same name in another Region is another stack, and a stack id — which carries its own
    /// Region — names no stack HERE when that Region is not this one.
    pub fn resolve(&self, region: &str, name_or_id: &str) -> Option<&StoredStack> {
        self.position_of(region, name_or_id).map(|i| &self.stacks[i])
    }

    /// [`Self::resolve`], for a caller that mutates the stack it resolved.
    pub fn resolve_mut(&mut self, region: &str, name_or_id: &str) -> Option<&mut StoredStack> {
        self.position_of(region, name_or_id)
            .map(|i| &mut self.stacks[i])
    }

    /// The stack a request names, including one whose deletion has COMPLETED and which a
    /// NAME therefore no longer resolves to ([`Self::resolve`]).
    ///
    /// CloudFormation keeps a deleted stack addressable — "information for stacks that have
    /// been deleted is kept for 90 days" — so a request whose answer depends on what became
    /// of that stack's resources is decided from its record rather than answered as if the
    /// stack had never existed. The MOST RECENT stack of the Region carrying the name, since
    /// a name is reusable once the stack holding it is gone and the newest record is the one
    /// the request is about; the stacks are held in creation order, so that is the last one.
    pub fn resolve_any(&self, region: &str, name_or_id: &str) -> Option<&StoredStack> {
        self.resolve(region, name_or_id).or_else(|| {
            self.stacks
                .iter()
                .rfind(|stack| stack.region == region && stack.stack_name == name_or_id)
        })
    }

    /// `Some(name_or_id)` when a request named a stack this Region holds none for, and
    /// `None` when it resolves or when the request named no stack at all.
    ///
    /// What an operation that addresses a NESTED record — a change set, which is addressed
    /// "by its name plus the stack it belongs to" — checks BEFORE it looks that record up:
    /// the outer resource is resolved first, so a request naming a stack that is not
    /// addressable is refused about the STACK rather than about the record the nested lookup
    /// could not reach inside it. A client can then tell a bad stack name (fix the name)
    /// from a record that is really missing (create it), and the two refusals keep their own
    /// status codes.
    ///
    /// The refusal itself is worded by the OPERATION ([`stack_does_not_exist`] and the forms
    /// beside it), so this answers WHICH name did not resolve rather than a message.
    pub fn unresolved_stack<'a>(
        &self,
        region: &str,
        name_or_id: Option<&'a str>,
    ) -> Option<&'a str> {
        name_or_id.filter(|name_or_id| self.resolve(region, name_or_id).is_none())
    }

    /// The live stacks of one Region, in creation order: what `DescribeStacks` reports when
    /// no `StackName` is given ("Deleted stacks don't show up in DescribeStacks if the
    /// deletion has been completed successfully"), and the set a stack NAME has to be
    /// unique within.
    pub fn live_stacks<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a StoredStack> {
        self.stacks
            .iter()
            .filter(move |stack| stack.is_live() && stack.region == region)
    }

    /// Every stack of one Region, live or deleted, in creation order: the set a read that
    /// reports deleted stacks too (`ListStacks`) answers from.
    pub fn stacks_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a StoredStack> {
        self.stacks
            .iter()
            .filter(move |stack| stack.region == region)
    }

    /// How many live stacks this store holds across every Region: what
    /// [`MAX_LIVE_STACKS`] bounds, since the per-Region quota is metered on a Region the
    /// caller chooses.
    pub fn live_stack_count(&self) -> usize {
        self.stacks.iter().filter(|stack| stack.is_live()).count()
    }

    /// How many resources this store's live stacks hold at once, across every Region: what
    /// [`MAX_CONCURRENT_RESOURCES`] bounds, since the Region a request is served in is one
    /// the caller chooses (the same reason [`Self::live_stack_count`] is a flat count).
    ///
    /// A deleted stack's resources are not counted — CloudFormation keeps reporting them
    /// for as long as it reports the stack, but nothing of theirs exists any more, so they
    /// do not occupy the account's concurrent-resource quota.
    pub fn current_resource_count(&self) -> usize {
        self.stacks
            .iter()
            .filter(|stack| stack.is_live())
            .map(StoredStack::current_resource_count)
            .sum()
    }

    /// The account limits this store reports and enforces, in the order CloudFormation
    /// reports them.
    ///
    /// `DescribeAccountLimits` "retrieves your account's CloudFormation limits", and
    /// `AccountLimit` names exactly three of them: the stacks, the stack outputs and the
    /// concurrent resources this account may hold. Each value is the ceiling this store
    /// itself holds a caller to — [`MAX_STACKS`], [`template::MAX_OUTPUTS_PER_TEMPLATE`] and
    /// [`MAX_CONCURRENT_RESOURCES`] — so the limits a caller reads are the limits its next
    /// write is measured against, rather than numbers nothing honours.
    ///
    /// The ORDER is the service's own, which is not the alphabetical order the model's
    /// documentation of `AccountLimit$Name` happens to list the values in: the per-STACK
    /// quotas come first and the per-account resource ceiling last, which is the order a
    /// client reading a limit positionally reads them in.
    pub fn account_limits(&self) -> Vec<crate::ops::types::AccountLimit> {
        [
            (account_limit_name::STACK, MAX_STACKS),
            (
                account_limit_name::STACK_OUTPUTS,
                template::MAX_OUTPUTS_PER_TEMPLATE,
            ),
            (
                account_limit_name::CONCURRENT_RESOURCES,
                MAX_CONCURRENT_RESOURCES,
            ),
        ]
        .into_iter()
        .map(|(name, value)| crate::ops::types::AccountLimit {
            name: Some(name.to_string()),
            // Every ceiling above is a small count, so the modelled `LimitValue`
            // (an integer) carries it; a ceiling past what it can carry is reported
            // saturated rather than wrapped into a negative limit.
            value: Some(i32::try_from(value).unwrap_or(i32::MAX)),
        })
        .collect()
    }

    /// The stack a resource with this physical id belongs to, which is what
    /// `DescribeStackResources` resolves its `PhysicalResourceId` member to ("You can pass
    /// the EC2 InstanceId to DescribeStackResources to find which stack the instance
    /// belongs to and what other resources are part of the stack").
    ///
    /// Searched over the LIVE stacks of the request's Region, in creation order: a physical
    /// id names a resource that EXISTS, and a stack whose deletion has completed holds none
    /// — its records say what it once deployed, which is what keeps the stack itself readable
    /// by its id, but nothing of those resources is there to find a stack from. So a request
    /// that can only name its stack through such a resource resolves nothing and is refused
    /// ([`stack_for_physical_resource_does_not_exist`]), rather than reporting a deleted
    /// resource as a live one.
    ///
    /// The first stack carrying the id wins, and this emulator derives a generated physical
    /// id from the stack id, so two stacks collide only where their templates NAME the same
    /// resource.
    pub fn stack_of_physical_resource(
        &self,
        region: &str,
        physical_resource_id: &str,
    ) -> Option<&StoredStack> {
        if physical_resource_id.is_empty() {
            // A resource that was never created carries no physical id, so nothing is
            // addressable by one: the empty string must not resolve to the first stack
            // holding such a resource.
            return None;
        }
        self.stacks
            .iter()
            .filter(|stack| stack.region == region && stack.is_live())
            .find(|stack| {
                stack.resources.iter().any(|deployed| {
                    deployed.resource.physical_resource_id == physical_resource_id
                })
            })
    }

    /// The registry record a `Type`-and-`TypeName` request names, in the Region the
    /// request was signed for: `DescribeType` reads an extension "from the CloudFormation
    /// registry in your current account and Region", so the same name in another Region
    /// names no record.
    ///
    /// A name can be held by TWO records at once — the private registration this account
    /// made and the PUBLIC extension a publication of it wrote, which carries the same kind
    /// and name under its own account-less ARN — and this answers whichever entered the
    /// registry first. An operation that must act on the REGISTRATION resolves it through
    /// [`Self::registered_extension_by_name`] instead, so its answer does not depend on that
    /// order.
    pub fn extension_by_name(
        &self,
        region: &str,
        type_kind: &str,
        type_name: &str,
    ) -> Option<&registry::StoredExtension> {
        self.extensions.iter().find(|extension| {
            extension.region == region
                && extension.type_kind == type_kind
                && extension.type_name == type_name
        })
    }

    /// The record a `Type`-and-`TypeName` request names that this account REGISTERED: the
    /// private one, "the extension ... only visible and usable within the account in which
    /// it is registered".
    ///
    /// [`Self::extension_by_name`] narrowed to the records a registry WRITER may act on, so
    /// a name a publication has also written a public record for still resolves to the
    /// registration.
    pub fn registered_extension_by_name(
        &self,
        region: &str,
        type_kind: &str,
        type_name: &str,
    ) -> Option<&registry::StoredExtension> {
        self.extensions.iter().find(|extension| {
            extension.region == region
                && extension.type_kind == type_kind
                && extension.type_name == type_name
                && extension.is_private()
        })
    }

    /// Every registry record of ONE Region, in the order the records entered the registry.
    ///
    /// `ListTypes` reports the extensions of "your current account and Region", so the
    /// listing is namespaced exactly as a single read is: a record another Region's
    /// registry holds is not among them — the same gate [`Self::extension_by_name`]
    /// applies to the name form.
    pub fn extensions_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a registry::StoredExtension> {
        self.extensions
            .iter()
            .filter(move |extension| extension.region == region)
    }

    /// The registry record an `Arn` request names, in the Region the request was signed
    /// for. An extension's ARN carries its Region and its kind already, and the registry
    /// is regional like the records it holds, so an ARN minted for another Region names no
    /// record HERE — the same gate [`Self::extension_by_name`] applies to the name form.
    pub fn extension_by_arn(&self, region: &str, arn: &str) -> Option<&registry::StoredExtension> {
        self.extensions
            .iter()
            .find(|extension| extension.region == region && extension.arn == arn)
    }

    /// [`Self::extension_by_arn`], for a registry WRITER: the record a request named by
    /// `Arn`, in the Region the request was signed for.
    pub fn extension_by_arn_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut registry::StoredExtension> {
        self.extensions
            .iter_mut()
            .find(|extension| extension.region == region && extension.arn == arn)
    }

    /// Record one extension REGISTRATION REQUEST. The oldest record is reclaimed past
    /// [`registry::MAX_TYPE_REGISTRATIONS`], so the store cannot grow without limit.
    ///
    /// This is the ONLY way a registration enters the store: starting a registration is what
    /// issues the token one is addressed by, and `RegisterType` — the registry writer that
    /// does so, still a scaffold stub with an issue of its own — writes through here rather
    /// than reaching into the vector, so every reader sees the same records.
    pub fn record_type_registration(&mut self, registration: registry::StoredTypeRegistration) {
        while self.type_registrations.len() >= registry::MAX_TYPE_REGISTRATIONS {
            self.type_registrations.remove(0);
        }
        self.type_registrations.push(registration);
    }

    /// Every registration request of ONE Region, in the order the requests were started.
    ///
    /// `ListTypeRegistrations` reports the registrations of the account and Region its
    /// request was signed for, the way every other registry read is namespaced: a request
    /// another Region holds is not among them.
    pub fn type_registrations_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a registry::StoredTypeRegistration> {
        self.type_registrations
            .iter()
            .filter(move |registration| registration.region == region)
    }

    /// The ONE registration request this Region holds under `registration_token`, or `None`
    /// when no record of it carries that token.
    pub fn type_registration(
        &self,
        region: &str,
        registration_token: &str,
    ) -> Option<&registry::StoredTypeRegistration> {
        self.type_registrations
            .iter()
            .find(|registration| registration.is_addressed_by(region, registration_token))
    }

    /// The PUBLIC extension record this Region's registry holds for this extension, if a
    /// publication has written one.
    ///
    /// That record is what a publication reads to decide the version number the next one is
    /// assigned ("if you don't specify a version number, CloudFormation increments the
    /// version number by one minor version release") and which publisher the extension is
    /// already published by, and it is a record of its own rather than the private
    /// registration it was published from: a public extension "is publicly visible and
    /// usable within any Amazon Web Services account", which a private registration is not.
    ///
    /// Resolved by the ARN a publication of this extension is addressed by, which is the
    /// SAME predicate [`Self::publish_extension_version`] writes through: the read that
    /// decides a publication and the write that performs it cannot land on two different
    /// records.
    pub fn published_extension(
        &self,
        region: &str,
        type_kind: &str,
        type_name: &str,
    ) -> Option<&registry::StoredExtension> {
        let arn = registry::public_type_arn(region, type_kind, type_name);
        self.extensions
            .iter()
            .find(|extension| extension.region == region && extension.arn == arn)
    }

    /// Publish one version of a private extension as a public extension of this Region, and
    /// answer the ARN the public extension is addressed by.
    ///
    /// The FIRST publication adds the public record ([`registry::published_record`]); a
    /// later one adds its version to that same record and moves the record's
    /// `LatestPublicVersion` to the number just assigned, so every version a publisher has
    /// published stays addressable by the public version number it was published under.
    /// Bounded at [`registry::MAX_PUBLISHED_VERSIONS`] versions per record and
    /// [`registry::MAX_EXTENSIONS`] records, oldest reclaimed first in both: a caller
    /// chooses how many publications it makes and under how many names.
    ///
    /// The record it writes through is the one [`Self::published_extension`] reads, so a
    /// publication whose publisher the caller is not is refused by its handler before this
    /// is reached rather than appended to another publisher's record here.
    pub fn publish_extension_version(&mut self, publication: registry::Publication<'_>) -> String {
        let arn = publication.public_arn();
        match self
            .extensions
            .iter_mut()
            .find(|extension| extension.region == publication.region && extension.arn == arn)
        {
            Some(record) => {
                let version_id = registry::next_public_version_id(&record.versions);
                record.publisher_id = Some(publication.publisher_id.to_string());
                record.latest_public_version =
                    Some(publication.public_version_number.to_string());
                record.versions.push(registry::published_version(
                    &arn,
                    &version_id,
                    &publication,
                ));
                while record.versions.len() > registry::MAX_PUBLISHED_VERSIONS {
                    record.versions.remove(0);
                }
            }
            None => {
                let version = registry::published_version(
                    &arn,
                    &registry::next_public_version_id(&[]),
                    &publication,
                );
                // A publication adds a record under the kind and name the CALLER chose, so
                // this is a caller-counted growth site exactly as an activation is: the
                // oldest record is reclaimed past the registry's ceiling, so the registry
                // cannot grow without limit.
                let over_the_bound =
                    (self.extensions.len() + 1).saturating_sub(registry::MAX_EXTENSIONS);
                if over_the_bound > 0 {
                    self.extensions.drain(..over_the_bound);
                }
                self.extensions
                    .push(registry::published_record(&publication, version));
            }
        }
        arn
    }

    /// Register this account as a publisher of public extensions, and answer the publisher
    /// id the registration is addressed by.
    ///
    /// The id is minted on the FIRST registration and kept from then on: it is "the ID
    /// assigned this account by CloudFormation", one per account rather than one per call,
    /// and the operation is `@idempotent`, so registering again re-verifies the account's
    /// identity — the record carries the verification the latest registration supplied —
    /// without handing out a second id.
    ///
    /// `Err` carries [`publisher::NOT_A_MARKETPLACE_SELLER`] for an account that is not
    /// enrolled as an Amazon Web Services Marketplace seller, which is what a publisher
    /// registration requires of the account registering
    /// ([`Self::marketplace_seller`]) — and the gate lives HERE, at the one write, so no
    /// caller can register an ineligible account by reaching the store directly, and a
    /// refused registration cannot have written anything.
    pub fn register_publisher(
        &mut self,
        connection_arn: Option<String>,
    ) -> Result<String, &'static str> {
        if !self.marketplace_seller.is_enrolled() {
            return Err(publisher::NOT_A_MARKETPLACE_SELLER);
        }
        Ok(match &mut self.publisher {
            Some(registered) => {
                registered.status = publisher::publisher_status::VERIFIED.to_string();
                registered.connection_arn = connection_arn;
                registered.publisher_id.clone()
            }
            none => {
                let registered =
                    publisher::StoredPublisher::new(new_publisher_id(), connection_arn);
                let publisher_id = registered.publisher_id.clone();
                *none = Some(registered);
                publisher_id
            }
        })
    }

    /// The publisher record a request addresses: the one `publisher_id` names, or this
    /// account's own where the request names none ("if you don't supply a PublisherId, and
    /// you have registered as an extension publisher, DescribePublisher returns information
    /// about your own publisher account").
    ///
    /// `None` where the store holds no such registration — an account that has not
    /// registered, or an id that is not the one it registered under.
    pub fn publisher(&self, publisher_id: Option<&str>) -> Option<&publisher::StoredPublisher> {
        let registered = self.publisher.as_ref()?;
        match publisher_id {
            Some(wanted) if registered.publisher_id != wanted => None,
            _ => Some(registered),
        }
    }

    fn position_of(&self, region: &str, name_or_id: &str) -> Option<usize> {
        self.stacks
            .iter()
            .position(|stack| stack.region == region && stack.stack_id == name_or_id)
            .or_else(|| {
                self.stacks.iter().position(|stack| {
                    stack.region == region && stack.stack_name == name_or_id && stack.is_live()
                })
            })
    }
}

/// A resource's `PhysicalResourceId`, which a resource that was never CREATED does not
/// have.
///
/// A resource whose create the service that owns it refused carries no identifier at all,
/// and `StackResource$PhysicalResourceId` is optional, so it is reported ABSENT — an id
/// nothing minted is what a client must never be handed.
pub fn physical_resource_id(resource: &StoredResource) -> Option<String> {
    (!resource.physical_resource_id.is_empty()).then(|| resource.physical_resource_id.clone())
}

/// Stored outputs as the modelled `Outputs` member.
///
/// A declared output whose value this service could not evaluate carries no
/// `OutputValue`: the member is optional, and an absent value is the truthful answer
/// where an invented identifier would not be.
pub fn output_views(outputs: &[StoredOutput]) -> Option<Vec<crate::ops::types::Output>> {
    non_empty(
        outputs
            .iter()
            .map(|output| crate::ops::types::Output {
                output_key: Some(output.key.clone()),
                output_value: output.value.clone(),
                description: output.description.clone(),
                export_name: output.export_name.clone(),
            })
            .collect(),
    )
}

/// The template a body declares, ready to compare against another revision of it.
///
/// Read by the SAME reader a deployment uses ([`template::parse`], which takes a JSON or a
/// YAML body under this service's template bounds), then SORTED by logical id — and that
/// sort is load-bearing rather than a preference: the parsed object iterates in insertion
/// order when anything in the build enables `serde_json`'s `preserve_order` feature
/// (`vendor/smithy-macros` does) and in sorted order otherwise, so without it one change
/// set would report its changes in two different orders depending on how the workspace was
/// built.
///
/// `None` for a body this service will not read at all; `Some([])` for one that declares no
/// resources — which `CreateChangeSet` refuses, since a change set is a plan over resources.
pub fn declared_template(template_body: &str) -> Option<template::Template> {
    declared_template_within(template_body, TEMPLATE_BODY_MAX_BYTES)
}

/// [`declared_template`] under the caller's OWN ceiling on the body
/// ([`template::parse_within`]), for an operation that reads a template larger than the quota
/// on one submitted for deployment ([`TEMPLATE_MAX_BYTES`]).
pub fn declared_template_within(
    template_body: &str,
    max_bytes: usize,
) -> Option<template::Template> {
    let mut declared = template::parse_within(template_body, max_bytes)?;
    declared
        .resources
        .sort_by(|left, right| left.logical_id.cmp(&right.logical_id));
    Some(declared)
}

/// The resource-level change list a change set carries, computed by comparing the stack's
/// revision with the submitted one — see [`change_plan`].
pub use change_plan::computed_changes;

/// [`computed_changes`] against a stack whose template this call has not read yet, which is
/// how a handler holds one — see [`change_plan::changes_against`].
pub use change_plan::{changes_against, DeployedStack};

/// Resolve the request's `Parameters` against the values the stack already carries.
///
/// A request with no `Parameters` at all leaves the stack's parameters as they are;
/// otherwise the request states the parameter set, and each entry either carries a new
/// value or asks (`UsePreviousValue`) for the one the stack is already using. "If you
/// specify true, do not specify a parameter value" — so a request doing both is refused,
/// and so is one asking for a previous value the stack does not have.
///
/// Shared by `UpdateStack` and `CreateChangeSet`, which resolve the member the same way:
/// a change set carries the parameters the update it describes would apply.
pub fn merged_parameters(
    previous: &[StoredParameter],
    requested: Option<&[crate::ops::types::Parameter]>,
) -> Result<Vec<StoredParameter>, String> {
    let Some(requested) = requested else {
        return Ok(previous.to_vec());
    };
    requested
        .iter()
        .map(|parameter| {
            let key = parameter
                .parameter_key
                .clone()
                .filter(|key| !key.is_empty())
                .ok_or_else(|| {
                    "Parameters: every parameter must have a ParameterKey".to_string()
                })?;
            if parameter.use_previous_value == Some(true) {
                if parameter
                    .parameter_value
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                {
                    return Err(use_previous_value_with_a_value(&key));
                }
                let value = previous
                    .iter()
                    .find(|stored| stored.key == key)
                    .map(|stored| stored.value.clone())
                    .ok_or_else(|| {
                        format!("Parameters: [{key}] has no previous value on this stack")
                    })?;
                return Ok(StoredParameter {
                    key,
                    value,
                    no_echo: false,
                    // Re-read where the deployment runs, against the store as it stands
                    // then ([`ssm_parameters::resolve`]).
                    resolved_value: None,
                });
            }
            let value = parameter
                .parameter_value
                .clone()
                .ok_or_else(|| format!("Parameters: [{key}] must have values"))?;
            Ok(StoredParameter {
                key,
                value,
                // Whether the template masks the parameter is a property of the TEMPLATE,
                // which this resolution does not read: the caller records it with
                // [`record_no_echo`] (or resolves through [`parameters_in_force`], which
                // does) once the body the parameters belong to is known. Which parameters
                // address a Parameter Store key is a property of the template too, and is
                // resolved the same way ([`ssm_parameters::resolve`]).
                no_echo: false,
                resolved_value: None,
            })
        })
        .collect()
}

/// The message CloudFormation answers a parameter entry that asks for the stack's previous
/// value AND supplies one of its own: `Parameter$UsePreviousValue` is "during a stack
/// update, use the existing parameter value that the stack is using for a given parameter
/// key. If you specify true, do not specify a parameter value", so the two together state
/// two different values for one key and the request is refused.
///
/// The sentence states the rule it enforces — a value is only in conflict with
/// `UsePreviousValue` when it is NON-EMPTY, which is what [`merged_parameters`] holds a
/// parameter to — and its one variable part is the `ParameterKey` the entry carried,
/// spelled as the request spelled it.
pub fn use_previous_value_with_a_value(parameter_key: &str) -> String {
    format!(
        "Invalid input for parameter key {parameter_key}. Cannot specify usePreviousValue as \
         true and non empty value for a parameter"
    )
}

/// `CreateStackInput$StackName`'s documented rule: "A stack name can contain only
/// alphanumeric characters (case sensitive) and hyphens. It must start with an
/// alphabetical character and can't be longer than 128 characters." The model carries no
/// `@pattern` or `@length` for `StackName`, so the handler is what enforces it.
pub const STACK_NAME_PATTERN: &str = "[a-zA-Z][-a-zA-Z0-9]*";
pub const STACK_NAME_MAX_LEN: usize = 128;

/// `Err(message)` naming the member and the constraint the name fails, in the form the
/// service's own input validation words it.
///
/// Shared by `CreateStack` and `CreateChangeSet`, which both name a stack that does not
/// exist yet and so both have to hold its name to the same rule.
pub fn validate_stack_name(name: &str) -> Result<(), String> {
    validate_name("stackName", name)
}

/// The same rule for a STACK SET's name, which `CreateStackSetInput$StackSetName` documents
/// in the very same words ("A stack name can contain only alphanumeric characters
/// (case-sensitive) and hyphens. It must start with an alphabetic character and can't be
/// longer than 128 characters"), reported against its own member.
pub fn validate_stack_set_name(name: &str) -> Result<(), String> {
    validate_name("stackSetName", name)
}

/// The one implementation of CloudFormation's documented name rule, reported against the
/// `member` the request named it in: a name no longer than [`STACK_NAME_MAX_LEN`] that
/// starts with an ASCII letter and carries only ASCII letters, digits and hyphens.
fn validate_name(member: &str, name: &str) -> Result<(), String> {
    if name.len() > STACK_NAME_MAX_LEN {
        return Err(format!(
            "1 validation error detected: Value '{name}' at '{member}' failed to satisfy \
             constraint: Member must have length less than or equal to {STACK_NAME_MAX_LEN}"
        ));
    }
    let well_formed = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !well_formed {
        return Err(format!(
            "1 validation error detected: Value '{name}' at '{member}' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {STACK_NAME_PATTERN}"
        ));
    }
    Ok(())
}

/// The parameter values in force on a stack: what the request supplied, plus the
/// template's own `Default` for every parameter the template declares and the request did
/// not pass a value for.
///
/// `Parameter$Default` is what CloudFormation uses when a stack operation passes no value
/// for a declared parameter, so the stack reports the parameter either way — the declared
/// defaults first, then any value supplied for a parameter the template does not declare
/// (which an operation submitting the body for DEPLOYMENT refuses outright, in the one place
/// that rule lives — [`parameter_constraints::set_refusal`]). The declarations come from
/// [`template::parse`], the one reader of a stack's template body, which is also where the
/// documented per-template parameter quota is enforced.
///
/// Linear in the supplied count: the supplied values are indexed once rather than
/// rescanned per declaration, so two caller-controlled counts cannot multiply.
pub fn parameters_in_force(
    template_body: &str,
    supplied: Vec<StoredParameter>,
) -> Vec<StoredParameter> {
    let template = template::parse(template_body).unwrap_or_default();
    let declared = template.parameter_defaults();
    let supplied_by_key: ::std::collections::HashMap<&str, &StoredParameter> = supplied
        .iter()
        .map(|parameter| (parameter.key.as_str(), parameter))
        .collect();
    let mut in_force: Vec<StoredParameter> = Vec::new();
    let mut keys: ::std::collections::HashSet<String> = ::std::collections::HashSet::new();
    for (key, default) in &declared {
        let value = match supplied_by_key.get(key.as_str()) {
            Some(parameter) => parameter.value.clone(),
            None => default.clone(),
        };
        if keys.insert(key.clone()) {
            in_force.push(StoredParameter {
                key: key.clone(),
                value,
                no_echo: false,
                resolved_value: None,
            });
        }
    }
    for parameter in supplied {
        if keys.insert(parameter.key.clone()) {
            in_force.push(parameter);
        }
    }
    // The template this set was resolved against is the one that says which of its
    // parameters are masked, so the flag is recorded here, off the parse this function
    // already did.
    mark_no_echo(&template, &mut in_force);
    in_force
}

/// Record, on each stored parameter, whether `template_body` declares it `NoEcho`.
///
/// For a caller that resolves a parameter set WITHOUT the template's declared defaults: the
/// masking rule is a property of the declaration, so every stored set that a template body
/// governs goes through this or through [`parameters_in_force`] (which is the path every
/// deploying operation and `CreateChangeSet` take). A body this service cannot read declares
/// nothing, which leaves every parameter unmasked rather than guessing.
pub fn record_no_echo(template_body: &str, parameters: &mut [StoredParameter]) {
    let template = template::parse(template_body).unwrap_or_default();
    mark_no_echo(&template, parameters);
}

/// Copy the parsed template's `NoEcho` declarations onto the parameters it governs.
///
/// A parameter the template does not declare at all carries no declaration, so it is not
/// masked: the mask is what a DECLARATION asks for.
fn mark_no_echo(template: &template::Template, parameters: &mut [StoredParameter]) {
    for parameter in parameters {
        parameter.no_echo = template
            .parameters
            .iter()
            .any(|declared| declared.key == parameter.key && declared.no_echo);
    }
}

/// The region the request was signed for, or [`DEFAULT_REGION`] for an unsigned one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The message CloudFormation answers when a request names a change set that is not
/// addressable: no change set of the named stack carries that name and no change set has
/// that ARN.
pub fn change_set_does_not_exist(name_or_id: &str) -> String {
    format!("ChangeSet [{name_or_id}] does not exist")
}

/// The `InvalidChangeSetStatusException` message CloudFormation answers a `DeleteChangeSet`
/// for a change set whose plan it will not discard — one it is still computing, or one it is
/// applying to the stack right now.
///
/// The execution status the change set is in is the message's ONLY variable part: the change
/// set this refusal is about is the one the request named, so the sentence does not name it
/// again.
pub fn cannot_delete_change_set(execution_status: &str) -> String {
    format!("Cannot delete ChangeSet in execution status {execution_status}")
}

/// The `InvalidChangeSetStatusException` message CloudFormation answers an
/// `ExecuteChangeSet` whose change set has already been executed — whose EXECUTION status,
/// not whose change-set status, is what refuses the call ([`StoredChangeSet::execution_began`]).
///
/// The two variable parts are the change set's own ARN and the `ExecutionStatus` it is in;
/// [`change_set_not_executable_in_status`] is the companion template for a change set whose
/// CREATION never put it in a state anything could execute.
pub fn change_set_not_executable_in_execution_status(
    change_set_id: &str,
    execution_status: &str,
) -> String {
    format!(
        "ChangeSet [{change_set_id}] cannot be executed in its current execution status of \
         [{execution_status}]"
    )
}

/// The `InvalidChangeSetStatusException` message CloudFormation answers an
/// `ExecuteChangeSet` whose change set's own `ChangeSetStatus` — its creation's outcome —
/// leaves nothing to execute ("the change set status might be CREATE_IN_PROGRESS").
///
/// The two variable parts are the change set's own ARN and the `ChangeSetStatus` it is in.
pub fn change_set_not_executable_in_status(change_set_id: &str, status: &str) -> String {
    format!("ChangeSet [{change_set_id}] cannot be executed in its current status of [{status}]")
}

/// The message CloudFormation answers when a request would act on a stack whose current
/// status does not allow it — an operation still in flight on it, or a stack a `CREATE`
/// change set created that nothing has executed yet.
///
/// Shared by `UpdateStack` and `CreateChangeSet`, which refuse the same states, so the two
/// cannot drift apart.
pub fn stack_cannot_be_updated(stack_id: &str, status: &str) -> String {
    format!("Stack:{stack_id} is in {status} state and can not be updated.")
}

/// The message CloudFormation answers a DELETION of a stack whose current status does not
/// allow one ([`status_takes_a_delete`]).
///
/// A DELETION is refused in its OWN words rather than in the frame an UPDATE of a busy stack
/// is refused with ([`stack_cannot_be_updated`]): the deletion's sentence brackets the stack
/// and states the status as the condition the delete cannot be carried out IN. The two
/// variable parts are read off the stack the request resolved — its id and the status it is
/// in — and no part of the sentence comes from any one run.
pub fn stack_cannot_be_deleted(stack_id: &str, status: &str) -> String {
    format!("Stack [{stack_id}] cannot be deleted while in status {status}")
}

/// Whether a stack whose last operation left it in `status` can take a DELETE, or whether the
/// request is the refusal [`stack_cannot_be_deleted`] words.
///
/// CloudFormation runs one operation at a time against a stack, and an UPDATE is the
/// operation a deletion cannot run beside: it is changing, replacing and removing the very
/// resources a deletion would dispose of, so a delete accepted while it runs would race it
/// and leave a stack whose resources neither operation accounts for. The three statuses an
/// update holds the stack in — the update itself, the rollback of one that failed, and the
/// cleanup of the resources a completed update replaced — therefore refuse it, and the
/// caller waits for that update or cancels it (`CancelUpdateStack`).
///
/// Every other status takes a delete, including the create still in flight and the
/// review-only stack a `CREATE` change set brought into existence, for both of which
/// deleting the stack is how a client clears it up.
pub fn status_takes_a_delete(status: &str) -> bool {
    !matches!(
        status,
        status::UPDATE_IN_PROGRESS
            | status::UPDATE_ROLLBACK_IN_PROGRESS
            | status::UPDATE_COMPLETE_CLEANUP_IN_PROGRESS
    )
}

/// Whether a stack whose last operation left it in `status` can take an UPDATE at all, or
/// whether the request is the refusal [`stack_cannot_be_updated`] words.
///
/// Shared by `UpdateStack` and by the UPDATE change sets `CreateChangeSet` plans, which is
/// an update of the stack as it stands: the statuses that take the plan are the statuses
/// that take the update, so the two cannot drift apart.
///
/// An update changes the configuration the stack HOLDS, so it is accepted from the statuses
/// a stack's last operation COMPLETED in — and from the two failed statuses an operation
/// that PRESERVED what it had provisioned leaves (`UpdateStackInput$DisableRollback`:
/// "preserve the state of previously provisioned resources when an operation fails"), where
/// correcting the template and asking again is what preserving them is for.
///
/// Every other status is a stack whose own failure another operation has to resolve first,
/// so an update of it would change nothing a client could rely on: a deletion or a rollback
/// that could not be carried out (retried, or continued, as its own operation), a create
/// that was rolled back, and a stack a `CREATE` change set brought into existence, which
/// holds no template for an update to change. A stack with an operation still in flight is
/// refused by the same rule, read off the transition it owes rather than off the status.
pub fn status_takes_an_update(status: &str) -> bool {
    matches!(
        status,
        status::CREATE_COMPLETE
            | status::UPDATE_COMPLETE
            | status::UPDATE_ROLLBACK_COMPLETE
            | status::CREATE_FAILED
            | status::UPDATE_FAILED
            | status::IMPORT_COMPLETE
    )
}

/// Whether `status` is one a FAILED operation that preserved what it had provisioned left
/// the stack in — the two statuses `status_takes_an_update` admits beside the completed
/// ones (`UpdateStackInput$DisableRollback` / `CreateStackInput$OnFailure=DO_NOTHING`:
/// "preserve the state of previously provisioned resources when an operation fails").
///
/// A stack in one of them holds resources whose configuration is NOT the one its template
/// declares, which is what a rule of `UpdateStack` turns on: resubmitting the configuration
/// the failed operation could not apply is not a no-op — there is outstanding work, which is
/// what makes it a retry.
pub fn status_preserved_a_failure(status: &str) -> bool {
    matches!(status, status::CREATE_FAILED | status::UPDATE_FAILED)
}

/// The message CloudFormation answers an operation on a stack whose own last operation FAILED
/// and was left standing, where the request does not ask for that state to go on being
/// preserved.
///
/// Such a status is not a terminal one: the stack is mid-change, with resources that do not
/// carry the configuration its template declares. The sentence names the status the stack is
/// in and the two ways out of it — the member that asks for the failed state to be kept
/// while the template is corrected (`DisableRollback`), and the call that puts the stack
/// back to its last stable state (`RollbackStack`) — and it is the status alone that varies.
/// The last clause carries no full stop, as CloudFormation words it.
pub fn stack_is_in_a_non_terminal_state(status: &str) -> String {
    format!(
        "This stack is currently in a non-terminal [{status}] state. To update the stack from \
         this state, please use the disable-rollback parameter with update-stack API. To \
         rollback to the last known good state, use the rollback-stack API"
    )
}

/// The statuses a stack can be ROLLED BACK from on request, in the order `RollbackStack`'s
/// own refusal states them ([`stack_cannot_be_rolled_back`]).
///
/// `RollbackStack` "rolls back the specified stack to the last known stable state from
/// CREATE_FAILED or UPDATE_FAILED stack statuses": those are the two statuses of an
/// operation that failed and PRESERVED what it had provisioned
/// (`DisableRollback`/`OnFailure=DO_NOTHING`), which is the state this call exists to clear
/// up. Every other status is a stack with nothing of that kind outstanding — one already
/// stable, one whose own rollback or deletion another operation resolves, or one with an
/// operation still in flight.
///
/// ONE list, so the gate and the sentence that states it cannot drift apart.
pub const ROLLBACK_STATUSES: [&str; 2] = [status::UPDATE_FAILED, status::CREATE_FAILED];

/// Whether a stack whose last operation left it in `status` can be ROLLED BACK on request,
/// or whether the call is the refusal [`stack_cannot_be_rolled_back`] words — the statuses
/// [`ROLLBACK_STATUSES`] names.
pub fn status_takes_a_rollback(status: &str) -> bool {
    ROLLBACK_STATUSES.contains(&status)
}

/// The message CloudFormation answers when `RollbackStack` names a stack whose current
/// status is not one a rollback can be started from.
///
/// Entirely FIXED text: the sentence states which statuses the call CAN be made from
/// ([`ROLLBACK_STATUSES`]) and carries no part of the stack it was made against — neither
/// its id nor the status it is really in, both of which are this run's own state and are
/// what `DescribeStacks` reports.
pub fn stack_cannot_be_rolled_back() -> String {
    format!(
        "RollbackStack can only be called on any of the [{}] statuses.",
        ROLLBACK_STATUSES.join(", ")
    )
}

/// The operation the CALLER invokes to continue a rollback, and the name CloudFormation's
/// own control plane dispatches it as.
const CONTINUE_UPDATE_ROLLBACK_CALL: &str = "ContinueUpdateRollback";

/// The statuses in which a stack's FAILED UPDATE is still awaiting its rollback: the one a
/// continuation is served from, where the rollback ran and failed, and the one where the
/// failed update's rollback has not been performed at all.
///
/// These are the statuses CloudFormation's update-rollback machinery takes the call for; a
/// stack whose rollback is already running, already finished, or was never owed one has
/// nothing for a continuation to resume, and the call never reaches that machinery
/// ([`cannot_continue_update_rollback`]).
pub const UPDATE_ROLLBACK_RECOVERY_STATUSES: [&str; 2] =
    [status::UPDATE_ROLLBACK_FAILED, status::UPDATE_FAILED];

/// The sentence the control plane's own status gate answers for the internal operation it
/// dispatches `ContinueUpdateRollback` as: entirely FIXED text, naming that operation and
/// saying the stack's current status is not one the call can be made from. Neither the
/// status nor the stack reaches any part of it — the same shape
/// `crate::ops::cancel_update_stack::CANNOT_CANCEL_UPDATE_STACK` has for the call that
/// cancels an update.
pub const CANNOT_CONTINUE_UPDATE_ROLLBACK: &str =
    "RollbackUpdatedStack cannot be called from current stack status";

/// The message CloudFormation answers when `ContinueUpdateRollback` names a stack whose
/// current status is not one a rollback can be continued from.
///
/// WHICH sentence that is depends on which gate refused the call, and the two gates word
/// their refusals differently:
///
/// * a stack whose failed update is still AWAITING its rollback is one the update-rollback
///   machinery takes the call for ([`UPDATE_ROLLBACK_RECOVERY_STATUSES`]), so what refuses
///   it is this operation's own precondition — continuing is defined only from
///   `UPDATE_ROLLBACK_FAILED`, the one of those statuses where a rollback was attempted —
///   and that refusal names the operation the CALLER invoked and the status the stack is in,
///   which is the status a client recovering a stuck stack branches on;
/// * a stack outside those statuses has no rollback for a continuation to resume, so the
///   control plane's status gate for the internal operation refuses the call before it
///   reaches that machinery, with that gate's invariant sentence
///   ([`CANNOT_CONTINUE_UPDATE_ROLLBACK`]).
///
/// Neither sentence carries the stack's id or the name the request spelled.
pub fn cannot_continue_update_rollback(status: &str) -> String {
    if UPDATE_ROLLBACK_RECOVERY_STATUSES.contains(&status) {
        return format!(
            "{CONTINUE_UPDATE_ROLLBACK_CALL} cannot be called from stack with {status} status."
        );
    }
    CANNOT_CONTINUE_UPDATE_ROLLBACK.to_string()
}

/// `Err(message)` for a request that names a change set by NAME without the stack it
/// belongs to.
///
/// `ChangeSetNameOrId` is "the name or Amazon Resource Name (ARN) of the change set", and
/// every operation that takes one documents the same constraint on its `StackName`: "If you
/// specify the name of a change set, you must also specify the stack name". A bare name
/// with no stack is therefore a bad request, not a change set that does not exist — only an
/// ARN names a change set on its own.
pub fn require_stack_for_change_set_name(
    change_set_name: &str,
    stack_name: Option<&str>,
) -> Result<(), String> {
    let named_by_arn = change_set_name.starts_with("arn:");
    if named_by_arn || stack_name.is_some() {
        return Ok(());
    }
    Err(format!(
        "StackName must be specified if ChangeSetName is not specified as an ARN: \
         [{change_set_name}]"
    ))
}

/// Mint the request id one response carries — the `<RequestId>` element of an awsQuery
/// `<ErrorResponse>` envelope, the counterpart of the `ResponseMetadata/RequestId` a
/// successful response carries.
///
/// Same 8-4-4-4-12 lowercase-hex form and the same per-response freshness as every other
/// identifier this service mints: it names ONE request for correlation, so two responses
/// must be distinguishable, and it is not a secret (nothing authenticates with it).
pub fn new_request_id() -> String {
    new_identifier("request")
}

/// What a message that must state THIS RESPONSE's request id writes in its place.
///
/// A message is worded where the refusal is decided, and the id a response reports is minted
/// where the response is finished ([`crate::service::query_error_envelope`]): a response
/// reports ONE request id, so the message cannot mint a second one of its own. It writes this
/// marker instead, and that layer substitutes every occurrence of it with the id it reports —
/// so the message and the envelope can never disagree, and nothing a request carries decides
/// the value.
pub const REQUEST_ID_PLACEHOLDER: &str = "{request-id}";

/// The verbs a stack operation's resource-failure summary is worded with
/// ([`resource_failure_summary`]): what the operation was doing to the resources it could
/// not carry through.
pub mod failed_action {
    /// A create — the resources a `CreateStack` (or a change set that creates them) could
    /// not bring into existence.
    pub const CREATE: &str = "create";
    /// An update — the resources an `UpdateStack` could not change.
    pub const UPDATE: &str = "update";
    /// A delete — the resources a `DeleteStack` (or a rollback) could not remove.
    pub const DELETE: &str = "delete";
    /// A create the EXPRESS deployment mode carried out, which CloudFormation names in the
    /// summary of the resources it could not create ([`super::create_action`]).
    pub const CREATE_VIA_EXPRESS_MODE: &str = "create via Express Mode";
}

/// The message CloudFormation answers when a request READS a stack that is not addressable:
/// no live stack has that name and no stack has that id.
///
/// ## One refusal per operation, not one sentence per service
///
/// CloudFormation does not word "that stack is not there" once. Each operation answers its
/// OWN fixed template for the class, and the only variable part of any of them is the
/// `StackName` the request named, spelled as the request spelled it — so these forms are
/// kept apart here, one function per template, and an operation is moved between them only
/// on evidence of what it really answers. The forms this service has evidence for:
///
/// * `Stack with id <name> does not exist` — this function, the one the reads of a stack's
///   own record and of its resources answer (`DescribeStacks`, `DescribeStackResources`,
///   `ListStackResources`, `GetTemplate`, `GetTemplateSummary`, `DescribeEvents`,
///   `CreateGeneratedTemplate`).
/// * `Stack [<name>] does not exist` — [`changed_stack_does_not_exist`], which is also
///   `DetectStackResourceDrift`'s, the whole change-set family's and the one the reads of a
///   stack's event and drift LOGS answer.
/// * `Stack '<name>' does not exist` — [`quoted_stack_does_not_exist`].
/// * `Stack <name> does not exist` — [`bare_stack_does_not_exist`].
/// * `Stack: <name> does not exist` — [`labelled_stack_does_not_exist`], which is also
///   `GetStackPolicy`'s and `ContinueUpdateRollback`'s.
pub fn stack_does_not_exist(name_or_id: &str) -> String {
    format!("Stack with id {name_or_id} does not exist")
}

/// The message CloudFormation answers for a stack that is not addressable with the named
/// stack in SQUARE BRACKETS, which is this family's template for `UpdateStack`,
/// `UpdateTerminationProtection`, `RollbackStack`, `DetectStackDrift`,
/// `DetectStackResourceDrift`, the operations that address a stack's CHANGE SETS
/// (`CreateChangeSet`, `ListChangeSets`, `DescribeChangeSet`, `DescribeChangeSetHooks`,
/// `ExecuteChangeSet`) and the reads of a stack's event and drift LOGS
/// (`DescribeStackEvents`, `DescribeStackResourceDrifts`) — see [`stack_does_not_exist`] for
/// the family, of which this is one template among several.
///
/// The bracketed name is the message's only variable part.
pub fn changed_stack_does_not_exist(name_or_id: &str) -> String {
    format!("Stack [{name_or_id}] does not exist")
}

/// The same refusal with the named stack in SINGLE QUOTES, which is this family's template
/// for `DescribeStackResource` ([`stack_does_not_exist`] documents the family).
pub fn quoted_stack_does_not_exist(name_or_id: &str) -> String {
    format!("Stack '{name_or_id}' does not exist")
}

/// The same refusal with the named stack BARE — no quoting, no bracketing — which is this
/// family's template for `SignalResource` ([`stack_does_not_exist`] documents the family).
pub fn bare_stack_does_not_exist(name_or_id: &str) -> String {
    format!("Stack {name_or_id} does not exist")
}

/// The same refusal with the named stack after a COLON, which is this family's template for
/// `SetStackPolicy`, `GetStackPolicy` and `ContinueUpdateRollback`
/// ([`stack_does_not_exist`] documents the family).
pub fn labelled_stack_does_not_exist(name_or_id: &str) -> String {
    format!("Stack: {name_or_id} does not exist")
}

/// The message answered when a stack IS addressable but carries no template: a stack a
/// `CREATE` change set created has none until that change set is executed, and "if the
/// template doesn't exist, a ValidationError is returned".
///
/// Built from what the request named and the status the stack is in, the way
/// [`stack_does_not_exist`] is built from the name — and worded once, so the operations that
/// read a stack's template cannot drift about it.
pub fn template_does_not_exist(name_or_id: &str, status: &str) -> String {
    format!("Template does not exist for stack [{name_or_id}] in {status} state")
}

/// The message answered when a STACK SET is addressable but carries no template.
///
/// The stack wording ([`template_does_not_exist`]) names a stack and interpolates a stack's
/// own status, neither of which holds for a request that named a `StackSetName`: a StackSet
/// carries a `StackSetStatus`, so the refusal names the StackSet the request addressed and
/// nothing else. Worded once, so every operation that reads a StackSet's template cannot
/// drift about it.
pub fn stack_set_template_does_not_exist(name_or_id: &str) -> String {
    format!("Template does not exist for StackSet [{name_or_id}]")
}

/// The `StackSetNotFoundException` message CloudFormation answers when a request names a
/// stack set this administrator account and Region does not hold — an unknown name, an
/// unknown id, or a stack set another Region holds.
///
/// Built from what the request named, the way [`stack_does_not_exist`] is, and within the
/// modelled `ErrorMessage` length (1..255) for any `StackSetName` a caller can send.
pub fn stack_set_does_not_exist(name_or_id: &str) -> String {
    format!("StackSet {name_or_id} not found")
}

/// The `ValidationError` message CloudFormation answers a request for a stack set's
/// AUTOMATIC-DEPLOYMENT targets with when the stack set it resolves is not service-managed.
///
/// Automatic deployment is deployment through Organizations, which is what the
/// service-managed permission model IS ("StackSets automatically creates the IAM roles
/// required to deploy to accounts managed by Organizations"), so a stack set under any other
/// permission model has no such targets to report at all — a different answer from a
/// service-managed stack set that has none YET. Entirely fixed text: no part of it is taken
/// from the request.
pub const AUTO_DEPLOYMENT_IS_SERVICE_MANAGED_ONLY: &str =
    "Auto-deployment targets are only available for service-managed stack sets";

/// The `OperationNotFoundException` message CloudFormation answers when a stack set holds no
/// operation under the `OperationId` a request named.
///
/// The text carries NO variable part, the way [`drift_operation_does_not_exist`] carries
/// none: the refusal names the KIND of record the request did not resolve, and the operation
/// id and the stack set name are the ones the caller already sent. Worded once, so the
/// operation-scoped stack set reads and the stop cannot drift about it.
pub fn stack_set_operation_does_not_exist() -> &'static str {
    "Operation not found"
}

/// The message CloudFormation answers when a request for a stack instance's RESOURCE
/// DRIFTS names an operation that is not a drift detection of that stack set.
///
/// `ListStackInstanceResourceDriftsInput$OperationId` is "the unique ID of the drift
/// operation", so what does not exist is a DRIFT operation under that id — an id the stack
/// set holds no operation for, and an id that names an operation of another action. The
/// text carries no variable part.
pub fn drift_operation_does_not_exist() -> &'static str {
    "Detect Drift Operation not found"
}

/// The `StackInstanceNotFoundException` message CloudFormation answers an operation that
/// READS one stack instance, addressed by the account and Region a request named, and finds
/// the stack set holds none.
///
/// The text carries no variable part, the way [`drift_operation_does_not_exist`] carries
/// none: the error names the KIND of record the request did not resolve, and the account and
/// Region the caller asked for are the ones it already sent. The wording belongs to the
/// reading operations only — an operation that UPDATES the instances a `Regions` list selects
/// words the same exception its own way, see [`no_stack_instance_in_region`].
pub fn stack_instance_does_not_exist() -> &'static str {
    "StackInstance not found"
}

/// The `StackInstanceNotFoundException` message CloudFormation answers an operation that
/// UPDATES the stack instances a request's `Regions` list selects when one of those Regions
/// holds no instance of the stack set.
///
/// An update is addressed at a SELECTION — a `Regions` list crossed with the accounts or
/// deployment targets — rather than at one instance, so the refusal names which Region of
/// that list resolved nothing: fixed text plus ONE variable part, the Region as the request
/// spelled it. A different sentence from [`stack_instance_does_not_exist`] because the
/// message belongs to the operation that refused, not to the exception shape alone.
pub fn no_stack_instance_in_region(region: &str) -> String {
    format!("No Stack Instance found in region {region}")
}

/// The refusal for a request that does not name exactly ONE source for the template it
/// submits: "Conditional: You must specify only one of the following parameters:
/// TemplateBody, TemplateURL, or set the UsePreviousTemplate to true."
///
/// The ONE wording of that condition, shared by every operation whose input carries the
/// three members (`UpdateStack`, `CreateChangeSet`, `UpdateStackSet`), so the sentence cannot
/// drift between them. It answers the request that names NO source at all; a request that
/// reuses the stack's template AND submits one of its own is refused for that COMBINATION
/// ([`PREVIOUS_AND_SUBMITTED_TEMPLATE`]).
pub const ONE_TEMPLATE_SOURCE: &str =
    "You must specify exactly one of TemplateBody, TemplateURL or UsePreviousTemplate";

/// The refusal for a request that asks to reuse the stack's existing template AND submits a
/// template of its own.
///
/// `UsePreviousTemplate` ("Reuse the existing template that is associated with the stack")
/// and a submitted `TemplateBody`/`TemplateURL` are two different templates to act on, so the
/// request states no one template — a distinct condition from naming no source at all, and
/// the service words it as its own sentence, naming the member in the lower case the service
/// spells it with there. The ONE wording of it, shared by every operation whose input carries
/// the three members, so neither the condition nor its wording is restated per operation.
pub const PREVIOUS_AND_SUBMITTED_TEMPLATE: &str =
    "You cannot specify both usePreviousTemplate and Template Body/Template URL.";

/// `Err(message)` for a request that does not state exactly ONE source for the template it
/// submits, where the input carries all three ways of stating one — the RULE every operation
/// whose input carries that trio is held to, in one place.
///
/// The three conditions are three distinct refusals, each worded for what the request really
/// states: a template submitted BOTH by value and by reference ([`BOTH_TEMPLATE_SOURCES`],
/// the rule a request carrying only that pair is held to), a request that reuses the stack's
/// template while submitting one of its own ([`PREVIOUS_AND_SUBMITTED_TEMPLATE`]), and one
/// that states no source at all ([`ONE_TEMPLATE_SOURCE`]).
pub fn validate_template_sources(
    template_body: Option<&str>,
    template_url: Option<&str>,
    use_previous_template: bool,
) -> Result<(), String> {
    validate_one_template_source(template_body, template_url)?;
    let submitted = template_body.is_some() || template_url.is_some();
    if use_previous_template && submitted {
        return Err(PREVIOUS_AND_SUBMITTED_TEMPLATE.to_string());
    }
    if !use_previous_template && !submitted {
        return Err(ONE_TEMPLATE_SOURCE.to_string());
    }
    Ok(())
}

/// The `StackStatusReason` of a deletion CloudFormation cancelled because it would have
/// removed an exported output value another stack still imports: the export, and the stack
/// that is still consuming it.
///
/// An export in use cannot be removed — deleting the stack that publishes it would break
/// the reference every importing stack resolved — so CloudFormation cancels the deletion
/// and the stack says so, naming both halves of the reference, taken from the stack being
/// deleted and from this store's index, never from any one recorded run.
pub fn export_in_use_delete_canceled_reason(
    export_name: &str,
    importing_stack_name: &str,
) -> String {
    exports::canceled_reason(
        exports::CanceledOperation::Delete,
        export_name,
        importing_stack_name,
    )
}

/// The message answered when a request names a `LogicalResourceId` the stack it selected
/// has not deployed: the stack is addressable, so what does not exist is the resource
/// under that logical id in that stack. Built from what the request named, the way
/// [`stack_does_not_exist`] is.
///
/// The template of the reads that report one resource of a stack (`DescribeStackResource`);
/// a DRIFT check of one resource words the same class its own way
/// ([`resource_does_not_exist_in_stack`]), the way the stack-not-found templates differ per
/// operation.
pub fn resource_does_not_exist(logical_resource_id: &str, stack_name: &str) -> String {
    format!("Resource {logical_resource_id} does not exist for stack {stack_name}")
}

/// The message `DetectStackResourceDrift` answers when the stack it resolved has not
/// deployed the `LogicalResourceId` the request named.
///
/// The fixed text is "does not exist in stack" and the two variable parts are the logical id
/// the request named and the stack's own ID — the stack as the SERVICE addresses it (its
/// ARN), not as the request spelled it, which is what a drift record itself reports a stack
/// by (`StackResourceDrift$StackId`).
pub fn resource_does_not_exist_in_stack(logical_resource_id: &str, stack_id: &str) -> String {
    format!("Resource {logical_resource_id} does not exist in stack {stack_id}")
}

/// The message answered when a request asks for the drift of a resource whose TYPE drift
/// detection is not supported for: "resources that don't currently support drift detection
/// can't be checked", so there is no comparison to answer with and the request is refused.
/// CloudFormation's own sentence for it, naming the `ResourceType` the named resource
/// carries.
pub fn drift_detection_unsupported(resource_type: &str) -> String {
    format!("Drift detection is not supported for ResourceType {resource_type}")
}

/// The message answered when a request names an extension the registry of this account and
/// Region does not hold — an unknown name or ARN, a name registered under another kind or
/// in another Region, a publisher that published no such extension, or a version the
/// record does not carry.
///
/// The sentence CloudFormation answers for `TypeNotFoundException`, with the EXTENSION the
/// request identified quoted in it: its type name, which is what an account-scoped registry
/// ARN carries ([`registry::arn_extension_name`]) as much as the name form does, so the two
/// ways of identifying one extension are refused identically. Anything else the request
/// named is quoted as the request spelled it; nothing else of the request appears in it.
pub fn extension_does_not_exist(identifier: &str) -> String {
    let named = registry::arn_extension_name(identifier)
        .unwrap_or_else(|| identifier.to_string());
    format!("The type '{named}' cannot be found.")
}

/// The message answered when a request names a `StackDriftDetectionId` this account has
/// not run a drift detection under — an id nothing issued, or one whose result this store
/// no longer retains ("the number of drift results CloudFormation retains for any given
/// stack, and for how long, may vary").
///
/// What the request did not resolve is the RESULT of a drift detection, which is what
/// `DescribeStackDriftDetectionStatus` reads, and the detection id the request named is the
/// message's only variable part — bracketed, as the stack-not-found template of the
/// operations that bracket theirs is ([`changed_stack_does_not_exist`]).
pub fn drift_detection_does_not_exist(detection_id: &str) -> String {
    format!("Stack drift detection result [{detection_id}] does not exist")
}

/// The `HookResultNotFoundException` message CloudFormation answers when a request names a
/// Hook invocation result this account and Region does not hold — an id no invocation was
/// recorded under, one another Region holds, or one this store no longer retains.
///
/// A FIXED sentence, unlike the messages above: this refusal states that the result was not
/// found and names nothing of the request, so there is no member to interpolate.
pub const HOOK_RESULT_NOT_FOUND: &str = "The hook result was not found.";

/// The message CloudFormation answers when a request names a `PhysicalResourceId` no
/// stack has deployed: such a request names its stack only through that resource, so what
/// is not addressable is the stack the resource would belong to.
pub fn stack_for_physical_resource_does_not_exist(physical_resource_id: &str) -> String {
    format!("Stack for {physical_resource_id} does not exist")
}

/// The message CloudFormation answers a read of the importers of an export name this account
/// and Region publishes NO export under — a name nothing ever exported, and one whose
/// exporting stack is gone.
///
/// The quoted name is the `ExportName` the request named and the rest is fixed text, the way
/// [`stack_does_not_exist`] is built from what the request named.
pub fn export_does_not_exist(export_name: &str) -> String {
    format!("Export '{export_name}' does not exist.")
}

/// The message CloudFormation answers a read of the importers of an export that EXISTS and
/// no stack imports.
///
/// A different refusal from [`export_does_not_exist`], because the two are what the
/// documented workflow asks apart: "to modify or remove an exported output value, first use
/// this action to see which stacks are using it" is answerable for an export that is there
/// and unused, and unanswerable for a name that is not there at all. The quoted name is the
/// `ExportName` the request named and the rest is fixed text.
pub fn export_is_not_imported(export_name: &str) -> String {
    format!("Export '{export_name}' is not imported by any stack.")
}

/// The reason CloudFormation reports a DEPLOYMENT it refused because the template imports
/// an export name nothing publishes in the account and Region it runs in.
///
/// A different sentence from the two reads above, because it answers a different question:
/// those refuse a request that NAMED the export, while this says why a stack operation
/// could not be carried out — the export the template's `Fn::ImportValue` names was not
/// found, so the declaration carrying it has no value. The name is the one the template
/// imported and the rest is fixed text; a create that is then rolled back reports it with
/// the rollback clause the service appends ([`rollback_requested`]).
pub fn no_export_named(export_name: &str) -> String {
    format!("No export named {export_name} found.")
}

/// `Some(list)` unless the list is empty, which the wire shapes spell as an absent
/// member rather than an empty one.
pub fn non_empty<T>(list: Vec<T>) -> Option<Vec<T>> {
    (!list.is_empty()).then_some(list)
}

/// A stored instant as the modelled timestamp type.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The request's `Tags` as stored tags.
///
/// A tag is caller-authored FREE TEXT, which this service holds as ASCII
/// ([`ascii_text`]): a tag whose key or value carries a codepoint outside ASCII is held,
/// and so read back, with that codepoint replaced by one `?`. Applied HERE, at the one
/// write every operation that takes a `Tags` list goes through, so a stack, a change set
/// and a stack set all hold a tag the same way and a read reports what is stored.
pub fn stored_tags(tags: &[crate::ops::types::Tag]) -> Vec<StoredTag> {
    tags.iter()
        .map(|tag| StoredTag {
            key: ascii_text(&tag.key),
            value: ascii_text(&tag.value),
        })
        .collect()
}

/// Stored tags as the modelled `Tags` member.
///
/// CloudFormation reports `Tags` on every stack, as an empty element for a stack that
/// carries none, so this is always present: a client reading the stack sees the same
/// shape whether or not the stack was tagged.
pub fn tag_views(tags: &[StoredTag]) -> Option<Vec<crate::ops::types::Tag>> {
    Some(
        tags.iter()
            .map(|tag| crate::ops::types::Tag {
                key: tag.key.clone(),
                value: tag.value.clone(),
            })
            .collect(),
    )
}

/// The stack's recorded operation as the modelled `LastOperations` member, always
/// present: "Information about the most recent operations performed on this stack", which
/// for this emulator is the one operation it last opened on the stack.
pub fn last_operation_views(
    last_operation: Option<&StoredOperation>,
) -> Option<Vec<crate::ops::types::OperationEntry>> {
    Some(
        last_operation
            .map(|operation| crate::ops::types::OperationEntry {
                operation_type: Some(operation.operation_type.clone()),
                operation_id: Some(operation.operation_id.clone()),
            })
            .into_iter()
            .collect(),
    )
}

/// The string CloudFormation reports in place of a `NoEcho` parameter's value.
///
/// `NoEcho`: "whether to mask the parameter value to prevent it from being displayed in
/// the console, command line tools, or API". AWS documents the masked value as four
/// asterisks, and the mask is a FIXED string — it is not as long as the value it stands
/// for, since its length would be the one thing a mask must not disclose.
pub const NO_ECHO_MASK: &str = "****";

/// Stored parameters as the modelled `Parameters` member. `UsePreviousValue` is a
/// request-time instruction, so it is never reported back; `ResolvedValue` is reported for
/// an SSM-typed parameter, whose own value is the NAME of a Parameter Store parameter
/// ([`ssm_parameters`]) — "Read-only. The value that corresponds to a Systems Manager
/// parameter key" — and absent for every parameter that addresses no such key.
///
/// A parameter the template in force declares `NoEcho` is reported with
/// [`NO_ECHO_MASK`] in place of its value: CloudFormation masks such a parameter wherever
/// it echoes parameters back, so this is the ONE projection of a stored parameter onto the
/// wire and every operation that reports `Parameters` (`DescribeStacks`,
/// `DescribeChangeSet`) goes through it. The stored value is untouched — a `Ref` to the
/// parameter still resolves to what the stack was deployed with.
pub fn parameter_views(
    parameters: &[StoredParameter],
) -> Option<Vec<crate::ops::types::Parameter>> {
    non_empty(
        parameters
            .iter()
            .map(|parameter| crate::ops::types::Parameter {
                parameter_key: Some(parameter.key.clone()),
                parameter_value: Some(if parameter.no_echo {
                    NO_ECHO_MASK.to_string()
                } else {
                    parameter.value.clone()
                }),
                use_previous_value: None,
                // A masked parameter's resolved value is the value itself, so it is masked
                // for the same reason the value is.
                resolved_value: match (parameter.no_echo, &parameter.resolved_value) {
                    (_, None) => None,
                    (true, Some(_)) => Some(NO_ECHO_MASK.to_string()),
                    (false, Some(resolved)) => Some(resolved.clone()),
                },
            })
            .collect(),
    )
}

/// A stack's drift result as the modelled `Stack$DriftInformation`.
///
/// CloudFormation reports `DriftInformation` on every stack — a stack nothing has checked
/// reports `NOT_CHECKED`, which is a result in its own right — so this is always present.
/// `LastCheckTimestamp` is the instant the most recent detection on the stack was
/// INITIATED, and is absent exactly while no detection has been.
pub fn stack_drift_information(
    stack: &StoredStack,
) -> Option<crate::ops::types::StackDriftInformation> {
    Some(crate::ops::types::StackDriftInformation {
        stack_drift_status: stack.drift_status.clone(),
        last_check_timestamp: stack.drift_last_check.map(timestamp),
    })
}

/// The same result as `StackSummary$DriftInformation`, the SUMMARY spelling of the same
/// two members — so `ListStacks` and `DescribeStacks` cannot disagree about one stack's
/// drift result.
pub fn stack_drift_information_summary(
    stack: &StoredStack,
) -> Option<crate::ops::types::StackDriftInformationSummary> {
    Some(crate::ops::types::StackDriftInformationSummary {
        stack_drift_status: stack.drift_status.clone(),
        last_check_timestamp: stack.drift_last_check.map(timestamp),
    })
}

/// One resource's drift result as the modelled `StackResource$DriftInformation` /
/// `StackResourceDetail$DriftInformation`, reported on every resource for the same reason.
pub fn resource_drift_information(
    deployed: &DeployedResource,
) -> Option<crate::ops::types::StackResourceDriftInformation> {
    Some(crate::ops::types::StackResourceDriftInformation {
        stack_resource_drift_status: deployed.drift_status.clone(),
        last_check_timestamp: deployed.drift_checked_at.map(timestamp),
    })
}

/// The same result as `StackResourceSummary$DriftInformation`, the SUMMARY spelling of the
/// same two members.
pub fn resource_drift_information_summary(
    deployed: &DeployedResource,
) -> Option<crate::ops::types::StackResourceDriftInformationSummary> {
    Some(crate::ops::types::StackResourceDriftInformationSummary {
        stack_resource_drift_status: deployed.drift_status.clone(),
        last_check_timestamp: deployed.drift_checked_at.map(timestamp),
    })
}

/// One resource's drift result as the modelled `StackResourceDrift` — the shape
/// `DescribeStackResourceDrifts` and `DetectStackResourceDrift` both report a checked
/// resource through.
///
/// `None` for a resource no drift check has been carried out over: "resources that haven't
/// yet been checked for drift aren't included", and neither are the ones "that don't
/// currently support drift detection", which are exactly the resources
/// [`DeployedResource::drift_detail`] is absent for.
///
/// `Timestamp` is "time at which CloudFormation performed drift detection on the stack
/// resource" — the instant of the check this record came from. The two property structures
/// and the differences are the comparison that check carried out, so what is reported is
/// what was compared rather than a comparison recomputed on a read; `PropertyDifferences`
/// is present only where there are differences, which is "only for resources whose
/// StackResourceDriftStatus is MODIFIED".
///
/// `PhysicalResourceIdContext` and `ModuleInfo` are absent: context pairs identify a
/// resource "in cases where a resource's logical and physical IDs aren't enough", which the
/// one physical id per logical resource this emulator records always is, and module
/// information belongs to a resource "created from a module included in the stack
/// template", which nothing here deploys.
pub fn stack_resource_drift_view(
    stack_id: &str,
    deployed: &DeployedResource,
) -> Option<crate::ops::types::StackResourceDrift> {
    deployed.drift_detail.as_ref()?;
    let checked_at = deployed.drift_checked_at?;
    Some(resource_drift_record(stack_id, deployed, checked_at))
}

/// The same record for a resource a check was just carried out over, WHATEVER that check
/// found — including a resource of a type no configuration is read for here, which carries
/// the `NOT_CHECKED` status and so neither property structure and no differences
/// ([`resource_drift_status::NOT_CHECKED`]: "any resources that do not currently support
/// drift detection have a status of NOT_CHECKED").
///
/// It is the same construction [`stack_resource_drift_view`] reports, held in one place so
/// the operation that checks ONE resource and the read that lists a stack's checked
/// resources cannot report the same comparison differently. `checked_at` is the instant of
/// the check being reported.
pub fn resource_drift_record(
    stack_id: &str,
    deployed: &DeployedResource,
    checked_at: SystemTime,
) -> crate::ops::types::StackResourceDrift {
    let detail = deployed.drift_detail.clone().unwrap_or_default();
    crate::ops::types::StackResourceDrift {
        stack_id: stack_id.to_string(),
        logical_resource_id: deployed.resource.logical_id.clone(),
        physical_resource_id: physical_resource_id(&deployed.resource),
        physical_resource_id_context: None,
        resource_type: deployed.resource.resource_type.clone(),
        expected_properties: detail.expected_properties,
        actual_properties: detail.actual_properties,
        property_differences: non_empty(
            detail
                .differences
                .into_iter()
                .map(|difference| crate::ops::types::PropertyDifference {
                    property_path: difference.property_path,
                    expected_value: difference.expected_value,
                    actual_value: difference.actual_value,
                    difference_type: difference.difference_type,
                })
                .collect(),
        ),
        stack_resource_drift_status: deployed.drift_status.clone(),
        timestamp: timestamp(checked_at),
        module_info: None,
        drift_status_reason: detail.reason,
    }
}

/// One JSON scalar as the single text an API member takes it as, or `None` for a
/// composite.
///
/// The three scalars a template property can carry each have exactly one textual
/// spelling, and that spelling is what a stack's provisioning sends and what a drift
/// comparison reads back ([`drift`]), so both sides are held to this one function.
pub fn scalar_text(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudformationCloudformationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudformationCloudformationState>)
}

/// One stack's rollback work, taken off the store for the operation that is about to
/// perform it ([`ComAmazonawsCloudformationCloudformationState::take_due_rollbacks`]).
#[derive(Debug, Clone)]
pub struct DueRollback {
    /// The Region the stack's resources live in, which is the credential scope every call
    /// the rollback makes carries.
    pub region: String,
    pub stack_id: String,
    /// The stack's identity and tags, which a resource the rollback re-declares is created
    /// carrying ([`provisioning::StackProvenance`]).
    pub provenance: provisioning::StackProvenance,
    /// The service role in force on the stack, whose permissions decide the calls this
    /// rollback makes in the services that own its resources ([`service_role`]).
    pub role_arn: Option<String>,
    /// What this rollback does with the `DeletionPolicy` of a resource the failed operation
    /// had created, which the revert removes again
    /// ([`StoredStack::rolled_back_resource_policies`]).
    pub policies: provisioning::Policies,
    pub rollback: OwedRollback,
}

/// Perform the rollbacks the stacks of this store owe the services that own their
/// resources, and record what those services answered.
///
/// An update held at a wait condition is failed by the CLOCK when its wait runs out
/// ([`StoredStack::settle_wait_conditions`]), and CloudFormation rolls such an update back
/// to the previous stack configuration exactly as it rolls back one an owning service
/// refused. The rollback's own calls — putting back what the failed update changed there —
/// reach other services' handlers, which the store cannot do under its lock and a stack's
/// clock cannot do at all, so they are made HERE: by the next operation of this service to
/// be called, before it reads or writes the stack.
///
/// This materialises what was already due at the same instant whether or not anybody
/// called: the failure is the clock's, and the rollback is what that failure owes. It is
/// therefore correct for a REPORTING operation to drive it — the status a later caller reads
/// is the one the clock had already made due — and it is what keeps a stack from reporting a
/// rollback as complete that nothing carried out: [`StoredStack::settle`] holds the
/// rollback's terminal status back until the work recorded here is done.
///
/// Call it with the handler's `ctx` and `&*self.engine`, holding no store lock.
pub async fn settle_due_rollbacks<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The failures the clock has made due, and the rollback work each one owes — taken
    // under the lock, performed with it RELEASED.
    let due: Vec<DueRollback> = {
        let mut store = state.lock();
        store.settle(SystemTime::now());
        // ...and how this emulator is reached, so a provider answer that lands out of band
        // can drive this same work itself ([`provider_driver`]).
        store.remember_provisioning_reach(ctx);
        store.take_due_rollbacks()
    };
    for owed in due {
        let outstanding = owed.rollback;
        // The rollback's calls are the STACK's, made under its service role: a role denied
        // the call that would put a resource back cannot make it, which is what leaves the
        // stack in `UPDATE_ROLLBACK_FAILED` below.
        let role = service_role::ServiceRole::of(engine, owed.role_arn.as_deref());
        let reverted = service_role::acting_as(
            role,
            provisioning::reconcile(
                ctx,
                &owed.region,
                &owed.provenance,
                outstanding.previous.clone(),
                &outstanding.applied,
                // A rollback puts back a configuration this stack already deployed, so
                // there is nothing for CloudFormation to refuse about it.
                None,
                owed.policies,
            ),
        )
        .await;
        let mut store = state.lock();
        let Some(stack) = store.resolve_mut(&owed.region, &owed.stack_id) else {
            continue;
        };
        // A resource whose owning service accepted the revert without having carried it out
        // — or whose PROVIDER has been sent the request and has not answered — holds this
        // rollback open, exactly as it holds the operation being rolled back open: the
        // rollback is not reported complete while a resource it put back is still coming up,
        // and a wait that runs out fails it ([`provisioning::advance_stabilisations`]).
        //
        // Recorded BEFORE the walk's events are scheduled, as every other deployment does
        // it: a resource still being put back is one the walk's own terminal event for it is
        // GATED behind ([`StoredStack::schedule_resource_events`]), so nothing reports the
        // revert as complete until the answer that decides it has landed.
        stack.record_stabilisations(&reverted.stabilising, SystemTime::now());
        stack.schedule_resource_events(&reverted.transitions);
        stack.record_materialised_resources(reverted.resources);
        // What the rollback left each resource in, which is what a client reads back —
        // and what says which resource a resumed rollback may skip.
        stack.record_rollback_resource_statuses(&reverted.transitions, SystemTime::now());
        if let Some(failure) = reverted.failure {
            // "A stack goes into the UPDATE_ROLLBACK_FAILED state when CloudFormation can't
            // roll back all changes after a failed stack update": a resource the owning
            // service would not put back leaves the stack there — the state
            // `ContinueUpdateRollback` resumes from — with that service's own refusal as
            // the reason, rather than reporting a rollback that did not happen. What those
            // services are still holding is kept, because that is what a resumed rollback
            // has to put back — the plan the rollback was opened with
            // ([`StoredStack::begin_owed_update_rollback`] holds it as the outstanding one
            // for exactly this, so there is nothing to put back on the stack here).
            stack.fail_transition(
                SystemTime::now(),
                status::UPDATE_ROLLBACK_FAILED,
                failure,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`CAPABILITY_AUTO_EXPAND`] names one value of the modelled `Capability` enum, whose
    /// value set codegen carries as `CAPABILITY_VALUES` — so the constant this crate
    /// compares an acknowledgement against cannot drift from the value a caller can
    /// actually send.
    #[test]
    fn the_auto_expand_capability_is_a_modelled_capability_value() {
        let modelled = crate::ops::types::CAPABILITY_VALUES;
        assert!(
            modelled.contains(&CAPABILITY_AUTO_EXPAND),
            "case {CAPABILITY_AUTO_EXPAND:?}: expected the modelled Capability value set to \
             carry it, got {modelled:?}"
        );
    }
}
