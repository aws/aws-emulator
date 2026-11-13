// SPDX-License-Identifier: CC0-1.0

//! One stack, as this emulator persists it: the record, the status transitions it lives
//! through, and the helpers those transitions use.
//!
//! Split out of [`super`] so the store module stays readable: this file holds everything
//! that belongs to ONE stack — its fields, the operations that open and settle on it, the
//! template it deploys and the events and resources those produce — while the module it
//! hangs off holds the store that keeps many of them, the records of the other resources
//! this service has (change sets, drift detections, registrations) and the projections onto
//! the modelled wire shapes. Nothing about the behaviour changes with the move.

use super::*;

/// One stack, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredStack {
    pub stack_id: String,
    pub stack_name: String,
    /// The Region the stack lives in: the one the call that created it was signed for.
    /// Every identifier the stack's resources carry is Regional, so this is what the
    /// template's `AWS::Region` pseudo parameter and every minted ARN resolve to.
    pub region: String,
    /// The template in force on the stack, or `None` for a stack that has none yet: a
    /// `CREATE` change set creates its stack "with a unique stack ID, but no template or
    /// resources", and the template becomes the stack's when the change set is executed.
    pub template_body: Option<String>,
    /// The `Processed` stage of the template this stack carries, computed once when the
    /// body is stored: `GetTemplate(TemplateStage=Processed)` serves it, and the resources
    /// this stack deploys are the ones it declares. Computed over
    /// [`Self::included_template_body`] — the submitted body with any `AWS::Include`
    /// snippets already spliced in — so one stage is the whole processing pipeline's.
    /// `Processed::Same` for a stack with no template, which has no stage to serve either
    /// way.
    pub processed_template: template::Processed,
    /// The template as CloudFormation processed it where processing needed something only
    /// the stack operation could read: the `AWS::Include` snippets its `Fn::Transform`s
    /// name, spliced in ([`include::process`]). `None` when the submitted body needed no
    /// such processing, which is every template that names no such transform.
    ///
    /// This is the body the DECLARED transforms are then applied to, and what the stack
    /// deploys from ([`Self::template_scope`]): a snippet is read once, by the operation
    /// that submits the template, and every later read of the stack sees the processed
    /// template rather than reaching into Amazon S3 again.
    pub processed_template_body: Option<String>,
    /// The template's own `Description`, parsed once when the body is stored rather than
    /// on every read: this is where CloudFormation takes both `Stack$Description` and
    /// `StackSummary$TemplateDescription` from, and a read must not re-parse a
    /// caller-sized body under the store lock.
    pub template_description: Option<String>,
    /// The change set that last acted on this stack (`Stack$ChangeSetId`): the one that
    /// created it in `REVIEW_IN_PROGRESS`, or the one `ExecuteChangeSet` last applied.
    pub change_set_id: Option<String>,
    pub parameters: Vec<StoredParameter>,
    pub capabilities: Vec<String>,
    pub tags: Vec<StoredTag>,
    pub notification_arns: Vec<String>,
    pub role_arn: Option<String>,
    pub timeout_in_minutes: Option<i32>,
    pub disable_rollback: bool,
    pub enable_termination_protection: bool,
    pub retain_except_on_create: Option<bool>,
    pub deployment_config: Option<StoredDeploymentConfig>,
    pub rollback_configuration: Option<StoredRollbackConfiguration>,
    pub deletion_mode: Option<String>,
    /// The stack policy body in force, as `SetStackPolicy` or `CreateStack`/`UpdateStack`
    /// last supplied it.
    ///
    /// A stack policy governs which of a stack's RESOURCES an update may change, and which
    /// action it may take on each: every update evaluates the document in force against the
    /// plan it would carry out ([`stack_policy`]), and takes no action the policy refuses.
    /// `None` for a stack with no policy, which protects nothing.
    pub stack_policy_body: Option<String>,
    /// `ResourceTypes`: the resource types this stack's operations may work with. Stored
    /// as supplied; it constrains which resource types a stack operation may create,
    /// which this service does not refuse on.
    pub resource_types: Vec<String>,
    pub status: String,
    pub status_reason: Option<String>,
    /// The last status this stack SETTLED in that is a stable one
    /// ([`is_stable_status`]), or `None` for a stack that has never completed an operation.
    ///
    /// `RollbackStack` rolls a stack back "to the last known stable state", and "will delete
    /// a stack if it doesn't contain a last known stable state", so this is what decides
    /// which of those a requested rollback is ([`StoredStack::begin_requested_rollback`]).
    /// Written by [`StoredStack::settle`] alone, since that is the one place a terminal
    /// status is installed.
    pub last_stable_status: Option<String>,
    pub pending: Option<PendingStatus>,
    /// The resources the stack's last COMPLETED create or update materialised (see
    /// [`StoredStack::deploy_template`]), each with the status that operation left it in.
    /// A completed DELETE leaves them in place, in `DELETE_COMPLETE`, and so does the
    /// ROLLBACK of a failed create over the resources that create had made or declared
    /// ([`disposed_resources`]): CloudFormation reports a stack's resources for as long as
    /// it reports the stack, and those rows are what say which resource was disposed of.
    pub resources: Vec<DeployedResource>,
    /// The stack's evaluated outputs, in `OutputKey` order, as of that same completed
    /// operation — what `DescribeStacks` reports as `Stack$Outputs`.
    pub outputs: Vec<StoredOutput>,
    /// What the create or update currently in flight will leave the two fields above
    /// with when it completes.
    pub pending_deployment: Option<PendingDeployment>,
    /// The direct dependency relation of the template IN FORCE on the stack, keyed by
    /// logical id ([`template::direct_dependencies`]): what each of the stack's resources
    /// waits for. Installed by the operation that deploys the template, so the stack's own
    /// deletion still knows the order to walk its resources in.
    pub resource_dependencies: BTreeMap<String, BTreeSet<String>>,
    /// The walk the operation in flight made over the stack's resources, with the
    /// per-resource events it still owes ([`timeline`]): a resource's `*_IN_PROGRESS` event
    /// is recorded when that resource was SCHEDULED and its terminal one when it FINISHED,
    /// so the event log of one operation is the timeline of its walk. [`Self::settle`] is
    /// what records the events whose instant the clock has reached; `None` for a stack with
    /// no walk still owing events.
    pub resource_timeline: Option<timeline::Timeline>,
    /// The wait conditions the operation in flight is waiting at, each with the signals
    /// that have arrived ([`wait_conditions`]).
    ///
    /// Non-empty means the operation's completion is NOT owed by the clock yet:
    /// CloudFormation "doesn't proceed with a stack creation or update until resources
    /// receive the required number of signals or the timeout period is exceeded", so
    /// [`Self::settle`] holds the pending transition back while one of these is
    /// unsatisfied, and `SignalResource` is what satisfies one.
    pub wait_conditions: Vec<wait_conditions::WaitConditionBarrier>,
    /// The resources of the operation in flight whose owning service accepted the create
    /// but does not report the resource ready yet, and the ones whose delete it accepted
    /// while still holding them ([`stabilising`]).
    ///
    /// An unreleased barrier means the operation's completion is NOT owed by the clock
    /// yet, for the same reason an unsatisfied wait condition does, and
    /// [`Self::settle`] holds the pending transition back. A barrier held for a resource
    /// being CREATED also reports that resource `*_IN_PROGRESS`, under the identifier its
    /// owning service already assigned; one held for a resource being DELETED reports
    /// nothing, because the stack no longer records that physical resource at all.
    /// [`provisioning::advance_stabilisations`] is what reads the owning service's answer
    /// and releases one.
    pub stabilisations: Vec<stabilising::StabilisationBarrier>,
    /// What the ROLLBACK of the UPDATE in flight would owe the services that own the
    /// stack's resources, if that update fails ([`OwedRollback`]) — computed by the handler
    /// that started it, because only that handler knows what it changed there.
    ///
    /// Held only while an update that can still fail with no call of its own is in flight,
    /// and retired with it: [`Self::begin_transition`] replaces it, [`Self::settle`] drops
    /// it when the operation completes, and [`Self::begin_owed_update_rollback`] is what
    /// turns it into work now owed when the update fails.
    pub rollback_plan: Option<OwedRollback>,
    /// The rollback work this stack owes the owning services RIGHT NOW: the plan above,
    /// promoted by the failure of the update that held it.
    ///
    /// A rollback cannot be reported as complete before it has been carried out, so
    /// [`Self::settle`] holds the rollback's own terminal status back while this is set, and
    /// [`settle_due_rollbacks`] — called by the operations a client drives a stack
    /// through — is what performs it and clears it.
    pub owed_rollback: Option<OwedRollback>,
    /// The stack CONFIGURATION in force before the update in flight applied its own
    /// ([`StackConfiguration`]) — snapshotted by the handler that applies one, because the
    /// members it replaces are written onto the stack when the update starts rather than
    /// staged like its resources.
    ///
    /// A failed update is rolled back "to the previous stack configuration", so this is
    /// what [`Self::begin_update_rollback`] puts back and what a rollback a client asks for
    /// on a stack whose failed state was kept ([`Self::begin_requested_rollback`]) goes back
    /// to. Retired by [`Self::settle`] when the operation holding it settles in a terminal
    /// `*_COMPLETE` status, whose configuration is the stack's from then on; one that settles
    /// in a FAILED status keeps it, since a rollback can still be asked for.
    pub previous_configuration: Option<StackConfiguration>,
    /// The rollback this stack still OWES the services that own its resources, held while it
    /// is in `UPDATE_ROLLBACK_FAILED`: what the previous configuration is, and what those
    /// services are still holding ([`OwedRollback`]).
    ///
    /// `UPDATE_ROLLBACK_FAILED` means a rollback could not be carried out, and both sides of
    /// it are still needed: `ContinueUpdateRollback` declares the previous configuration
    /// again, against what the failed update left in those services. The stack's own records
    /// report what they hold, so a rollback resumed from those records would ask for no
    /// change at all. `None` for a stack with no rollback outstanding; cleared when a
    /// rollback completes.
    pub unreverted_rollback: Option<OwedRollback>,
    /// The operations this stack has lived through, in the order they were opened and
    /// bounded at [`MAX_OPERATIONS_PER_STACK`] (oldest evicted first): each "a discrete
    /// change attempt on the stack", with the span it ran for and how it ended
    /// ([`StoredOperation`]).
    ///
    /// Every event an operation causes carries that operation's id
    /// ([`StoredEvent::operation_id`]), so this is what an event is reported AGAINST, and
    /// the LAST entry is the operation that last acted on the stack —
    /// `Stack$LastOperations` ([`Self::last_operation`]).
    pub operations: ::std::collections::VecDeque<StoredOperation>,
    pub creation_time: SystemTime,
    pub last_updated_time: Option<SystemTime>,
    pub deletion_time: Option<SystemTime>,
    pub client_request_token: Option<String>,
    /// The client request tokens the operations on this stack have been started under, in
    /// the order they claimed one and bounded at
    /// [`MAX_CLIENT_REQUEST_TOKENS_PER_STACK`] (oldest-first). The field above is the
    /// token of the operation in force, which the stack's events carry; this is the whole
    /// set the token-uniqueness rule is checked against
    /// ([`reused_client_request_token`]).
    pub client_request_tokens: ::std::collections::VecDeque<StoredRequestToken>,
    pub events: ::std::collections::VecDeque<StoredEvent>,
    /// The bytes of caller-sized `ResourceProperties` the event log is holding right now,
    /// kept alongside it so [`StoredStack::push_event`] can honour
    /// [`MAX_EVENT_PROPERTIES_BYTES_PER_STACK`] without walking the log.
    pub retained_event_properties_bytes: usize,
    /// The resource signals this stack has been sent (`SignalResource`), in arrival
    /// order, one per `UniqueId` per logical resource and bounded at
    /// [`MAX_SIGNALS_PER_STACK`]. A signal is kept whatever the stack was doing when it
    /// arrived: CloudFormation accepts a signal for any of a stack's resources, and only
    /// a resource whose declaration waits on resource signals
    /// ([`template::waits_for_resource_signals`]) has an operation that acts on one.
    pub signals: ::std::collections::VecDeque<StoredResourceSignal>,
    /// The exported output values this stack's deployed template IMPORTS, keyed by export
    /// name: "the value of an output exported by another stack", resolved once where the
    /// template was deployed. A stack that imports an export is what `ListImports` reports
    /// for that export name, and what makes the exporting stack's export in use.
    pub imports: BTreeMap<String, String>,
    /// The other stacks' output values this stack's deployed template READ with an
    /// `Fn::GetStackOutput`, keyed by the reference that read one: resolved once where the
    /// template was deployed and KEPT here, so the stack goes on reporting the value as of
    /// its own deployment rather than whatever the publishing stack carries now
    /// ([`super::stack_outputs`]).
    pub stack_outputs: super::stack_outputs::Values,
    /// The identifier each resource this stack ADOPTED already existed under, keyed by
    /// logical id ([`resource_import`]): what executing an IMPORT change set took over
    /// ([`resource_import::adopt_identifier`]).
    ///
    /// Remembered on the stack because nothing else can say it: the identity of a resource
    /// CloudFormation did not create is stated nowhere in the template, so every deployment
    /// after the import — which recomputes every other resource's identity from that
    /// template — would otherwise read the adopted resource as a different one and replace
    /// it. An entry is kept only while the deployed template still declares the resource.
    pub adopted: BTreeMap<String, String>,
    /// `Stack$DriftInformation`'s `StackDriftStatus`: the stack-level result the last
    /// drift detection that COMPLETED against this stack left ([`drift`]).
    /// [`DRIFT_STATUS_NOT_CHECKED`] until one has, since "NOT_CHECKED: CloudFormation
    /// hasn't checked if the stack differs from its expected template configuration".
    pub drift_status: String,
    /// `StackDriftInformation$LastCheckTimestamp`: "most recent time when a drift
    /// detection operation was INITIATED on the stack", so it is set by the call that
    /// starts one rather than by the result landing.
    pub drift_last_check: Option<SystemTime>,
}

// ---------------------------------------------------------------------------
// The stack's identity and the template it deploys
// ---------------------------------------------------------------------------

impl StoredStack {
    /// A stack as `CreateStack` first stores it: the identity and the template, with every
    /// optional member at its default. The caller fills in what the request supplied and
    /// then opens the create transition.
    ///
    /// Computes the template's processed stage, so a HANDLER uses
    /// [`Self::new_processed`] instead: the expansion is caller-sized work and must not be
    /// done while the store lock is held.
    pub fn new(
        stack_id: String,
        stack_name: String,
        template_body: Option<String>,
        now: SystemTime,
    ) -> Self {
        let processed_template = template_body
            .as_deref()
            .map(template::process)
            .unwrap_or(template::Processed::Same);
        Self::new_processed(stack_id, stack_name, template_body, processed_template, now)
    }

    /// [`Self::new`] for a caller that computed the template's processed stage already —
    /// off the store lock.
    pub fn new_processed(
        stack_id: String,
        stack_name: String,
        template_body: Option<String>,
        processed_template: template::Processed,
        now: SystemTime,
    ) -> Self {
        Self {
            stack_id,
            stack_name,
            region: DEFAULT_REGION.to_string(),
            template_description: template_body.as_deref().and_then(template::description),
            processed_template,
            template_body,
            processed_template_body: None,
            change_set_id: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            tags: Vec::new(),
            notification_arns: Vec::new(),
            role_arn: None,
            timeout_in_minutes: None,
            disable_rollback: false,
            enable_termination_protection: false,
            retain_except_on_create: None,
            deployment_config: None,
            rollback_configuration: None,
            deletion_mode: None,
            stack_policy_body: None,
            resource_types: Vec::new(),
            status: status::CREATE_IN_PROGRESS.to_string(),
            status_reason: None,
            // Nothing has settled yet, so the stack has no stable state to go back to.
            last_stable_status: None,
            pending: None,
            resources: Vec::new(),
            outputs: Vec::new(),
            pending_deployment: None,
            resource_dependencies: BTreeMap::new(),
            resource_timeline: None,
            wait_conditions: Vec::new(),
            stabilisations: Vec::new(),
            rollback_plan: None,
            owed_rollback: None,
            previous_configuration: None,
            unreverted_rollback: None,
            operations: ::std::collections::VecDeque::new(),
            creation_time: now,
            last_updated_time: None,
            deletion_time: None,
            client_request_token: None,
            client_request_tokens: ::std::collections::VecDeque::new(),
            events: ::std::collections::VecDeque::new(),
            retained_event_properties_bytes: 0,
            signals: ::std::collections::VecDeque::new(),
            imports: BTreeMap::new(),
            stack_outputs: super::stack_outputs::Values::new(),
            adopted: BTreeMap::new(),
            drift_status: DRIFT_STATUS_NOT_CHECKED.to_string(),
            drift_last_check: None,
        }
    }

    /// Replace the stored template body with one whose processed stage was computed
    /// already — off the store lock.
    ///
    /// The only way a body reaches a stored stack: there is deliberately no variant that
    /// computes the stage itself, so no handler can expand a caller-sized template while
    /// holding the store lock.
    pub fn set_template_body(
        &mut self,
        template_body: String,
        processed_template: template::Processed,
    ) {
        self.template_description = template::description(&template_body);
        self.processed_template = processed_template;
        self.template_body = Some(template_body);
        // The spliced body belonged to the body that was there: a new one has not been
        // through that processing yet, and the operation that submitted it is what records
        // it ([`Self::set_processed_template_body`]).
        self.processed_template_body = None;
    }

    /// Record the template as CloudFormation processed it, which is the one this stack
    /// deploys from. Called by the operation that submitted the body, once the snippets its
    /// transforms name have been read ([`include::process`]).
    pub fn set_processed_template_body(&mut self, processed: String) {
        self.processed_template_body = Some(processed);
    }

    /// Record that this deployment deploys NOTHING: its template could not be processed, so
    /// no resource was computed from it and none is asked of any owning service.
    ///
    /// The stack is left owning no resources and no outputs rather than the ones an
    /// unprocessed template would have declared — "CloudFormation processes the template
    /// fragments" first, so a template whose processing failed never reached the stage that
    /// provisions anything.
    pub fn deploy_nothing(&mut self) {
        self.pending_deployment = Some(PendingDeployment::default());
    }

    /// The body the stack's DECLARED transforms are applied to: the submitted body with
    /// the `AWS::Include` snippets the operation read spliced into it, and the submitted
    /// body itself for a template that names no such transform.
    pub fn included_template_body(&self) -> Option<&str> {
        self.processed_template_body
            .as_deref()
            .or(self.template_body.as_deref())
    }

    /// The body `GetTemplate(TemplateStage=Processed)` serves, or `None` when the stack has
    /// no template at all or this service could not process the transforms it declares.
    pub fn processed_stage_body(&self) -> Option<&str> {
        self.processed_template.body(self.included_template_body()?)
    }

    /// Materialise what the stored template declares: one record per declared resource,
    /// with its properties evaluated, the stack's outputs evaluated against those
    /// resources, the stack's parameters and the pseudo parameters, and what the operation
    /// does to each resource.
    ///
    /// What is deployed is the template's PROCESSED stage — the document CloudFormation
    /// deploys — so a transform's expansion is part of what the stack provisions.
    ///
    /// Called by the operation that DEPLOYS the template (`CreateStack`, `UpdateStack`,
    /// `ExecuteChangeSet`), never by a read: the values are a function of the template body
    /// and the parameters in force, so they are computed once where those change and read
    /// back unchanged afterwards. Creating each declared resource in the service that OWNS
    /// it is [`provisioning::reconcile`], which the deploying operation calls with these
    /// records once the store is unlocked.
    ///
    /// The result is held as the operation's PENDING deployment and takes effect when
    /// [`Self::settle`] completes that operation — a stack reports the resources and
    /// outputs of the template that finished deploying, so a caller reading a stack whose
    /// create is still in progress sees none, and one reading mid-update still sees the
    /// values in force rather than the ones the update is about to install.
    ///
    /// `Err(reason)` when this service cannot deploy that template AT ALL: its PROCESSED
    /// stage is one the transform section cannot compute, or its body is one the reader
    /// will not read. Nothing is deployed then, and the caller (the handler that opened the
    /// operation) ends the operation in the service's modelled FAILURE state with that
    /// reason — never in a completion reporting work nothing performed. A stack with no
    /// template at all is not such a case: there is nothing to deploy, which is what a
    /// stack a `CREATE` change set created is for.
    /// `exports` is the exported output values in force in the account and Region this
    /// deployment runs in, keyed by export name: what its template's `Fn::ImportValue`
    /// references resolve against. The deployment keeps the ones it really imported, which
    /// is the stack's entry in the index `ListImports` reports.
    /// `stack_outputs` is the other half of the cross-stack relation: every output value a
    /// template deployed in that Region can read with an `Fn::GetStackOutput`
    /// ([`super::stack_outputs::Values`]). The deployment keeps the ones it really read, so
    /// the stack reports them as of its own deployment.
    pub fn deploy_template(
        &mut self,
        exports: &BTreeMap<String, String>,
        stack_outputs: &super::stack_outputs::Values,
    ) -> Result<(), String> {
        self.deploy_template_adopting(exports, stack_outputs, &BTreeMap::new())
    }

    /// [`Self::deploy_template`] for a deployment that ADOPTS resources that already exist:
    /// `adopting` is the identifier each of them exists under, keyed by logical id
    /// ([`resource_import::adopted_identifiers`]).
    ///
    /// Such a resource is not created, so the identity in force for it is the one it already
    /// carries rather than the one the naming rules computed
    /// ([`resource_import::adopt_identifier`]) — which is what makes the deployment reconcile
    /// against the resource that is really there instead of creating a second one, and what
    /// the stack's `Ref` to it and the outputs built from it report. The stack REMEMBERS it
    /// ([`Self::adopted`]), so every later deployment of the same declaration keeps that
    /// identity instead of recomputing one for a resource CloudFormation never created.
    pub fn deploy_template_adopting(
        &mut self,
        exports: &BTreeMap<String, String>,
        stack_output_values: &super::stack_outputs::Values,
        adopting: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        for (logical_id, identifier) in adopting {
            self.adopted
                .insert(logical_id.clone(), identifier.clone());
        }
        if self.template_body.is_none() {
            // No template to deploy: the stack a CREATE change set created, which carries
            // no resources until that change set is executed.
            self.pending_deployment = Some(PendingDeployment::default());
            return Ok(());
        }
        if self.processed_stage_body().is_none() {
            // The template's PROCESSED stage is unavailable, so what it declares is
            // UNKNOWN rather than empty: nothing is deployed, and the resources and
            // outputs in force are left alone.
            let reason = self
                .processed_template
                .unavailable_reason()
                .unwrap_or("the template's processed stage is unavailable")
                .to_string();
            self.pending_deployment = None;
            return Err(reason);
        }
        let Some((declared, parameters)) = self.template_scope() else {
            // A template this service will not read — a body it cannot parse, or one past a
            // documented per-template quota: the stack is left with no resources and no
            // outputs rather than with invented ones, and the operation fails.
            self.pending_deployment = None;
            return Err(
                "the template is not one this service can read: it is malformed, or past a \
                 documented per-template quota"
                    .to_string(),
            );
        };
        let stack = template::StackScope {
            region: &self.region,
            stack_name: &self.stack_name,
            stack_id: &self.stack_id,
            notification_arns: &self.notification_arns,
        };
        // ONE budget for the whole deployment: the resource names and every output are
        // evaluated out of it, so the total work one request can ask for is bounded.
        let mut budget = template::Budget::new();
        let no_resources = BTreeMap::new();
        let parameter_types = ssm_parameters::parameter_types_in_force(&declared);
        // The deployment's scope with no resource resolved yet, which is what
        // `provision` turns into the stack's resources.
        let scope = template::Scope {
            stack,
            parameters: &parameters,
            parameter_types: &parameter_types,
            resources: &no_resources,
            mappings: &declared.mappings,
            conditions: &declared.conditions,
            imports: exports,
            stack_outputs: stack_output_values,
        };
        let mut resources = template::provision(&declared, &scope, &mut budget);
        let by_logical_id: BTreeMap<String, StoredResource> = resources
            .iter()
            .map(|resource| (resource.logical_id.clone(), resource.clone()))
            .collect();
        // Everything resolved against the whole set, now that every resource's identity
        // is known.
        let resolved = template::Scope {
            resources: &by_logical_id,
            ..scope
        };
        // A second pass: each resource's properties are evaluated against the whole set,
        // so a property naming another resource (a stream's `RoleARN` from an
        // `Fn::GetAtt`) carries that resource's value. This is the request the service
        // that owns the resource is asked to create it with, and the properties blob its
        // events report.
        template::resolve_properties(&declared, &mut resources, &resolved, &mut budget);
        // A resource this stack ALREADY created in the service that owns it, and that this
        // template declares again unchanged, is still that same created resource: the
        // record carries forward both the fact that it exists there and what that service
        // reported about it, so a later deletion still deletes it. Creating what is new
        // and deleting what this deployment replaced is
        // [`provisioning::reconcile`](provisioning::reconcile), which the deploying
        // operation calls with these records.
        let already_created = self.current_resources();
        // The identifier each resource this stack ADOPTED already exists under: the ones this
        // deployment adopts and the ones an earlier one did ([`Self::adopted`]). Read out
        // here so the walk below only reads it.
        let adopted_in_force = self.adopted.clone();
        // Whether any record took over an identifier its owning service MINTED, which is
        // not the one the properties above were resolved against.
        let mut adopted_minted_identity = false;
        for resource in &mut resources {
            // A resource this stack ADOPTED already exists in the service that owns it, under
            // the identifier the plan that adopted it reported: that identifier is the one in
            // force, so the record takes it over before anything is compared against it
            // ([`resource_import::adopt_identifier`]). A declaration that STATES the
            // resource's name decides the identity itself, so an adoption no longer applies
            // to it — such a declaration asks for the resource that name belongs to.
            let adopted = adopted_in_force
                .get(&resource.logical_id)
                .filter(|_| declared_name(resource).is_none());
            if let Some(identifier) = adopted {
                if resource_import::adopt_identifier(resource, identifier) {
                    adopted_minted_identity = true;
                }
            }
            // Whether the identifier a resource of this type is known by is one this
            // deployment cannot recompute, so the record carries the one in force FORWARD
            // rather than reading the resource as a different one: an identifier assigned
            // ELSEWHERE ([`identity_is_assigned_elsewhere`]), and the identifier of a
            // resource this stack ADOPTED, which CloudFormation did not create and the
            // template therefore states nowhere.
            let keeps_its_identifier = identity_is_assigned_elsewhere(&resource.resource_type)
                || adopted.is_some();
            // The record of the same resource this stack already created. Judged by
            // identity first; for a type whose id the OWNING SERVICE assigns, a record
            // under the same logical id and type IS that resource, because nothing here
            // can recompute the id it was created with — as long as the declaration still
            // asks for the same resource, which is what the type's REPLACEMENT properties
            // say ([`declares_the_same_replacing_properties`]). A declaration that states
            // one of them differently is a different resource (AWS replaces such a resource
            // when the template changes one), and a type that publishes none declares none
            // on either side, so the test is vacuous for it.
            let existing = already_created
                .iter()
                .find(|existing| {
                    existing.materialised && provisioning::is_same_resource(existing, resource)
                })
                .or_else(|| {
                    keeps_its_identifier
                        .then(|| {
                            already_created.iter().find(|existing| {
                                existing.materialised
                                    && existing.logical_id == resource.logical_id
                                    && existing.resource_type == resource.resource_type
                                    && declares_the_same_replacing_properties(existing, resource)
                            })
                        })
                        .flatten()
                });
            if let Some(existing) = existing {
                // Deliberately NOT `provisioning::carry_forward`: that one also carries
                // the properties the resource was created with, and this record's
                // properties are what `reconcile` compares against them to see whether
                // the deployment asks for a DIFFERENT configuration of a resource that
                // already exists. Only the fact that it exists, what the owning service
                // reported about it, and an identity that service MINTED (which no
                // deployment recomputes) carry forward here.
                resource.materialised = true;
                if keeps_its_identifier {
                    // The identifier in force is the one that service minted, or the one an
                    // adopted resource already had, so the record keeps it: an output naming
                    // the resource reports the id a client can call that service with, and
                    // `reconcile` sees the resource the stack already has rather than one to
                    // create again.
                    if existing.physical_resource_id != resource.physical_resource_id {
                        adopted_minted_identity = true;
                    }
                    resource.physical_resource_id = existing.physical_resource_id.clone();
                }
                for (name, value) in &existing.attributes {
                    resource.attributes.insert(name.clone(), value.clone());
                }
            }
        }
        // A property naming a resource whose identity its owning service MINTED was
        // resolved against the stand-in the naming rules recomputed above, not against the
        // identity now in force, so those properties are resolved ONCE MORE against the
        // records as they stand. Only a deployment that really took such an identifier over
        // pays for the second pass; without it a re-deploy of the same template would read
        // as a CHANGED configuration of every resource that names one.
        if adopted_minted_identity {
            let in_force: BTreeMap<String, StoredResource> = resources
                .iter()
                .map(|resource| (resource.logical_id.clone(), resource.clone()))
                .collect();
            template::resolve_properties(
                &declared,
                &mut resources,
                &template::Scope {
                    resources: &in_force,
                    ..scope
                },
                &mut budget,
            );
        }
        // The outputs are evaluated against the identities now in force, which is why this
        // is after the carry-forward above: a resource whose id the owning service assigned
        // carries the one it minted, and an output naming that resource must report THAT.
        let outputs = self.evaluated_outputs(&declared, scope, &resources, &mut budget);
        // The wait conditions the template declares, with the resources whose creation
        // waits on each: computed HERE, where the template is already parsed and the store
        // is not locked, and charged to the SAME budget as every other evaluation of this
        // deployment, so what one request can ask for stays bounded in total.
        let wait_conditions = wait_conditions::declared(&declared, &resources, &mut budget);
        // What each declared resource WAITS FOR, read here for the same reason: the
        // deployment's walk over these resources is ordered by it ([`timeline`]), and a
        // read must not re-parse a caller-sized body under the store lock.
        let dependencies = template::direct_dependencies(&declared);
        // Which exports this template really IMPORTS, resolved against the same scope its
        // resources and outputs were: only those names belong on the stack, and they are
        // what makes each exporting stack's export in use.
        let imported = template::imported_export_names(&declared, &scope, &mut budget);
        let mut unresolved_imports = Vec::new();
        let mut imports = BTreeMap::new();
        for name in imported {
            match exports.get(&name) {
                Some(value) => {
                    imports.insert(name, value.clone());
                }
                // "Fn::ImportValue returns the value of an output exported by another
                // stack": no live stack of this account and Region exports that name, so
                // there is no value to import and the deployment cannot be carried out
                // ([`Self::unresolved_cross_stack_failure`]).
                None => {
                    unresolved_imports.push(name);
                }
            }
        }
        // Which outputs of OTHER stacks this template really reads, resolved against the
        // same scope: only those belong on the stack, and the stack KEEPS them, so what it
        // reports is the value as of its own deployment.
        let read = template::stack_output_reads(&declared, &scope, &mut budget);
        let mut unresolved_stack_outputs = Vec::new();
        let mut read_stack_outputs = super::stack_outputs::Values::new();
        for reference in read {
            match stack_output_values.get(&reference) {
                Some(value) => {
                    read_stack_outputs.insert(reference, value.clone());
                }
                // No live stack of this account and Region carries that output, so there is
                // no value to read and the deployment cannot be carried out
                // ([`Self::unresolved_cross_stack_failure`]).
                None => unresolved_stack_outputs.push(reference),
            }
        }
        // An adoption is the stack's record of a resource's identity only while the stack
        // still MANAGES that resource: a deployment whose template no longer declares the
        // logical id stops managing it, so the entry goes with it and a later declaration of
        // that id creates a resource of its own.
        self.adopted
            .retain(|logical_id, _| resources.iter().any(|resource| resource.logical_id == *logical_id));
        self.pending_deployment = Some(PendingDeployment {
            resources,
            outputs,
            imports,
            unresolved_imports,
            stack_outputs: read_stack_outputs,
            unresolved_stack_outputs,
            wait_conditions,
            // Nothing has been acted on yet: this deployment has not started.
            attempted: BTreeSet::new(),
            dependencies,
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Outputs, events and the per-resource event timeline
// ---------------------------------------------------------------------------

impl StoredStack {
    /// The reason a deployment whose template names a CROSS-STACK VALUE that is not there
    /// cannot be carried out, or `None` when every cross-stack reference of the deployment
    /// in flight resolved.
    ///
    /// Two references read a value another stack holds, and neither has one to read when
    /// what it names is absent:
    ///
    /// * "The intrinsic function Fn::ImportValue returns the value of an output exported by
    ///   another stack": the import's value is the one that export carries in the account and
    ///   Region the deployment runs in, so a name no live stack of that Region exports leaves
    ///   the declaration carrying it with no value at all;
    /// * `Fn::GetStackOutput` reads one output of a named stack of that Region, so a
    ///   reference naming no such output leaves the declaration carrying it with no value
    ///   either ([`super::stack_outputs::no_output_named`]).
    ///
    /// CloudFormation cannot deploy such a template — it refuses the deployment before
    /// anything is asked of the service that owns the resource, which is why the reason is
    /// CloudFormation's OWN diagnostic ([`provisioning::Materialised::failure`]) and names
    /// the reference rather than a resource that tried and failed.
    ///
    /// One reference is reported — the FIRST the template names, which is the one the
    /// deployment could not resolve first; the imports come first because they are resolved
    /// first.
    pub fn unresolved_cross_stack_failure(&self) -> Option<String> {
        let deployment = self.pending_deployment.as_ref()?;
        if let Some(name) = deployment.unresolved_imports.first() {
            return Some(no_export_named(name));
        }
        let reference = deployment.unresolved_stack_outputs.first()?;
        Some(super::stack_outputs::no_output_named(reference))
    }

    /// Redirect the PENDING transition to a FAILURE: the terminal status the operation in
    /// flight settles to, and the reason that status carries.
    ///
    /// Called where this service cannot carry out what the request asked for — a template
    /// whose transforms it cannot apply, or one its reader will not read — so the operation
    /// reaches the service's modelled failure state with a reason that says so, rather than
    /// a completion reporting work nothing performed. The operation is still ANSWERED and
    /// still takes its clock-owed span: a caller reading straight after the call sees
    /// `*_IN_PROGRESS`, and one reading past the due instant sees the failure and the event
    /// that carries it. [`Self::fail_transition`] is the other case — a failure discovered
    /// after the call, which is reported at the instant it happened.
    pub fn fail_pending_transition(&mut self, settles_to: &str, reason: String) {
        if let Some(pending) = self.pending.as_mut() {
            pending.status = settles_to.to_string();
            pending.status_reason = Some(reason);
        }
    }

    /// The stack's declared outputs, evaluated against `resources`.
    ///
    /// An output's value is a function of the template, the parameters in force and the
    /// resources' IDENTITIES, so it is evaluated wherever one of those is settled: by
    /// [`Self::deploy_template`] when the template is deployed, and again by
    /// [`Self::record_materialised_resources`] once the services that own the resources
    /// have reported the identifiers they minted.
    pub(super) fn evaluated_outputs(
        &self,
        declared: &template::Template,
        scope: template::Scope<'_>,
        resources: &[StoredResource],
        budget: &mut template::Budget,
    ) -> Vec<StoredOutput> {
        let identities: BTreeMap<String, StoredResource> = resources
            .iter()
            .map(|resource| (resource.logical_id.clone(), resource.clone()))
            .collect();
        template::evaluate_outputs(
            declared,
            &template::Scope {
                resources: &identities,
                ..scope
            },
            budget,
        )
    }

    /// The template this stack carries and the parameter values in force on it.
    ///
    /// "Default: A value of the appropriate type for the template to use if no value is
    /// specified when a stack is created" — the request's values win over the template's
    /// declared defaults, which is what this merge is.
    ///
    /// `None` for a stack with no template, or one this service cannot read: there is
    /// nothing to evaluate then.
    ///
    /// The document read here is the template's PROCESSED stage — what CloudFormation
    /// deploys — so a resource or an output a transform's expansion declares is part of
    /// what the stack has, and a stage this service could not compute evaluates to
    /// nothing rather than to the unexpanded document.
    pub(super) fn template_scope(&self) -> Option<(template::Template, BTreeMap<String, String>)> {
        let declared = self.processed_stage_body().and_then(template::parse)?;
        let mut parameters = declared.parameter_defaults();
        for parameter in &self.parameters {
            parameters.insert(
                parameter.key.clone(),
                parameter.value_in_force().to_string(),
            );
        }
        Some((declared, parameters))
    }

    /// Re-evaluate the stack's outputs over the resource records now in force.
    ///
    /// Called where the identities those records carry have CHANGED since the template was
    /// deployed — which is what provisioning does for a type whose physical id the owning
    /// service assigns ([`template::identity_is_service_assigned`]): the output of such a
    /// resource must report the identifier that service minted, since that is the one a
    /// client calls it with. Nothing else about the evaluation differs, so the stack's
    /// outputs stay a function of its template, its parameters and its resources.
    pub(super) fn reevaluate_outputs(&mut self) {
        let Some((declared, parameters)) = self.template_scope() else {
            return;
        };
        let resources = self.current_resources();
        let parameter_types = ssm_parameters::parameter_types_in_force(&declared);
        let no_resources = BTreeMap::new();
        // The imports of the deployment in flight where there is one, else the stack's own:
        // an output reading an imported value reports the value THAT deployment resolved.
        let imports = match &self.pending_deployment {
            Some(deployment) => deployment.imports.clone(),
            None => self.imports.clone(),
        };
        // The other stacks' outputs this stack READ, taken from the same side: an output
        // reading one reports the value the deployment that read it resolved.
        let stack_outputs = match &self.pending_deployment {
            Some(deployment) => deployment.stack_outputs.clone(),
            None => self.stack_outputs.clone(),
        };
        let mut budget = template::Budget::new();
        let outputs = self.evaluated_outputs(
            &declared,
            template::Scope {
                stack: template::StackScope {
                    region: &self.region,
                    stack_name: &self.stack_name,
                    stack_id: &self.stack_id,
                    notification_arns: &self.notification_arns,
                },
                parameters: &parameters,
                parameter_types: &parameter_types,
                resources: &no_resources,
                mappings: &declared.mappings,
                conditions: &declared.conditions,
                imports: &imports,
                stack_outputs: &stack_outputs,
            },
            &resources,
            &mut budget,
        );
        match &mut self.pending_deployment {
            Some(deployment) => deployment.outputs = outputs,
            None => self.outputs = outputs,
        }
    }

    /// Whether this stack is still part of the live set — the set addressable by stack
    /// NAME. A stack whose deletion has completed is reachable only by its stack id
    /// (`DescribeStacksInput$StackName`: "Deleted stacks: You must specify the unique
    /// stack ID").
    pub fn is_live(&self) -> bool {
        self.status != status::DELETE_COMPLETE
    }

    /// Resolve the DECLARATIONS of the deployment in flight once more, against the resource
    /// records as they now stand.
    ///
    /// A declaration that reads an attribute of a resource whose own wait was still open
    /// resolved to nothing when the deployment computed it: an attribute the owning service
    /// publishes only once it reports the resource READY, and, for a CUSTOM RESOURCE, the
    /// `Data` its provider answers with, both land later
    /// ([`provisioning::advance_stabilisations`] records them). Once they have, the
    /// declaration the owning service is asked for is the one that reads them, so the
    /// properties are evaluated again here — over the same template, parameters and scope the
    /// deployment used, with the recorded attributes in place.
    ///
    /// The returned map is one entry per resource whose declaration STILL reads an attribute
    /// the resource it names does not publish, with the error that is: the answer is in hand
    /// and does not carry the key, so the consuming resource cannot be created at all.
    ///
    /// WHICH error depends on the resource NAMED. A type that does not publish the attribute
    /// at all is a template error ([`template::unsupported_attribute_reason`]); a CUSTOM
    /// RESOURCE publishes whatever its provider's answer carried, so a key that answer left
    /// out is a fault of the VENDOR's response, named with the response object it was read
    /// from ([`provisioning::custom_resource::vendor_response_missing_attribute`]) — the
    /// request this stack sent that provider, which the barrier that wait was held at carries.
    pub fn resolve_deferred_declarations(&mut self) -> BTreeMap<String, String> {
        let Some((declared, parameters)) = self.template_scope() else {
            return BTreeMap::new();
        };
        let Some(deployment) = self.pending_deployment.as_ref() else {
            return BTreeMap::new();
        };
        let imports = deployment.imports.clone();
        let stack_outputs = deployment.stack_outputs.clone();
        let mut resources = deployment.resources.clone();
        let in_force: BTreeMap<String, StoredResource> = resources
            .iter()
            .map(|resource| (resource.logical_id.clone(), resource.clone()))
            .collect();
        let parameter_types = ssm_parameters::parameter_types_in_force(&declared);
        let mut budget = template::Budget::new();
        let unresolved = {
            let scope = template::Scope {
                stack: template::StackScope {
                    region: &self.region,
                    stack_name: &self.stack_name,
                    stack_id: &self.stack_id,
                    notification_arns: &self.notification_arns,
                },
                parameters: &parameters,
                parameter_types: &parameter_types,
                resources: &in_force,
                mappings: &declared.mappings,
                conditions: &declared.conditions,
                imports: &imports,
                stack_outputs: &stack_outputs,
            };
            template::resolve_properties(&declared, &mut resources, &scope, &mut budget);
            template::unresolved_attributes(&declared, &scope, &mut budget)
        };
        if let Some(deployment) = self.pending_deployment.as_mut() {
            deployment.resources = resources;
        }
        unresolved
            .into_iter()
            .map(|(logical_id, (named, attribute))| {
                (logical_id, self.unresolved_read_reason(&named, &attribute))
            })
            .collect()
    }

    /// What a resource's declaration reading `attribute` of the resource `named` fails with
    /// ([`Self::resolve_deferred_declarations`] words every unresolved read through this).
    fn unresolved_read_reason(&self, named: &str, attribute: &str) -> String {
        let answered = self
            .current_resources()
            .iter()
            .find(|resource| resource.logical_id == named)
            .filter(|resource| {
                provisioning::custom_resource::is_custom_resource(&resource.resource_type)
            })
            .and_then(|_| {
                self.stabilisations
                    .iter()
                    .filter(|barrier| barrier.logical_id == named)
                    .find_map(|barrier| barrier.provider_request.as_ref())
            });
        match answered {
            Some(request) => provisioning::custom_resource::vendor_response_missing_attribute(
                attribute,
                &self.stack_id,
                named,
                &request.request_id,
            ),
            None => template::unsupported_attribute_reason(named, attribute),
        }
    }

    /// The stack's `Description`: the template's own `Description`, which is where
    /// CloudFormation takes both `Stack$Description` and
    /// `StackSummary$TemplateDescription` from.
    pub fn description(&self) -> Option<String> {
        self.template_description.clone()
    }

    /// Append a stack-level event. `LogicalResourceId` is the stack's name and
    /// `PhysicalResourceId` its id, as CloudFormation reports for the stack's own
    /// resource.
    /// The log is bounded at [`MAX_EVENTS_PER_STACK`]: past that the OLDEST event is
    /// reclaimed, so one caller cannot grow a single stack's history without limit.
    pub fn record_event(
        &mut self,
        at: SystemTime,
        resource_status: &str,
        resource_status_reason: Option<String>,
        operation_id: Option<String>,
        client_request_token: Option<String>,
    ) {
        let logical_resource_id = self.stack_name.clone();
        let physical_resource_id = Some(self.stack_id.clone());
        self.record_resource_event(
            at,
            &logical_resource_id,
            physical_resource_id,
            STACK_RESOURCE_TYPE,
            resource_status,
            resource_status_reason,
            // The stack's own event reports no `ResourceProperties`: the stack is not a
            // resource the template declared properties for.
            None,
            operation_id,
            client_request_token,
        );
    }

    /// Append an event about ONE of the stack's resources, which is what
    /// `DescribeStackEvents` reports beside the stack's own events ("all stack related
    /// events"): the resource's logical and physical ids and its type, with the status it
    /// is in, the reason it is in it and the properties it was deployed with
    /// (`StackEvent$ResourceProperties`: "BLOB of the properties used to create the
    /// resource").
    ///
    /// [`Self::record_event`] is the stack-level case of this writer (the stack's own
    /// resource); every event of either goes through [`Self::push_event`], so the
    /// [`MAX_EVENTS_PER_STACK`] bound and the oldest-first eviction live there once.
    #[allow(clippy::too_many_arguments)]
    pub fn record_resource_event(
        &mut self,
        at: SystemTime,
        logical_resource_id: &str,
        physical_resource_id: Option<String>,
        resource_type: &str,
        resource_status: &str,
        resource_status_reason: Option<String>,
        resource_properties: Option<String>,
        operation_id: Option<String>,
        client_request_token: Option<String>,
    ) {
        self.push_event(StoredEvent {
            event_id: new_identifier(&self.stack_id),
            stack_id: self.stack_id.clone(),
            stack_name: self.stack_name.clone(),
            operation_id,
            logical_resource_id: logical_resource_id.to_string(),
            physical_resource_id,
            resource_type: resource_type.to_string(),
            timestamp: at,
            resource_status: resource_status.to_string(),
            resource_status_reason,
            resource_properties,
            client_request_token,
            // A transition, not a validation result ([`Self::record_validation_failure`]).
            validation: None,
            // ...and one no pre-deployment validation failed: a failure of that stage is
            // reported by [`Self::fail_transition_on_validation`], which marks its event.
            failed_validation: false,
            // Filled by [`Self::push_event`], from the topics the stack carries now.
            owed_notification: None,
        });
    }

    /// Append an event reporting a PRE-DEPLOYMENT VALIDATION this stack's plan did not pass,
    /// about the resource it failed on: "Validation errors - Failures from CloudFormation
    /// Early Validations", which is the detail a failed change set's `StatusReason` sends a
    /// client to `DescribeEvents` for.
    ///
    /// Nothing was done to the resource — the plan was not executed — so the event carries no
    /// resource status: it reports WHICH validation failed on which resource and why
    /// ([`StoredValidationFailure`]), and is not part of the resource-transition stream.
    pub fn record_validation_failure(
        &mut self,
        at: SystemTime,
        logical_resource_id: &str,
        resource_type: &str,
        validation: StoredValidationFailure,
        operation_id: Option<String>,
        client_request_token: Option<String>,
    ) {
        self.push_event(StoredEvent {
            event_id: new_identifier(&self.stack_id),
            stack_id: self.stack_id.clone(),
            stack_name: self.stack_name.clone(),
            operation_id,
            logical_resource_id: logical_resource_id.to_string(),
            // Nothing was created, so there is no physical resource to name.
            physical_resource_id: None,
            resource_type: resource_type.to_string(),
            timestamp: at,
            resource_status: String::new(),
            resource_status_reason: None,
            resource_properties: None,
            client_request_token,
            validation: Some(validation),
            // The detailed status this event reports follows from the validation it carries
            // ([`super::detailed_status_of`]), so nothing else marks it.
            failed_validation: false,
            owed_notification: None,
        });
    }

    /// Append one event, oldest-first eviction off the FRONT of the deque so each bound
    /// costs O(1) rather than a shift of the whole log.
    ///
    /// TWO bounds, because a caller chooses both how many events it causes and how big the
    /// properties document each of them carries: [`MAX_EVENTS_PER_STACK`] entries, and
    /// [`MAX_EVENT_PROPERTIES_BYTES_PER_STACK`] bytes of retained
    /// `StackEvent$ResourceProperties` — with [`MAX_EVENT_PROPERTIES_BYTES`] the ceiling on
    /// any ONE of them, past which the event reports no properties at all rather than a
    /// document cut in half.
    pub(super) fn push_event(&mut self, mut event: StoredEvent) {
        // "The Amazon SNS topic ARNs to publish stack related events": the notification this
        // event owes goes to the topics the stack carries NOW, so an operation that later
        // clears them does not silence an event already recorded, and one that names a topic
        // again does not deliver the events of the window in which the stack had none. What
        // the event carries is the DIGEST of that list and not the list itself
        // ([`super::stack_notifications`]).
        event.owed_notification = super::stack_notifications::owed_by(&self.notification_arns);
        if event
            .resource_properties
            .as_ref()
            .is_some_and(|properties| properties.len() > MAX_EVENT_PROPERTIES_BYTES)
        {
            event.resource_properties = None;
        }
        let properties_bytes = event_properties_bytes(&event);
        while self.events.len() >= MAX_EVENTS_PER_STACK
            || (!self.events.is_empty()
                && self.retained_event_properties_bytes + properties_bytes
                    > MAX_EVENT_PROPERTIES_BYTES_PER_STACK)
        {
            let Some(evicted) = self.events.pop_front() else {
                break;
            };
            self.retained_event_properties_bytes = self
                .retained_event_properties_bytes
                .saturating_sub(event_properties_bytes(&evicted));
        }
        self.retained_event_properties_bytes += properties_bytes;
        self.events.push_back(event);
    }

    /// Plan the event each RESOURCE transition of a stack operation owes, as the walk that
    /// operation made over the stack's resources ([`timeline`]).
    ///
    /// "Returns all stack related events for a specified stack": an event is not only the
    /// stack's own status changes but one record per transition of each resource the
    /// operation acted on, naming that resource — which is what tells a client WHICH
    /// resource a failed deployment failed on and, through
    /// `StackEvent$ResourceStatusReason`, why the service that owns it refused.
    ///
    /// A stack operation is a WALK over the dependency graph, though, and
    /// `StackEvent$Timestamp` is "Time the status was updated" — so these records are not
    /// written here at one instant. Each one falls due when the walk reached it: a resource
    /// is scheduled once everything it depends on has finished, and a wait condition the
    /// operation waits at finishes when its signals arrive. [`Self::settle`] is what
    /// records each event at that instant, so the log a client reads is the timeline of the
    /// deployment rather than a set of simultaneous records.
    ///
    /// Every event of one stack operation carries that operation's id and the token the
    /// call supplied ("All events triggered by a given stack operation are assigned the
    /// same client request token"), which is what the stack is reporting as its last
    /// operation while these are planned.
    pub fn schedule_resource_events(
        &mut self,
        transitions: &[provisioning::ResourceTransition],
    ) {
        // A resource the operation planned a transition for is one it ACTED ON, which is
        // what the deployment in flight remembers about it: CloudFormation reports a
        // resource once the work on it has started, and that record outlives an operation
        // that then disposed of the resource.
        if let Some(deployment) = self.pending_deployment.as_mut() {
            for transition in transitions {
                deployment.attempted.insert(transition.logical_id.clone());
            }
        }
        // The walk belongs to the operation in flight, so its timeline is measured from
        // the instant that operation started.
        let started_at = self
            .pending
            .as_ref()
            .map_or_else(SystemTime::now, |pending| pending.started_at);
        // Whatever an earlier walk of this stack still owed happened before this one: it is
        // recorded rather than dropped, and never after what follows it.
        self.flush_resource_timeline(started_at);
        let gated: BTreeSet<String> = self
            .wait_conditions
            .iter()
            .map(|barrier| barrier.logical_id().to_string())
            // A resource whose owning service has not reported it ready yet is gated the
            // same way a wait condition is: what ENDS its creation is that service's
            // answer, not the span the walk was laid out across. A wait for a resource this
            // operation DELETED gates nothing here — the stack stopped recording that
            // physical resource when the delete was accepted, so there is no event of it
            // left to hold.
            .chain(
                self.coming_up().map(|barrier| barrier.logical_id.clone()),
            )
            .collect();
        let planned = {
            // The relation of the template being DEPLOYED where there is one, else the one
            // in force — which is what a deletion walks.
            let dependencies = match &self.pending_deployment {
                Some(deployment) => &deployment.dependencies,
                None => &self.resource_dependencies,
            };
            timeline::plan(
                started_at,
                STATUS_TRANSITION_DELAY,
                transitions,
                dependencies,
                &gated,
                self.last_operation()
                    .map(|operation| operation.operation_id.clone()),
                self.client_request_token.clone(),
            )
        };
        self.resource_timeline = planned;
    }

    /// Record the per-resource events the walk in flight has made due at `now`, leaving the
    /// ones it has not reached for a later call.
    ///
    /// Only ever records an event whose instant has passed, so it reports what the walk had
    /// got to at `now` even if nobody had called, and never stamps one in the future.
    pub(super) fn record_due_resource_events(&mut self, now: SystemTime) {
        self.drain_resource_timeline(now, false);
    }

    /// Record every per-resource event the walk still owes, none of them later than `at`,
    /// and have done with it.
    ///
    /// Called where the operation that made the walk CONCLUDES — it completed, or it
    /// failed: everything it did happened at or before that instant, so nothing it recorded
    /// is left unreported and nothing is stamped after the conclusion it belongs to.
    pub(super) fn flush_resource_timeline(&mut self, at: SystemTime) {
        self.drain_resource_timeline(at, true);
    }

    /// Record the per-resource events of the walk in flight, oldest first.
    ///
    /// `flush` is what separates the two callers above: with it the walk is compressed into
    /// the span the operation really took ([`timeline::Timeline::compress_into`]) and every
    /// event it still owes is recorded, at its own instant or at `now` where the walk had
    /// not reached it; without it only the events the walk has reached are recorded and the
    /// rest are left for the clock.
    pub(super) fn drain_resource_timeline(&mut self, now: SystemTime, flush: bool) {
        let Some(mut walk) = self.resource_timeline.take() else {
            return;
        };
        let released = self.gate_releases();
        if flush {
            walk.compress_into(now, &released);
        }
        let times = walk.times(&released);
        // (instant, step, the event's place in that step), so the log is appended in the
        // order the walk did these things.
        let mut owed: Vec<(SystemTime, usize, usize)> = Vec::new();
        for (index, step) in walk.steps.iter().enumerate() {
            let at = times.get(index).copied().unwrap_or_default();
            let count = step.events.len();
            for (place, event) in step.events.iter().enumerate() {
                if event.recorded {
                    continue;
                }
                if let Some(when) = due_instant(at.event_at(place, count), now, flush) {
                    owed.push((when, index, place));
                }
            }
        }
        owed.sort();
        for (at, index, place) in owed {
            let Some(step) = walk.steps.get_mut(index) else {
                continue;
            };
            let logical_id = step.logical_id.clone();
            let resource_type = step.resource_type.clone();
            let Some(event) = step.events.get_mut(place) else {
                continue;
            };
            event.recorded = true;
            let physical_resource_id = event.physical_resource_id.clone();
            let status = event.status.clone();
            let reason = event.reason.clone();
            let resource_properties = event.resource_properties.clone();
            self.record_resource_event(
                at,
                &logical_id,
                physical_resource_id,
                &resource_type,
                &status,
                reason,
                resource_properties,
                walk.operation_id.clone(),
                walk.client_request_token.clone(),
            );
        }
        if !flush {
            // Kept until the operation it belongs to concludes, not until its last event is
            // recorded: what it computed is also the instant each resource's own status was
            // updated, which the conclusion installs.
            self.resource_timeline = Some(walk);
        }
    }
}

// ---------------------------------------------------------------------------
// Signals, request tokens and the operations the stack lives through
// ---------------------------------------------------------------------------

impl StoredStack {
    /// The instant each GATED resource of the operation in flight finished, by logical id —
    /// which is also when the resources behind it were scheduled.
    ///
    /// Two kinds of resource are gated. A wait condition finished when the signal that
    /// brought it up to its `Count` arrived, read off the stack's signal log, which is the
    /// one record of what has arrived. A resource whose owning service had not reported it
    /// ready when the create answered finished when that service was first seen to report
    /// it ready ([`stabilising`]). One that has neither is absent, because nothing has
    /// finished it.
    fn gate_releases(&self) -> BTreeMap<String, SystemTime> {
        let mut released: BTreeMap<String, SystemTime> = self
            .wait_conditions
            .iter()
            .filter_map(|barrier| {
                self.satisfied_at(barrier)
                    .map(|at| (barrier.logical_id().to_string(), at))
            })
            .collect();
        // A resource held for its owning service to report it ready finished when that
        // service was first seen to report it: the instant the barrier was released. A wait
        // for a resource the operation DELETED gates no event — the stack stopped recording
        // that physical resource — so it finishes nothing here.
        for barrier in self.coming_up() {
            if let Some(at) = barrier.released_at {
                released.insert(barrier.logical_id.clone(), at);
            }
        }
        released
    }

    /// The instant `barrier` was satisfied: when the signal that brought it up to its
    /// `Count` arrived, or `None` while it is short of it.
    ///
    /// Over the same signals [`Self::successful_signals`] counts, in arrival order: the
    /// `Count`-th of them is the one that satisfied the wait condition.
    fn satisfied_at(
        &self,
        barrier: &wait_conditions::WaitConditionBarrier,
    ) -> Option<SystemTime> {
        let mut arrivals: Vec<SystemTime> = self
            .successful_signal_arrivals(barrier.logical_id())
            .collect();
        arrivals.sort_unstable();
        arrivals.get(barrier.count().saturating_sub(1)).copied()
    }

    /// Record a signal `SignalResource` sent one of this stack's resources, and answer
    /// whether it is one the stack had not already been sent.
    ///
    /// Keyed by the signalled resource and the signal's `UniqueId`, so re-sending a signal
    /// under a unique id already recorded is the same signal and not a second one — which
    /// is what lets a client retry the call. The log is bounded at
    /// [`MAX_SIGNALS_PER_STACK`], oldest reclaimed first.
    pub fn record_signal(
        &mut self,
        logical_resource_id: &str,
        unique_id: &str,
        status: &str,
        at: SystemTime,
    ) -> bool {
        if let Some(recorded) = self.signals.iter_mut().find(|signal| {
            signal.logical_resource_id == logical_resource_id && signal.unique_id == unique_id
        }) {
            recorded.status = status.to_string();
            recorded.received_at = at;
            return false;
        }
        while self.signals.len() >= MAX_SIGNALS_PER_STACK {
            self.signals.pop_front();
        }
        self.signals.push_back(StoredResourceSignal {
            logical_resource_id: logical_resource_id.to_string(),
            unique_id: unique_id.to_string(),
            status: status.to_string(),
            received_at: at,
        });
        true
    }

    /// Remember a client request token one of this stack's operations was started under:
    /// the token, and the `*_IN_PROGRESS` status of the operation that claimed it.
    ///
    /// Bounded at [`MAX_CLIENT_REQUEST_TOKENS_PER_STACK`], oldest reclaimed first, since a
    /// client chooses how many operations it drives against one stack. A token names ONE
    /// operation, so the FIRST operation to claim it keeps it: an operation this service
    /// opens under the token already in force (the rollback of a failed create, say)
    /// leaves the claim with the operation the caller sent it for.
    pub(super) fn claim_client_request_token(
        &mut self,
        token: &str,
        in_progress: &str,
        operation_id: Option<&str>,
    ) {
        if self
            .client_request_tokens
            .iter()
            .any(|claimed| claimed.token == token)
        {
            return;
        }
        while self.client_request_tokens.len() >= MAX_CLIENT_REQUEST_TOKENS_PER_STACK {
            self.client_request_tokens.pop_front();
        }
        self.client_request_tokens.push_back(StoredRequestToken {
            token: token.to_string(),
            opened: in_progress.to_string(),
            operation_id: operation_id.map(str::to_string),
        });
    }

    /// Record that the removal of ONE physical resource this stack no longer manages was
    /// REFUSED, with the reason that service — or that resource's own provider — gave.
    ///
    /// `DELETE_FAILED` is "the status of a resource the service that owns it refused to
    /// delete", and the resource is still there: this is the event that names which physical
    /// resource was left behind and why, for a removal the operation in flight owed but is
    /// not itself failed by — the cleanup of a resource an update REPLACED, whose own change
    /// has already been applied ("the stack update was successful, and CloudFormation is
    /// deleting the old resources"). A removal the OPERATION is failed by goes through
    /// [`Self::fail_stabilisation`] instead, which installs the operation's failed status too.
    pub fn record_failed_removal(
        &mut self,
        now: SystemTime,
        logical_resource_id: &str,
        resource_type: &str,
        physical_resource_id: &str,
        reason: &str,
    ) {
        let (operation_id, client_request_token) = match self.pending.as_ref() {
            Some(pending) => (
                pending.operation_id.clone(),
                pending.client_request_token.clone(),
            ),
            None => (None, self.client_request_token.clone()),
        };
        self.record_resource_event(
            now,
            logical_resource_id,
            Some(physical_resource_id.to_string()),
            resource_type,
            resource_status::DELETE_FAILED,
            Some(reason.to_string()),
            None,
            operation_id,
            client_request_token,
        );
    }

    /// The claim an operation of this stack already made on `token`, or `None` for a token
    /// new to it.
    ///
    /// What the token-uniqueness rule is read through: `Some` is a token already used for
    /// this stack, and the operation it names is what tells a retry of that same operation
    /// from a second one presenting its token.
    pub fn client_request_token_claim(&self, token: &str) -> Option<&StoredRequestToken> {
        self.client_request_tokens
            .iter()
            .find(|claimed| claimed.token == token)
    }

    /// Record the operation now acting on this stack, which is the one
    /// `Stack$LastOperations` reports until the next one starts.
    ///
    /// Keyed by the operation's id, so an operation already recorded is not opened a
    /// second time: the calls that open one, and the rollback a failure of it owes, are
    /// all the SAME attempt when they carry the same id.
    pub fn record_operation(&mut self, operation_type: &str, operation_id: &str, at: SystemTime) {
        self.record_change_set_operation(operation_type, operation_id, None, at);
    }

    /// [`Self::record_operation`] for an operation a CHANGE SET caused, which is reported
    /// against that change set as well as against the stack.
    pub fn record_change_set_operation(
        &mut self,
        operation_type: &str,
        operation_id: &str,
        change_set_id: Option<String>,
        at: SystemTime,
    ) {
        if self
            .operations
            .iter()
            .any(|recorded| recorded.operation_id == operation_id)
        {
            return;
        }
        while self.operations.len() >= MAX_OPERATIONS_PER_STACK {
            self.operations.pop_front();
        }
        self.operations.push_back(StoredOperation::opened(
            operation_type,
            operation_id,
            change_set_id,
            at,
        ));
    }

    /// The operation that last acted on this stack — what `Stack$LastOperations` reports —
    /// or `None` for a stack nothing has opened an operation on.
    pub fn last_operation(&self) -> Option<&StoredOperation> {
        self.operations.back()
    }

    /// The operation this stack recorded under `operation_id`, or `None` for an id it holds
    /// no operation for (one of another stack, and one this log has evicted).
    pub fn operation(&self, operation_id: &str) -> Option<&StoredOperation> {
        self.operations
            .iter()
            .find(|recorded| recorded.operation_id == operation_id)
    }

    /// Close the operation `operation_id` at `at`, in the status the stack-level transition
    /// that just settled names.
    ///
    /// A transition that did not go through ([`FAILED_SUFFIX`]) is the attempt FAILING, and
    /// it stays failed: the rollback its failure owed runs under the same id and settling
    /// that rollback does not turn the attempt into one that succeeded. `ended_at` is the
    /// instant its LAST transition settled, so an operation still unwinding has the one it
    /// has reached.
    pub(super) fn conclude_operation(
        &mut self,
        operation_id: Option<&str>,
        settled_status: &str,
        at: SystemTime,
    ) {
        let Some(operation_id) = operation_id else {
            return;
        };
        let Some(recorded) = self
            .operations
            .iter_mut()
            .find(|recorded| recorded.operation_id == operation_id)
        else {
            return;
        };
        recorded.ended_at = Some(at);
        if settled_status.ends_with(FAILED_SUFFIX) {
            recorded.status = operation_status::FAILED.to_string();
        } else if recorded.status != operation_status::FAILED {
            recorded.status = operation_status::SUCCEEDED.to_string();
        }
    }

    /// Close the operation that was CREATING `change_set_id`, in the state the change set's
    /// own terminal `ChangeSetStatus` names: a creation that completed is the attempt
    /// succeeding, and one that failed — a plan that would change nothing, a template whose
    /// transforms could not be applied — is the attempt failing.
    ///
    /// A change set settles on its own clock rather than through a stack transition
    /// ([`StoredChangeSet::settle`]), which is why its operation is concluded from here.
    pub fn conclude_change_set_operation(
        &mut self,
        change_set_id: &str,
        settled_status: &str,
        at: SystemTime,
    ) {
        let Some(recorded) = self
            .operations
            .iter_mut()
            .find(|recorded| recorded.change_set_id.as_deref() == Some(change_set_id))
        else {
            return;
        };
        recorded.ended_at = Some(at);
        recorded.status = if settled_status == change_set_status::FAILED {
            operation_status::FAILED
        } else {
            operation_status::SUCCEEDED
        }
        .to_string();
    }

    /// Materialise the status transition the clock has already made due, if any, and record
    /// the per-resource events the operation's walk has reached.
    ///
    /// Only ever applies a transition whose due instant has passed, so it reports what
    /// was owed at `now` even if nobody had called — it never advances a stack because
    /// it was read.
    ///
    /// An operation waiting at a wait condition has no transition owed at all until the
    /// signals arrive, so the barriers are consulted FIRST: one whose `Timeout` has run
    /// out fails the operation (which the clock does owe at that instant), and one still
    /// waiting holds the pending transition back however long ago it was opened. An
    /// operation one of whose resources its owning service has not reported ready yet is
    /// held the same way ([`stabilising`]) — and that wait is not the clock's to end at all,
    /// so nothing here releases it.
    ///
    /// The WALK the operation made over the stack's resources is on the same clock and is
    /// reported as it proceeds ([`timeline`]): a client polling `DescribeStackEvents` reads
    /// the resources the walk has scheduled and finished so far, and the walk is laid out
    /// inside the span the operation's own clock owes, so no resource of it is reported as
    /// finishing after the operation that acted on it.
    pub fn settle(&mut self, now: SystemTime) {
        self.settle_wait_conditions(now);
        // What the operation's walk over the resources owes at `now`, whether or not the
        // operation as a whole is due: a deployment in flight is reported as far as it has
        // got.
        self.record_due_resource_events(now);
        if self.waiting_for_signals() {
            // The operation is still waiting for its signals: nothing is due.
            return;
        }
        if self.waiting_for_stabilisation() {
            // A resource the operation created is not ready in the service that owns it
            // yet, or one it deleted is still held there, so the operation's completion is
            // not owed either: what releases it is that service's own answer
            // ([`provisioning::advance_stabilisations`]), which a read of the store cannot
            // ask for.
            return;
        }
        if self.owes_a_teardown() {
            // The teardown in flight has not asked about every resource the stack holds
            // yet: it left what a resource still being removed depends on alone, and those
            // resources are asked about once that removal answers
            // ([`provisioning::advance_stabilisations`] drives that pass too). A deletion
            // that still owes a resource a call is not one the clock can complete — the
            // resource is still there.
            return;
        }
        if self.owed_rollback.is_some() {
            // A rollback this stack owes the services that own its resources has not been
            // carried out yet ([`settle_due_rollbacks`] is what performs it), and a
            // rollback is not reported as finished before it has: the clock owes nothing
            // until then.
            return;
        }
        let Some(due_at) = self.pending.as_ref().map(|pending| pending.due_at) else {
            // No operation is in flight, so a walk left over belongs to nothing: whatever
            // it still owed is recorded rather than held.
            self.flush_resource_timeline(now);
            return;
        };
        if due_at > now {
            return;
        }
        // When each resource the walk acted on finished, which is the instant its own
        // status was updated — read before the walk is drained, since draining it is what
        // retires it.
        let finished_at = self.resource_finished_instants();
        // Everything the operation did happened before it concluded, so whatever its walk
        // still owed is recorded now — ahead of the stack's own terminal event, and never
        // later than the last instant the walk itself computed.
        let walk_ends_at = self.resource_walk_ends_at().unwrap_or(due_at);
        self.flush_resource_timeline(due_at.max(walk_ends_at));
        // The operation is completing, so its barriers belong to nothing from here on.
        // They are kept until exactly this point, so what the operation had created stays
        // reported for the whole of it rather than disappearing between the releasing
        // signal and the completion it makes due.
        self.wait_conditions.clear();
        self.stabilisations.clear();
        // ...and so does the rollback the operation would have owed had it failed. A
        // rollback that is COMPLETING is one the owning services carried out, so nothing is
        // left unreverted either.
        self.rollback_plan = None;
        self.unreverted_rollback = None;
        let pending = self.pending.take().expect("checked just above");
        let completing_deletion = pending.status == status::DELETE_COMPLETE;
        let deployed = self.pending_deployment.take();
        // A ROLLBACK that is completing went through the phase that removes what the
        // operation it reverted had created, between putting the previous configuration back
        // and its own terminal status: that phase is an event of the stack's, recorded ahead
        // of the terminal one it leads to.
        self.record_rollback_cleanup(
            due_at,
            &pending.status,
            pending.operation_id.clone(),
            pending.client_request_token.clone(),
        );
        // The per-RESOURCE events of this operation were recorded as its walk reached each
        // resource ([`Self::drain_resource_timeline`], flushed just above) — so by now the
        // only event still owed is the stack's own terminal one, which closes the operation
        // after them.
        self.status = pending.status;
        // "A last known stable state includes any status in a *_COMPLETE": the operation
        // that just concluded settled the stack in one, so that status is what a later
        // rollback has to go back to ([`Self::last_stable_status`]). Recorded here, the one
        // place a terminal status is installed.
        if is_stable_status(&self.status) {
            self.last_stable_status = Some(self.status.clone());
            // ...and the configuration that operation applied is the stack's from here on,
            // so the revision it replaced is no longer anything to go back to. An operation
            // that settled in a FAILED status keeps it: that stack is one a rollback can
            // still be asked for ([`Self::begin_requested_rollback`]), and the revision it
            // would put back is held nowhere else.
            self.previous_configuration = None;
        }
        // The reason the terminal status carries: the one the operation failed with, and
        // nothing for one that completed.
        self.status_reason = pending.status_reason.clone();
        if completing_deletion || self.status == status::ROLLBACK_COMPLETE {
            if completing_deletion {
                self.deletion_time = Some(due_at);
            }
            // The operation DISPOSED of the stack's resources — a deletion that completed,
            // and the rollback of a create that removed again what the create was
            // deploying. Each resource it disposed of keeps its RECORD, at its terminal
            // delete status as of the instant the walk disposed of it — reported, not
            // dropped: "For deleted stacks, DescribeStackResources returns resource
            // information for up to 90 days after the stack has been deleted", and a
            // rolled-back stack's rows are the only thing left that says WHICH resource
            // the rollback disposed of. The output VALUES go, since they were evaluated
            // against resources that no longer exist.
            self.resources =
                disposed_resources(&self.resources, deployed, due_at, &finished_at);
            self.outputs.clear();
            // A stack that holds no resources any more consumes nobody's export: it drops
            // out of the cross-stack reference index `ListImports` reports.
            self.imports.clear();
            // Nothing of this stack reads another's output any more either: the
            // declarations that read one are gone with the resources and the outputs.
            self.stack_outputs.clear();
        } else if let Some(deployment) = deployed {
            // The create or update completed: the template it deployed is what the stack
            // reports resources and outputs for from here on.
            self.resources = installed_resources(
                &self.resources,
                deployment.resources,
                due_at,
                &finished_at,
            );
            self.outputs = deployment.outputs;
            // The imports the template that finished deploying resolved are the stack's
            // from here on: that is what makes it an importer of those exports.
            self.imports = deployment.imports;
            // The other stacks' outputs that template read are the stack's from here on,
            // which is what makes the value it reports its OWN deployment's.
            self.stack_outputs = deployment.stack_outputs;
            // The relation the template that finished deploying describes is the stack's
            // from here on, which is what its deletion walks.
            self.resource_dependencies = deployment.dependencies;
        }
        let status = self.status.clone();
        // What the EVENT for this terminal status says. One failure is explained ONCE in the
        // event stream, by the transition that accounts for it: a rollback's reason is
        // carried by the event that OPENED that rollback ([`Self::begin_rollback`],
        // [`Self::begin_update_rollback`]), so the terminal status it settles into adds no
        // second copy of it — a client counting the events that carry a failure reads one
        // failure, not one per transition that observed it. The stack's own
        // `StackStatusReason` keeps the reason either way, which is where a client reads why
        // the stack is in the status it rests in.
        let reason = if concludes_a_rollback(&status) {
            None
        } else {
            pending.status_reason
        };
        // The transition the clock made due is the operation's: it concluded at the instant
        // that status was installed, in the state that status names
        // ([`Self::conclude_operation`]).
        self.conclude_operation(pending.operation_id.as_deref(), &status, due_at);
        self.record_event(
            due_at,
            &status,
            reason,
            pending.operation_id,
            pending.client_request_token,
        );
    }

    /// Record the CLEANUP phase of a ROLLBACK that is completing at `at`.
    ///
    /// "UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS — CloudFormation is cleaning up the
    /// resources after the rollback": a rollback does not go straight from putting the
    /// previous configuration back to its terminal status — it then removes what the
    /// operation it reverted had created, and that phase is a status of its own in the
    /// stack's event stream, which is what a client auditing the documented rollback phases
    /// reads. It belongs to every rollback alike: the one a failed update owed, the one
    /// `RollbackStack` asked for and the one `ContinueUpdateRollback` resumed, all of which
    /// reach their terminal status through here.
    ///
    /// The phase falls due at the instant the rollback's terminal status does, because by
    /// then the revert itself is through: [`Self::settle`] holds that status back until the
    /// work the rollback owed the owning services has been carried out
    /// ([`Self::owed_rollback`]), and what is left is the removal this phase reports.
    ///
    /// `terminal` is the status the operation is settling into, so nothing is recorded for an
    /// operation that is not a rollback of an update.
    ///
    /// The phase is an EVENT of the rollback and not a status the stack rests in: the
    /// terminal status falls due at the same instant, and it is what a client reading the
    /// stack sees, so nothing here writes [`Self::status`].
    fn record_rollback_cleanup(
        &mut self,
        at: SystemTime,
        terminal: &str,
        operation_id: Option<String>,
        client_request_token: Option<String>,
    ) {
        if terminal != status::UPDATE_ROLLBACK_COMPLETE {
            return;
        }
        self.record_event(
            at,
            status::UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS,
            None,
            operation_id,
            client_request_token,
        );
    }

    /// The instant the LAST resource the walk in flight acted on finished, if it has one.
    ///
    /// The walk is laid out inside the span the operation's own clock owes, so this is
    /// normally before the operation's due instant; it is what the events the operation's
    /// conclusion still owes are held to, so none of them is stamped after the walk really
    /// got there.
    fn resource_walk_ends_at(&self) -> Option<SystemTime> {
        self.resource_timeline
            .as_ref()?
            .ends_at(&self.gate_releases())
    }

    /// The instant each resource the walk in flight acted on finished, by logical id —
    /// what `StackResource$Timestamp` ("Time the status was updated") reports for a
    /// resource the operation leaves in a new status.
    pub(super) fn resource_finished_instants(&self) -> BTreeMap<String, SystemTime> {
        match &self.resource_timeline {
            Some(walk) => walk.finished_instants(&self.gate_releases()),
            None => BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Opening an operation, and the waits that hold its conclusion back
// ---------------------------------------------------------------------------

impl StoredStack {
    /// Start a stack operation: move the stack to its `*_IN_PROGRESS` status, record the
    /// opening event, and make the terminal status due one [`STATUS_TRANSITION_DELAY`]
    /// from now.
    ///
    /// A create or update whose template declares a wait condition is due its terminal
    /// status only once the signals arrive: [`Self::opening_barriers`] is what records
    /// that, and [`Self::settle`] holds the transition back while one is unsatisfied.
    pub fn begin_transition(
        &mut self,
        now: SystemTime,
        in_progress: &str,
        settles_to: &str,
        operation_id: Option<String>,
        client_request_token: Option<String>,
    ) {
        // An operation starts only once the one before it is over: whatever its walk over
        // the resources still owed happened before this call.
        self.flush_resource_timeline(now);
        self.status = in_progress.to_string();
        self.status_reason = Some(USER_INITIATED.to_string());
        // `Stack$LastOperations`: "Information about the most recent operations performed
        // on this stack" — this call is that operation, and the type is the one the
        // status it opens names.
        if let (Some(operation_type), Some(id)) =
            (operation_type_of(in_progress), operation_id.clone())
        {
            self.record_operation(operation_type, &id, now);
        }
        // "A unique identifier for this request": the token this operation runs under is
        // remembered against the stack, so a later operation presenting it is the
        // duplicate submission `TokenAlreadyExistsException` names and a repeat of THIS
        // operation under it is the retry the member is documented for.
        if let Some(token) = client_request_token.as_deref() {
            self.claim_client_request_token(token, in_progress, operation_id.as_deref());
        }
        self.record_event(
            now,
            in_progress,
            Some(USER_INITIATED.to_string()),
            operation_id.clone(),
            client_request_token.clone(),
        );
        // The rollback the operation now starting would owe is not known until that
        // operation has asked the owning services for what it wanted
        // ([`Self::hold_rollback_plan`]); whatever the operation before it held belongs to
        // nothing.
        self.rollback_plan = None;
        self.pending = Some(PendingStatus {
            status: settles_to.to_string(),
            // A completion carries no reason, and nothing has failed yet:
            // `Self::fail_transition` is what records a failure of this operation.
            status_reason: None,
            started_at: now,
            due_at: now + STATUS_TRANSITION_DELAY,
            operation_id,
            client_request_token,
        });
        self.wait_conditions = self.opening_barriers(now, in_progress);
        // The create-time waits an EARLIER operation opened belong to that operation: this
        // one has not created anything yet, and what it creates opens its own
        // ([`Self::record_stabilisations`]).
        self.stabilisations.clear();
    }

    /// The wait conditions the operation now opening has to wait at.
    ///
    /// Only a CREATE or an UPDATE waits: those are the operations CloudFormation "doesn't
    /// proceed with … until resources receive the required number of signals"
    /// (`SignalResource`), while a delete or a rollback removes what is there and waits
    /// for nothing. A wait a `CreationPolicy` declares is a wait of the CREATE alone — that
    /// attribute "is invoked during resource creation" — so an update that re-deploys the
    /// same resource is not held at it
    /// ([`wait_conditions::DeclaredWaitCondition::only_on_create`]), while a wait condition
    /// waits whichever operation deploys it. A wait condition the stack has ALREADY deployed
    /// and that this
    /// deployment declares unchanged is not waited at a second time — it is the same
    /// satisfied wait condition ("you can't update a wait condition that has already been
    /// satisfied unless you also change the handle").
    fn opening_barriers(
        &self,
        now: SystemTime,
        in_progress: &str,
    ) -> Vec<wait_conditions::WaitConditionBarrier> {
        if !matches!(
            in_progress,
            status::CREATE_IN_PROGRESS | status::UPDATE_IN_PROGRESS
        ) {
            return Vec::new();
        }
        let Some(deployment) = &self.pending_deployment else {
            return Vec::new();
        };
        deployment
            .wait_conditions
            .iter()
            .filter(|declared| {
                !declared.only_on_create() || in_progress == status::CREATE_IN_PROGRESS
            })
            .filter(|declared| !self.already_satisfied(&declared.logical_id))
            .cloned()
            .map(|declared| wait_conditions::WaitConditionBarrier::open(declared, now))
            .collect()
    }

    /// Whether the wait condition under this logical id is one the stack already has,
    /// declared again exactly as it stands: such a wait condition got its signals when it
    /// was created and is not waited at again.
    fn already_satisfied(&self, logical_id: &str) -> bool {
        let Some(deployment) = &self.pending_deployment else {
            return false;
        };
        let declared = deployment
            .resources
            .iter()
            .find(|resource| resource.logical_id == logical_id);
        let installed = self
            .resources
            .iter()
            .find(|deployed| deployed.resource.logical_id == logical_id);
        match (declared, installed) {
            (Some(declared), Some(installed)) => {
                installed.resource.resource_type == declared.resource_type
                    && installed.resource.properties == declared.properties
            }
            _ => false,
        }
    }

    /// When each DISTINCT success signal this stack has received for `logical_id` arrived.
    ///
    /// Read off the stack's signal log, which `SignalResource` keys by the signalled
    /// resource and the signal's `UniqueId` ([`Self::record_signal`]): "if you send
    /// multiple signals to a single resource (such as signaling a wait condition), each
    /// signal requires a different unique ID", so two signals under one id are one signal.
    /// This is the one place that says which of a stack's signals COUNT for a resource:
    /// [`Self::successful_signals`] counts them and [`Self::satisfied_at`] reads when the
    /// one that satisfied a wait condition arrived.
    fn successful_signal_arrivals<'stack>(
        &'stack self,
        logical_id: &'stack str,
    ) -> impl Iterator<Item = SystemTime> + 'stack {
        self.signals
            .iter()
            .filter(move |signal| {
                signal.logical_resource_id == logical_id
                    && signal.status == resource_signal_status::SUCCESS
            })
            .map(|signal| signal.received_at)
    }

    /// How many DISTINCT success signals this stack has received for `logical_id` — the
    /// count a wait condition's `Count` is compared against.
    pub fn successful_signals(&self, logical_id: &str) -> usize {
        self.successful_signal_arrivals(logical_id).count()
    }

    /// Whether this barrier has the signals it waits for, so the operation may proceed.
    pub fn barrier_satisfied(&self, barrier: &wait_conditions::WaitConditionBarrier) -> bool {
        self.successful_signals(barrier.logical_id()) >= barrier.count()
    }

    /// Whether the operation in flight is still waiting at one of its wait conditions.
    pub(super) fn waiting_for_signals(&self) -> bool {
        self.wait_conditions
            .iter()
            .any(|barrier| !self.barrier_satisfied(barrier))
    }

    /// Make the operation's terminal status due where every wait condition it is waiting
    /// at now has the signals it waits for, and say whether this released it.
    ///
    /// Called by `SignalResource` once it has recorded a signal: "when the wait condition
    /// receives the requisite number of success signals, CloudFormation resumes the
    /// creation of the stack". The operation is then owed its terminal status from the
    /// signal that released it rather than from the call that started it, so a caller
    /// reading straight after the signal still sees the operation in progress — and the
    /// barriers stay on the stack until [`Self::settle`] completes it, so the resources it
    /// had created go on being reported while it finishes.
    pub fn release_wait_conditions(&mut self, now: SystemTime) -> bool {
        if self.wait_conditions.is_empty() || self.waiting_for_signals() {
            return false;
        }
        if let Some(pending) = &mut self.pending {
            pending.due_at = now + STATUS_TRANSITION_DELAY;
        }
        true
    }

    /// Record the resources of the operation in flight whose owning service accepted the
    /// call about them without having carried it out: a create that does not report the
    /// resource ready, and a delete that leaves that service still holding it
    /// ([`stabilising`]).
    ///
    /// Called by the operation that drove the deployment or the teardown, BEFORE it
    /// schedules the per-resource events: a resource being CREATED is gated, so the
    /// transition that ENDS its creation is not stamped until the owning service reports it
    /// ready ([`Self::schedule_resource_events`]).
    ///
    /// The resource's own `*_IN_PROGRESS` event is stamped with the instant the operation
    /// that acted on the resource started, which is what the barrier carries as the instant
    /// the resource's work began ([`stabilising::StabilisationBarrier::opened_at`]). What the
    /// WAIT itself runs from is the instant the call it is waiting for was MADE, which is
    /// this record ([`stabilising::StabilisationBarrier::asked_at`]): the calls were just
    /// answered, so a resource an operation reaches late in a long walk gets the whole of the
    /// window from its own call rather than what is left of the operation's.
    pub fn record_stabilisations(
        &mut self,
        pending: &[provisioning::Stabilising],
        now: SystemTime,
    ) {
        if pending.is_empty() {
            return;
        }
        let opened_at = self
            .pending
            .as_ref()
            .map_or(now, |operation| operation.started_at);
        let asked_at = SystemTime::now();
        for stabilising in pending {
            let barrier = stabilising::StabilisationBarrier::open(stabilising, opened_at, asked_at);
            // ONE barrier per wait: the same wait recorded twice would hold the operation
            // until BOTH were released, and the second release can never come — the answer
            // the wait is for is one answer. Two passes can reach the same conclusion at
            // once (a client's read of the stack and the task a landed provider answer
            // starts, see [`super::provider_driver`]), and each records what that conclusion
            // owes, so the wait a pass opens is skipped where the stack is already holding
            // it.
            if self
                .stabilisations
                .iter()
                .any(|held| held.is_the_same_wait(&barrier) && !held.released())
            {
                continue;
            }
            self.stabilisations.push(barrier);
        }
    }

    /// Whether the operation in flight is still waiting for the service that owns one of its
    /// resources to report that resource ready, or gone.
    pub fn waiting_for_stabilisation(&self) -> bool {
        self.stabilisations
            .iter()
            .any(|barrier| !barrier.released())
    }

    /// The resources of the operation in flight whose create or delete is still open, as the
    /// read that ends each wait addresses them — nothing of the store, so the caller can ask
    /// their owning services with the lock released.
    pub fn open_stabilisations(&self) -> Vec<stabilising::StabilisationBarrier> {
        self.stabilisations
            .iter()
            .filter(|barrier| !barrier.released())
            .cloned()
            .collect()
    }

    /// The waits of the operation in flight that are for a resource it CREATED.
    ///
    /// These are the waits a READ of the stack reports something about: such a resource is
    /// reported `*_IN_PROGRESS` under the identifier its owning service assigned
    /// ([`Self::reported_resources`]) and its own terminal event is held back
    /// ([`Self::schedule_resource_events`]). A wait for a resource the operation DELETED
    /// holds the operation and nothing else, because the stack stopped recording that
    /// physical resource when the owning service accepted the delete. A resource the
    /// operation is creating but has not STARTED on yet is one of these too
    /// ([`stabilising::Awaited::Deferred`]): the operation is bringing it into the stack.
    pub(super) fn coming_up(&self) -> impl Iterator<Item = &stabilising::StabilisationBarrier> {
        self.stabilisations
            .iter()
            .filter(|barrier| barrier.awaited != stabilising::Awaited::Gone)
    }

    /// Record that the service owning the resource `barrier` is held for has given the answer
    /// that wait was for, and make the operation's terminal status due where that was the
    /// last resource it was waiting for.
    ///
    /// The barrier HELD a conclusion the operation's own clock had already made due
    /// ([`STATUS_TRANSITION_DELAY`], stamped when the operation began): it never postponed
    /// it. So releasing the last barrier restores that instant rather than starting a fresh
    /// span — the conclusion stands at the LATER of the instant the operation's own span
    /// elapsed and the instant the owning service gave its answer, which is the first instant
    /// at which every part of it had happened. `now` is when that service was seen to give
    /// it, and a release is only ever recorded from a call that asked
    /// ([`provisioning::advance_stabilisations`]), which is the one thing a read of this
    /// store cannot do — so a read that asks and learns the resource came up, or went away,
    /// long ago reports the conclusion the clock owed from then, instead of charging the
    /// caller another span for having been the one to notice. The barriers stay on the stack
    /// until [`Self::settle`] completes it, so the resources the operation created go on
    /// being reported while it finishes.
    pub fn release_stabilisation(
        &mut self,
        barrier: &stabilising::StabilisationBarrier,
        now: SystemTime,
    ) {
        let Some(held) = self
            .stabilisations
            .iter_mut()
            .find(|held| held.is_the_same_wait(barrier) && !held.released())
        else {
            return;
        };
        held.released_at = Some(now);
        if self.waiting_for_stabilisation() {
            return;
        }
        if let Some(pending) = &mut self.pending {
            pending.due_at = pending.due_at.max(now);
        }
    }

    /// Record that what `barrier` was waiting for could not be carried through — the owning
    /// service reports the resource failed, or the wait ran out — and answer the resources
    /// the ROLLBACK of a failed create now has to remove, or `None` where this failure owes
    /// no such removal.
    ///
    /// For a resource being CREATED, its own terminal event becomes that failure rather than
    /// the completion the walk had planned for it, so the event stream says which resource
    /// did not come up and why; for one being DELETED, what the owning service had already
    /// answered stands. Either way the stack takes the failed status of the operation in
    /// flight, exactly as it does when an owning service refuses the call outright.
    ///
    /// A failed operation does not stop there. "ROLLBACK — Roll back the stack creation.
    /// This is the default": a failed CREATE removes what it had made, which is the work this
    /// answers — the caller performs it, since the calls reach the services that own those
    /// resources. A failed UPDATE is rolled back to the previous stack configuration instead,
    /// which is recorded as owed here ([`Self::begin_owed_update_rollback`]) and performed by
    /// [`settle_due_rollbacks`], exactly as a wait condition's timeout does it. A caller that
    /// asked for the failed state to be kept (`DisableRollback`, `OnFailure=DO_NOTHING`) gets
    /// neither: it said what it wanted left behind.
    ///
    /// What that removal asks the owning services about is what was really created, and a
    /// resource whose create was never carried out at all is not in it: a CUSTOM RESOURCE is
    /// brought into being by the answer its provider gives, so a `Create` no provider
    /// answered left nothing to remove ([`Self::discard_uncreated_resource`]).
    pub fn fail_stabilisation(
        &mut self,
        now: SystemTime,
        barrier: &stabilising::StabilisationBarrier,
        reason: &str,
    ) -> Option<Vec<StoredResource>> {
        // Nothing is owed for a barrier this stack is no longer holding, or for one whose
        // operation has already concluded: a failure is the OPERATION's, so it is recorded
        // exactly once however many barriers an earlier pass read off the stack — the same
        // guard [`Self::settle_wait_conditions`] makes before it fails one.
        let held = self
            .stabilisations
            .iter()
            .find(|held| held.is_the_same_wait(barrier) && !held.released())
            .filter(|_| self.pending.is_some())?;
        // Whether the wait was for a PROVIDER's answer that never came, read off the
        // barrier the stack itself holds rather than the reading handed in: the answer is
        // recorded on the stack's own copy ([`Self::record_provider_response`]).
        let unanswered_provider_request =
            held.provider_request.is_some() && held.provider_response.is_none();
        let logical_id = barrier.logical_id.as_str();
        let failed_status = failed_status_of(&self.status);
        let updating = self.status == status::UPDATE_IN_PROGRESS;
        let creating = self.status == status::CREATE_IN_PROGRESS;
        // WHICH action on the resource did not go through: what the stack's summary reports
        // it failed for, and the `*_FAILED` status that action leaves the RESOURCE itself in.
        //
        // The walk that opened this wait had already recorded the outcome it PLANNED for the
        // resource — it accepted the call and held the resource at the wait, so the record
        // states the work as carried through — and a wait that ends in a failure is exactly
        // that work NOT being carried through. `StackResource$ResourceStatus` is the
        // resource's "Current status", so it reports the failed status with the reason that
        // ended the wait: nothing reports a completion a provider or an owning service
        // refused, and this is the set a resumed rollback reads ("you can specify only
        // resources that are in the UPDATE_FAILED state because a rollback failed"). Both
        // follow from the same arm, so the event and the status cannot disagree about which
        // action failed.
        let (action, failed_resource_status) = match barrier.awaited {
            // The resource was being CREATED, so its own event is the failed CREATE, which
            // the walk had planned as a completion and the gate held back.
            stabilising::Awaited::Ready => {
                self.fail_planned_resource_event(
                    logical_id,
                    resource_status::CREATE_FAILED,
                    reason,
                );
                // A create whose PROVIDER never answered created nothing at all, so the
                // rollback below owes nobody a call about this resource.
                if unanswered_provider_request {
                    self.discard_uncreated_resource(logical_id);
                }
                (failed_action::CREATE, resource_status::CREATE_FAILED)
            }
            // The resource's creation had not STARTED: it was waiting for the resources it
            // depends on, and what did not go through is still its own create. There is no
            // planned event of it to rewrite — CloudFormation reports a resource once its
            // creation has started — so the failure is recorded as the resource's own.
            stabilising::Awaited::Deferred => {
                self.record_resource_event(
                    now,
                    logical_id,
                    None,
                    &barrier.resource_type,
                    resource_status::CREATE_FAILED,
                    Some(reason.to_string()),
                    None,
                    self.pending
                        .as_ref()
                        .and_then(|pending| pending.operation_id.clone()),
                    self.client_request_token.clone(),
                );
                (failed_action::CREATE, resource_status::CREATE_FAILED)
            }
            // The resource ALREADY EXISTED and what was being carried out on it was the
            // change this deployment declared, so what did not go through is its UPDATE:
            // the event the walk planned for it is the completion of that change, and the
            // failure that replaces it says the resource was not updated — never that a
            // resource the stack already had failed to be created.
            stabilising::Awaited::Updated => {
                self.fail_planned_resource_event(
                    logical_id,
                    resource_status::UPDATE_FAILED,
                    reason,
                );
                (failed_action::UPDATE, resource_status::UPDATE_FAILED)
            }
            // The resource was being DELETED, and what the owning service answered about
            // that delete was recorded when it answered: what did not happen is the removal,
            // so no event of the resource is rewritten.
            stabilising::Awaited::Gone => {
                (failed_action::DELETE, resource_status::DELETE_FAILED)
            }
        };
        self.record_resource_status(
            logical_id,
            failed_resource_status,
            Some(reason.to_string()),
            now,
        );
        let summary = resource_failure_summary(action, [logical_id]);
        // Whether the stack RESTS in the failed status decides whether an event of its own
        // reports that status: a rollback this failure owes begins at this very instant and
        // carries the failure on its own event
        // ([`Self::fail_transition_superseded`]).
        let rolls_back = !self.disable_rollback && (updating || creating);
        if rolls_back {
            self.fail_transition_superseded(now, failed_status, summary.clone());
        } else {
            self.fail_transition(now, failed_status, summary.clone());
        }
        if self.disable_rollback {
            return None;
        }
        if updating {
            self.begin_owed_update_rollback(now, summary);
            return None;
        }
        if !creating {
            return None;
        }
        self.begin_rollback(now, summary);
        Some(self.current_resources())
    }

    /// Replace the terminal event the walk planned for one GATED resource with the failure
    /// that ended it instead.
    ///
    /// The walk recorded the resource's completion when its create was accepted and the
    /// gate held that event back ([`timeline`]), so the event the stack has not recorded yet
    /// is the one to rewrite: nothing reports a completion the resource never reached.
    pub(super) fn fail_planned_resource_event(
        &mut self,
        logical_id: &str,
        status: &str,
        reason: &str,
    ) {
        self.rewrite_planned_resource_event(logical_id, status, reason, Reported::NotYet);
    }

    /// [`Self::fail_planned_resource_event`] for a failure the CALLER has already put in the
    /// event log: the planned completion is rewritten to that failure and RETIRED, so the
    /// resource is reported — and installed — as having failed, and the log carries the one
    /// event the caller wrote rather than that event and a completion after it.
    ///
    /// This is the shape a client's own FAILURE signal has: "a failure signal causes
    /// CloudFormation to immediately fail the stack creation or update"
    /// (`SignalResource`), and the signal that did it is what the resource's event reports
    /// ([`crate::ops::signal_resource`]).
    pub fn fail_reported_resource_event(
        &mut self,
        logical_id: &str,
        status: &str,
        reason: &str,
    ) {
        self.rewrite_planned_resource_event(logical_id, status, reason, Reported::Already);
    }

    /// The one writer of both rewrites above.
    ///
    /// The rewritten event reports the identifier the resource is known by NOW, not the one
    /// the walk planned the event with: a CUSTOM RESOURCE's physical id is the one its
    /// provider answered with ([`Self::adopt_provider_identity`], which the answer that ended
    /// the wait records before the failure is), so the event that reports the failure names
    /// the physical resource the provider is responsible for rather than the stand-in the
    /// naming rules had computed.
    fn rewrite_planned_resource_event(
        &mut self,
        logical_id: &str,
        status: &str,
        reason: &str,
        reported: Reported,
    ) {
        let in_force = self
            .current_resources()
            .iter()
            .find(|resource| resource.logical_id == logical_id)
            .map(|resource| resource.physical_resource_id.clone())
            .filter(|id| !id.is_empty());
        let Some(walk) = self.resource_timeline.as_mut() else {
            return;
        };
        for step in walk
            .steps
            .iter_mut()
            .filter(|step| step.logical_id == logical_id)
        {
            for event in step
                .events
                .iter_mut()
                .filter(|event| !event.recorded && !event.in_progress())
            {
                event.status = status.to_string();
                event.reason = Some(reason.to_string());
                event.recorded = reported == Reported::Already;
                if let Some(id) = &in_force {
                    event.physical_resource_id = Some(id.clone());
                }
            }
        }
    }
}

/// Whether the failure a planned event is rewritten to is already in the stack's event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reported {
    /// The rewritten event is the one that will report it, when the walk reaches it.
    NotYet,
    /// The caller has already recorded it, so the rewritten event is retired rather than
    /// reported a second time.
    Already,
}

// ---------------------------------------------------------------------------
// Failures, rollbacks and the resources a read reports
// ---------------------------------------------------------------------------

impl StoredStack {
    /// Materialise what the wait conditions of the operation in flight owe at `now`.
    ///
    /// The clock owes this to a wait condition whose `Timeout` has run out: "if the wait
    /// condition doesn't receive the specified number of success signals before the
    /// `Timeout` period expires, CloudFormation assumes that the wait condition has
    /// failed", so the operation takes the failed status carrying that reason — which was
    /// owed at that instant even if nobody had called.
    ///
    /// A failed operation does not stop there, and this failure is no different from one an
    /// owning service answered: a failed UPDATE is rolled back to the previous stack
    /// configuration, which is what [`crate::ops::update_stack`]'s own failure path does
    /// ([`Self::begin_owed_update_rollback`]). The CALLS that rollback makes in the
    /// services that own the resources cannot be made from here — this runs under the store
    /// lock, with no request to make them on — so the work is recorded as owed
    /// ([`Self::owed_rollback`]) and the next operation that can reach those services
    /// performs it ([`settle_due_rollbacks`]), with the rollback's own terminal status held
    /// back until it has.
    ///
    /// Nothing else about a barrier moves here: a SIGNAL is what satisfies one
    /// ([`Self::release_wait_conditions`]), and a barrier left behind by an operation that
    /// has already concluded belongs to no operation and is dropped.
    pub(super) fn settle_wait_conditions(&mut self, now: SystemTime) {
        if self.wait_conditions.is_empty() {
            return;
        }
        if self.pending.is_none() {
            self.wait_conditions.clear();
            return;
        }
        let Some(reason) = self
            .wait_conditions
            .iter()
            .find(|barrier| barrier.has_expired(now) && !self.barrier_satisfied(barrier))
            .map(|barrier| {
                barrier.timed_out_reason(self.successful_signals(barrier.logical_id()))
            })
        else {
            return;
        };
        let failed_status = failed_status_of(&self.status);
        let updating = self.status == status::UPDATE_IN_PROGRESS;
        self.wait_conditions.clear();
        // The UPDATE whose rollback is opened right below never rests in the failed status,
        // so no event of the stack's reports it ([`Self::fail_transition_superseded`]);
        // every other barrier timeout leaves the stack in the failed status it reports.
        if updating && !self.disable_rollback {
            self.fail_transition_superseded(now, failed_status, reason.clone());
        } else {
            self.fail_transition(now, failed_status, reason.clone());
        }
        if updating {
            self.begin_owed_update_rollback(now, reason);
        }
    }

    /// Open the ROLLBACK the UPDATE that has just failed owes.
    ///
    /// "If the stack update fails, CloudFormation rolls back the stack to the previous stack
    /// configuration": the stack moves to `UPDATE_ROLLBACK_IN_PROGRESS` and the work that
    /// puts the previous configuration back in the owning services becomes owed
    /// ([`Self::owed_rollback`]) — the plan the update's own handler computed, since only it
    /// knows what the update changed there.
    ///
    /// Called after [`Self::fail_transition`] has recorded the update's own failure, with
    /// that failure's reason. The rollback is owed HOWEVER far the update got: the plan the
    /// handler held ([`Self::hold_rollback_plan`]) says what has to be put back in the owning
    /// services, and an update that changed nothing there has a rollback that asks them for
    /// nothing while the stack still goes through `UPDATE_ROLLBACK_IN_PROGRESS` to
    /// `UPDATE_ROLLBACK_COMPLETE`. Nothing is opened only for a caller that asked for the
    /// failed state to be kept (`DisableRollback`), which is the one update that parks at
    /// `UPDATE_FAILED`.
    pub fn begin_owed_update_rollback(&mut self, now: SystemTime, reason: String) {
        if self.disable_rollback {
            // The caller asked for the failed state to be KEPT, so nothing is put back now
            // — and the plan is kept with it: `RollbackStack` is the call that clears such a
            // stack up, and it is the only record of what the failed update changed in the
            // owning services.
            return;
        }
        let plan = self.rollback_plan.take();
        self.begin_update_rollback(now, reason);
        // The rollback now in flight is this stack's OUTSTANDING one until it finishes: a
        // resource it is still waiting on can fail it later ([`Self::fail_stabilisation`]),
        // with no call of its own to work out what is left to put back, and the stack it
        // leaves in `UPDATE_ROLLBACK_FAILED` is one `ContinueUpdateRollback` resumes — which
        // asks the owning services, and a custom resource's provider, for the previous
        // configuration again. A rollback that COMPLETES is one those services carried out,
        // so [`Self::settle`] clears it there.
        self.unreverted_rollback = plan.clone();
        self.owed_rollback = plan;
    }

    /// Hold what the ROLLBACK of the update in flight would owe the services that own the
    /// stack's resources, for a failure that falls due with no call of its own to compute
    /// it.
    ///
    /// Called by the handler that drove the update's own calls to those services, once it
    /// has — only that handler knows what the update changed there. It says WHAT such a
    /// rollback has to put back, not whether one happens
    /// ([`Self::begin_owed_update_rollback`] decides that): a plan over resources the update
    /// left exactly as they were asks those services for nothing.
    pub fn hold_rollback_plan(&mut self, plan: Option<OwedRollback>) {
        self.rollback_plan = plan;
    }

    /// The stack's own record of the configuration in force on it right now
    /// ([`StackConfiguration`]).
    pub fn configuration(&self) -> StackConfiguration {
        StackConfiguration {
            template_body: self.template_body.clone(),
            template_description: self.template_description.clone(),
            processed_template: self.processed_template.clone(),
            processed_template_body: self.processed_template_body.clone(),
            parameters: self.parameters.clone(),
            tags: self.tags.clone(),
            notification_arns: self.notification_arns.clone(),
            capabilities: self.capabilities.clone(),
            role_arn: self.role_arn.clone(),
            rollback_configuration: self.rollback_configuration.clone(),
            deployment_config: self.deployment_config.clone(),
            resource_types: self.resource_types.clone(),
        }
    }

    /// Hold the configuration in force BEFORE the update that is being applied, so a
    /// failure of that update has the revision to go back to.
    ///
    /// Called by the handler that applies an update, with [`Self::configuration`] read
    /// before it writes any of those members onto the stack — an update replaces them where
    /// it starts, so the stack is the only place the previous revision was recorded.
    pub fn hold_previous_configuration(&mut self, configuration: Option<StackConfiguration>) {
        self.previous_configuration = configuration;
    }

    /// Put a configuration back on the stack: the whole revision, so every member a client
    /// reads back reports the one that is really deployed.
    fn restore_configuration(&mut self, configuration: StackConfiguration) {
        self.template_body = configuration.template_body;
        self.template_description = configuration.template_description;
        self.processed_template = configuration.processed_template;
        self.processed_template_body = configuration.processed_template_body;
        self.parameters = configuration.parameters;
        self.tags = configuration.tags;
        self.notification_arns = configuration.notification_arns;
        self.capabilities = configuration.capabilities;
        self.role_arn = configuration.role_arn;
        self.rollback_configuration = configuration.rollback_configuration;
        self.deployment_config = configuration.deployment_config;
        self.resource_types = configuration.resource_types;
    }

    /// Roll the stack's own configuration record back to the revision in force before the
    /// update that failed ([`Self::previous_configuration`]), where one is held.
    ///
    /// The counterpart, for the stack's own record, of abandoning the failed update's
    /// pending deployment: a rolled-back update deployed nothing, so what the stack reports
    /// as its configuration is the last revision that really was deployed.
    pub(super) fn roll_configuration_back(&mut self) {
        if let Some(configuration) = self.previous_configuration.take() {
            self.restore_configuration(configuration);
        }
    }

    /// The rollback this stack owes the owning services right now, taken for the caller
    /// that is about to perform it.
    pub fn take_owed_rollback(&mut self) -> Option<OwedRollback> {
        self.owed_rollback.take()
    }

    /// The resources a read of this stack reports RIGHT NOW.
    ///
    /// For a stack with no operation waiting at a wait condition that is what its last
    /// completed operation installed, which is what CloudFormation reports for a stack
    /// whose operation is still in flight: the resources of the template that finished
    /// deploying.
    ///
    /// For one that IS waiting — or that has been signalled and is completing — the
    /// operation has got as far as the wait condition: the resources it has already
    /// created are reported with the status that operation left them in, a wait condition
    /// still waiting is reported `*_IN_PROGRESS` (it is created and not finished), and the
    /// resources whose creation waits on one still waiting
    /// ([`wait_conditions::DeclaredWaitCondition::blocks`]) are not reported at all —
    /// CloudFormation reports a resource once its creation has STARTED, and these have
    /// not. A wait condition that HAS its signals, and what was waiting behind it, are
    /// reported as created, which is what the operation went on to do. An update's
    /// resources in force that the operation has not reached are reported as they stand,
    /// since it has not touched them.
    ///
    /// A resource whose owning service accepted the create and does not report it ready yet
    /// ([`stabilising`]) is reported exactly the same way, and that is the point of the
    /// wait: the resource IS reported, under the identifier that service already assigned,
    /// at `*_IN_PROGRESS` — so a client can resolve the resource through the owning service
    /// while the stack goes on waiting for it, and never reads it as complete before it is.
    /// A wait for a resource the operation DELETED reports nothing here: the stack stopped
    /// recording that physical resource when its owning service accepted the delete.
    pub fn reported_resources(&self) -> Vec<DeployedResource> {
        let Some(opened_at) = self
            .wait_conditions
            .iter()
            .map(|barrier| barrier.opened_at)
            .chain(self.coming_up().map(|barrier| barrier.opened_at))
            .min()
        else {
            return self.resources.clone();
        };
        let Some(deployment) = self.pending_deployment.as_ref() else {
            return self.resources.clone();
        };
        let unsatisfied: Vec<&wait_conditions::WaitConditionBarrier> = self
            .wait_conditions
            .iter()
            .filter(|barrier| !self.barrier_satisfied(barrier))
            .collect();
        let started: Vec<StoredResource> = deployment
            .resources
            .iter()
            .filter(|resource| {
                !unsatisfied
                    .iter()
                    .any(|barrier| barrier.blocks(&resource.logical_id))
            })
            .cloned()
            .collect();
        let mut reported =
            installed_resources(&self.resources, started, opened_at, &self.resource_finished_instants());
        for deployed in &mut reported {
            // A wait condition is in the status of the OPERATION that is waiting at it — it
            // is a resource that operation is creating or updating — and so is a resource
            // held for the answer that finishes the work on it: a resource the operation is
            // bringing INTO the stack is being created, and one the stack already holds is
            // being updated (which is what a CUSTOM RESOURCE held for its provider's answer
            // to an `Update` is), by the same rule [`Self::in_progress_status_of`] applies to
            // every other resource.
            let waiting_at = unsatisfied
                .iter()
                .find(|barrier| barrier.logical_id() == deployed.resource.logical_id)
                .map(|barrier| {
                    (
                        barrier.opened_at,
                        self.in_progress_status_of(&deployed.resource.logical_id),
                    )
                });
            let gated = waiting_at.or_else(|| {
                self.coming_up()
                    .find(|barrier| {
                        !barrier.released()
                            && barrier.logical_id == deployed.resource.logical_id
                    })
                    .map(|barrier| {
                        (
                            barrier.opened_at,
                            self.in_progress_status_of(&deployed.resource.logical_id),
                        )
                    })
            });
            if let Some((gated_at, in_progress)) = gated {
                deployed.status = in_progress.to_string();
                deployed.status_reason = Some(initiated_reason(in_progress).to_string());
                deployed.status_timestamp = gated_at;
            }
        }
        // What the stack already had and this operation has not reached keeps the status
        // it carries: the operation has not touched it.
        for deployed in &self.resources {
            if !reported
                .iter()
                .any(|shown| shown.resource.logical_id == deployed.resource.logical_id)
            {
                reported.push(deployed.clone());
            }
        }
        reported
    }

    /// The resource-level status of a resource the operation in flight has STARTED and not
    /// finished.
    ///
    /// `StackResource$ResourceStatus` is "Current status of the resource", so it names what
    /// is being done to THAT resource rather than what the stack-level operation is: a
    /// resource the operation is bringing into the stack is being CREATED, whether the
    /// operation around it is a create or an update, and only one the stack already holds is
    /// being UPDATED. It is the same rule [`installed_resources`] applies once the operation
    /// finishes, read while it is still under way.
    ///
    /// A record the stack kept at `DELETE_COMPLETE` is not a resource the stack holds: the
    /// operation that disposed of it took it away, so a deployment declaring that logical id
    /// again creates the resource.
    fn in_progress_status_of(&self, logical_id: &str) -> &'static str {
        let held = self.resources.iter().any(|deployed| {
            deployed.resource.logical_id == logical_id
                && deployed.status != resource_status::DELETE_COMPLETE
        });
        if held {
            resource_status::UPDATE_IN_PROGRESS
        } else {
            resource_status::CREATE_IN_PROGRESS
        }
    }

    /// The resources the stack has RIGHT NOW: the ones the operation in flight is about to
    /// install while it is still in flight, else the ones its last completed operation
    /// left. This is the set a stack's deletion has to delete.
    pub fn current_resources(&self) -> Vec<StoredResource> {
        match &self.pending_deployment {
            Some(deployment) => deployment.resources.clone(),
            None => self
                .resources
                .iter()
                .map(|deployed| deployed.resource.clone())
                .collect(),
        }
    }

    /// The logical ids of the resources the stack HOLDS a record of right now — the ones a
    /// teardown of it is RESPONSIBLE for, whether or not any owning service has something to
    /// delete for them.
    ///
    /// The same set [`disposed_resources`] keeps records for: every resource the stack has
    /// installed and has not already disposed of, plus every resource the operation in
    /// flight has ACTED ON ([`PendingDeployment::attempted`]). So what a teardown reports
    /// per resource ([`provisioning::delete`]) and what the stack goes on reporting
    /// afterwards are one set — a declaration an operation never reached is in neither,
    /// because nothing was brought into being under it, and a record already at
    /// `DELETE_COMPLETE` is in neither either, because the operation that disposed of it
    /// already reported that.
    pub fn held_resource_ids(&self) -> BTreeSet<String> {
        let mut held: BTreeSet<String> = self
            .resources
            .iter()
            .filter(|deployed| deployed.status != resource_status::DELETE_COMPLETE)
            .map(|deployed| deployed.resource.logical_id.clone())
            .collect();
        if let Some(deployment) = &self.pending_deployment {
            held.extend(deployment.attempted.iter().cloned());
        }
        held
    }

    /// What the ROLLBACK of a failed operation on this stack does with the `DeletionPolicy`
    /// of a resource that operation had created.
    ///
    /// `RetainExceptOnCreate` is what chooses: "when set to `true`, newly created resources
    /// are deleted when the operation rolls back. This includes newly created resources
    /// marked with a deletion policy of `Retain`" — so under it the rollback deletes them
    /// whatever they declare, and under the documented default (`false`) each resource's own
    /// policy stands, exactly as it does when the stack itself is deleted.
    pub fn rolled_back_resource_policies(&self) -> provisioning::Policies {
        match self.retain_except_on_create {
            Some(true) => provisioning::Policies::Ignored,
            _ => provisioning::Policies::Honoured,
        }
    }

    /// How many resources the stack OCCUPIES right now, for the account-wide resource
    /// ceiling ([`ComAmazonawsCloudformationCloudformationState::current_resource_count`]).
    ///
    /// Not quite [`Self::current_resources`] counted: a record an operation that DISPOSED
    /// of the resource left behind, at `DELETE_COMPLETE`, is still REPORTED (and still
    /// carried by `current_resources`, which is the set a deletion walks) but is NOT
    /// counted, for the same reason a deleted stack's records are not — the stack goes on
    /// naming what became of that resource, while nothing of it exists to occupy the
    /// account's concurrent-resource quota. What bounds the stored set is the per-template
    /// resource ceiling ([`template::MAX_RESOURCES_PER_TEMPLATE`]) over the stacks the
    /// store keeps ([`MAX_STACKS`], [`MAX_LIVE_STACKS`]).
    pub fn current_resource_count(&self) -> usize {
        match &self.pending_deployment {
            Some(deployment) => deployment.resources.len(),
            None => self
                .resources
                .iter()
                .filter(|deployed| deployed.status != resource_status::DELETE_COMPLETE)
                .count(),
        }
    }

    /// What CloudFormation records on, and propagates to, every resource it provisions for
    /// this stack: the stack's identity and the stack-level `Tags` in force
    /// ([`provisioning::StackProvenance`]).
    ///
    /// Read off the stack under the store lock, so the deployment that follows — which runs
    /// with the lock released — carries the tags the stack really has rather than a second
    /// idea of them.
    pub fn provenance(&self) -> provisioning::StackProvenance {
        provisioning::StackProvenance {
            stack_name: self.stack_name.clone(),
            stack_id: self.stack_id.clone(),
            tags: self
                .tags
                .iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect(),
            capabilities: self.capabilities.clone(),
            // The mode the stack's deployments run in, which is part of how a failure of
            // these resources is reported ([`provisioning::StackProvenance::express`]).
            express: super::stored_deployment_mode(self.deployment_config.as_ref())
                == Some(super::DEPLOYMENT_MODE_EXPRESS),
            // A deployment reverts a resource whose change it failed only where it is one
            // that CHANGES resources the stack already has: the operation deploying such a
            // template is what says so, and how that revert is reported
            // ([`provisioning::FailedResourceRevert`]).
            failed_resource_revert: provisioning::FailedResourceRevert::None,
            // A stack holds no record of another stack's resources, so the index of which
            // stack manages which physical name is the STORE's to read: the operation that
            // deploys this stack fills it in
            // ([`provisioning::StackProvenance::managed_names`]), and a deployment that
            // re-creates only this stack's own resources leaves it empty.
            managed_names: super::physical_name::ManagedNames::default(),
            // What the deployment in flight declares each resource waits for, which is what
            // says whether a resource can be created yet
            // ([`provisioning::StackProvenance::dependencies`]).
            dependencies: provisioning::dependencies_in_flight(self),
        }
    }

    /// Replace the resources of [`Self::current_resources`] with what the services that
    /// own them answered — the identifier each one minted, the attributes they reported,
    /// and whether each resource was really created there.
    ///
    /// For an operation still in flight that is the pending deployment, which
    /// [`Self::settle`] installs when the operation completes. For one that has already
    /// settled (a deletion acting on a stack whose create completed), the installed
    /// records are rewritten in place, each keeping the resource STATUS and timestamp its
    /// deployment gave it: nothing about a resource's status changed because its
    /// provisioning record did.
    ///
    /// The stack's outputs are evaluated again over the recorded set
    /// ([`Self::reevaluate_outputs`]): for a type whose physical id the owning service
    /// ASSIGNS, the identity in these records is the one it minted rather than the one the
    /// naming rules computed, and an output naming that resource reports what a client can
    /// call that service with.
    pub fn record_materialised_resources(&mut self, resources: Vec<StoredResource>) {
        if let Some(deployment) = &mut self.pending_deployment {
            deployment.resources = resources;
            self.reevaluate_outputs();
            return;
        }
        for resource in resources {
            match self
                .resources
                .iter_mut()
                .find(|deployed| deployed.resource.logical_id == resource.logical_id)
            {
                Some(deployed) => deployed.resource = resource,
                // A resource the stack did not have installed under that logical id: it
                // takes the status of the operation that recorded it.
                None => self.resources.push(DeployedResource::new(
                    resource,
                    resource_status::CREATE_COMPLETE,
                    SystemTime::now(),
                )),
            }
        }
        self.reevaluate_outputs();
    }

    /// Record the attributes the service owning the resource `logical_id` names publishes for
    /// it now that it has finished coming UP, and evaluate the stack's outputs again over
    /// them.
    ///
    /// A resource whose owning service reports it still being created does not publish every
    /// identifier it will have: an Amazon OpenSearch Service domain has no endpoint "used to
    /// submit index, search, and data upload requests" until the cluster behind it is
    /// running, so the value `Fn::GetAtt` reads for that endpoint exists only once that
    /// service reports the domain up. CloudFormation resolves a stack's outputs for a
    /// resource it reports CREATE_COMPLETE, which is the instant this records — so an output
    /// over such an attribute carries the value the owning service publishes rather than
    /// nothing, exactly as [`Self::record_materialised_resources`] does for an identity that
    /// service assigned.
    ///
    /// Written onto the resource records of the operation IN FLIGHT where there is one, since
    /// those are what [`Self::settle`] installs when it completes. An attribute already
    /// carrying that value changes nothing, so a repeated read evaluates nothing again. The
    /// set stays under the same ceiling as every other resource's
    /// ([`template::MAX_ATTRIBUTES_PER_RESOURCE`]).
    pub fn record_ready_attributes(
        &mut self,
        logical_id: &str,
        attributes: &BTreeMap<String, String>,
    ) {
        if attributes.is_empty() {
            return;
        }
        let Some(held) = self.recorded_resource_mut(logical_id) else {
            return;
        };
        let mut changed = false;
        for (name, value) in attributes {
            if held.attributes.get(name) == Some(value) {
                continue;
            }
            if !held.attributes.contains_key(name)
                && held.attributes.len() >= template::MAX_ATTRIBUTES_PER_RESOURCE
            {
                continue;
            }
            held.attributes.insert(name.clone(), value.clone());
            changed = true;
        }
        if changed {
            self.reevaluate_outputs();
        }
    }

    /// Record the identity the PROVIDER of a custom resource answered with as the one the
    /// resource is known by.
    ///
    /// A custom resource's physical id is the provider's to decide — "a required custom
    /// resource provider-defined physical ID that is unique for that provider" — so the
    /// identifier the naming rules computed for the record is a stand-in until the provider
    /// answers, and what `Ref` resolves to from then on is this one. Written onto the records
    /// of the operation IN FLIGHT where there is one, since those are what [`Self::settle`]
    /// installs; an id already carrying this value changes nothing.
    pub fn adopt_provider_identity(&mut self, logical_id: &str, physical_resource_id: &str) {
        if physical_resource_id.is_empty() {
            return;
        }
        let Some(held) = self.recorded_resource_mut(logical_id) else {
            return;
        };
        if held.physical_resource_id == physical_resource_id {
            return;
        }
        held.physical_resource_id = physical_resource_id.to_string();
        // An output whose value is a `Ref` to this resource resolved to the stand-in, so what
        // the stack publishes is re-evaluated against the identity it really has.
        self.reevaluate_outputs();
    }

    /// The record the stack keeps of the resource `logical_id`, for a writer that is
    /// recording what became of it: the one of the operation IN FLIGHT where there is one,
    /// since those are what [`Self::settle`] installs, and the installed record otherwise.
    ///
    /// One reading of "which record is this resource's", so every writer below updates the
    /// record the same read would report.
    fn recorded_resource_mut(&mut self, logical_id: &str) -> Option<&mut StoredResource> {
        match &mut self.pending_deployment {
            Some(deployment) => deployment
                .resources
                .iter_mut()
                .find(|resource| resource.logical_id == logical_id),
            None => self
                .resources
                .iter_mut()
                .map(|deployed| &mut deployed.resource)
                .find(|resource| resource.logical_id == logical_id),
        }
    }

    /// Record that NOTHING was created anywhere for the resource `logical_id`, so no teardown
    /// of it owes anybody a call.
    ///
    /// A CUSTOM RESOURCE is brought into being by the answer its provider PUTs to the request
    /// CloudFormation sent it and by nothing else — "a required custom resource
    /// provider-defined physical ID that is unique for that provider" is what the resource
    /// IS — so a request no provider ever answered leaves nothing behind: the only identifier
    /// the record carries is the stand-in CloudFormation computed for it, which addresses no
    /// resource any provider minted. The record stops claiming the resource was created, and
    /// the teardown of the failed operation then reports its removal exactly as it does for
    /// every other resource nothing was created for ([`provisioning::delete`]) rather than
    /// asking a provider to delete a resource that does not exist.
    fn discard_uncreated_resource(&mut self, logical_id: &str) {
        if let Some(held) = self.recorded_resource_mut(logical_id) {
            held.materialised = false;
        }
    }

    /// Report that the update in flight has applied every change and is now removing the
    /// resources it REPLACED.
    ///
    /// "UPDATE_COMPLETE_CLEANUP_IN_PROGRESS — the stack update was successful, and
    /// CloudFormation is deleting the old resources": the status a client polling the update
    /// reads while that cleanup runs, which is what says the change itself landed. The
    /// operation's own terminal status is untouched — it is still the one the update settles
    /// into once the cleanup is through.
    pub fn report_update_cleanup(&mut self, now: SystemTime) {
        if self.status == status::UPDATE_COMPLETE_CLEANUP_IN_PROGRESS {
            return;
        }
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if pending.status != status::UPDATE_COMPLETE {
            return;
        }
        let operation_id = pending.operation_id.clone();
        let client_request_token = pending.client_request_token.clone();
        self.status = status::UPDATE_COMPLETE_CLEANUP_IN_PROGRESS.to_string();
        self.status_reason = None;
        self.record_event(
            now,
            status::UPDATE_COMPLETE_CLEANUP_IN_PROGRESS,
            None,
            operation_id,
            client_request_token,
        );
    }

    /// Open a deletion CloudFormation CANCELS: the attempt runs and then puts the stack
    /// back where it was.
    ///
    /// "Once the call completes successfully, stack deletion starts": the call is taken and
    /// the stack reports `DELETE_IN_PROGRESS` ("User Initiated") like any other delete, and
    /// the condition that makes the deletion impossible is reported by the workflow rather
    /// than by the call. CloudFormation then cancels it, so the stack settles back into the
    /// status it held before the attempt — a HEALTHY stack, not a failed one, because
    /// nothing was touched: a cancelled deletion disposed of nothing, so every resource and
    /// every exported output value it published is still there, and the identical call
    /// succeeds once what blocked it is gone. `reason` is what the restored status carries,
    /// which is where a client polling the deletion reads that it was cancelled and why.
    ///
    /// The restored status is terminal in the sense a poller needs — it is not an
    /// `*_IN_PROGRESS` one — so a caller waiting on the deletion has something to stop on.
    pub fn begin_canceled_deletion(
        &mut self,
        now: SystemTime,
        reason: String,
        operation_id: Option<String>,
        client_request_token: Option<String>,
    ) {
        // The status the stack is HEALTHY in is the one it goes back to, read before the
        // attempt installs its own: the status it holds, or — when an operation of its own
        // was still in flight when the cancelled delete arrived, and so will never settle —
        // the one that operation was going to reach.
        let restored = match self.pending.as_ref() {
            Some(pending) if !pending.status.ends_with(IN_PROGRESS_SUFFIX) => {
                pending.status.clone()
            }
            _ => self.status.clone(),
        };
        self.begin_transition(
            now,
            status::DELETE_IN_PROGRESS,
            &restored,
            operation_id,
            client_request_token,
        );
        // The restored status says WHY the deletion was cancelled, which is where a client
        // polling the delete reads what blocked it.
        if let Some(pending) = self.pending.as_mut() {
            pending.status_reason = Some(reason);
        }
    }

    /// Leave each of the stack's resource records in the status the DELETION left it in.
    ///
    /// `StackResource$ResourceStatus` is the resource's "Current status", so once a deletion
    /// an owning service refused has ended, the resource it refused reports the failed
    /// delete status and that service's refusal, one the same walk DID delete reports its
    /// deletion as complete, and one whose `DeletionPolicy` kept it — nothing was asked of
    /// the service that owns it — reports its deletion as skipped: the per-resource view and
    /// the event stream of one deletion cannot disagree about what became of each resource. A
    /// record the walk never reached — one it stopped before, or one no owning service holds
    /// — keeps the status it carried, because nothing happened to that resource.
    ///
    /// A deletion that COMPLETES disposes of every record at once
    /// ([`disposed_resources`]); this is the counterpart for the one that could not.
    pub fn record_resource_deletion_statuses(
        &mut self,
        transitions: &[provisioning::ResourceTransition],
        at: SystemTime,
    ) {
        self.record_resource_statuses(transitions, at);
    }

    /// Leave each of the stack's resource records in the status one walk left it in, for
    /// every transition of that walk which ENDED.
    ///
    /// `StackResource$ResourceStatus` is the resource's "Current status", so what a walk did
    /// to a resource is what that resource then reports — the one rule both the deletion
    /// above and the rollback below are an application of, so the two cannot drift about what
    /// a resource's current status is. A transition still IN PROGRESS is not what became of
    /// the resource (the walk that opened it is what concludes it), and a record no
    /// transition names keeps the status it carried, because that walk did not act on it.
    fn record_resource_statuses(
        &mut self,
        transitions: &[provisioning::ResourceTransition],
        at: SystemTime,
    ) {
        for transition in transitions {
            if transition.status.ends_with(IN_PROGRESS_SUFFIX) {
                continue;
            }
            self.record_resource_status(
                &transition.logical_id,
                &transition.status,
                transition.reason.clone(),
                at,
            );
        }
    }

    /// Leave ONE resource's record in `status` as of `at`, with the reason that status came
    /// with — the single write every caller above makes per resource, and the one place a
    /// resource's current status is set.
    ///
    /// Also what a wait that could not be carried through writes
    /// ([`Self::fail_stabilisation`]): the walk that opened the wait had recorded the outcome
    /// it PLANNED, so without this the record would go on stating work a provider — or the
    /// service that owns the resource — then refused. Writing both through one function is
    /// what keeps the per-resource views and that resource's event stream agreeing.
    ///
    /// A logical id this stack holds no record for is left alone: there is no resource to
    /// report a status for.
    fn record_resource_status(
        &mut self,
        logical_id: &str,
        status: &str,
        reason: Option<String>,
        at: SystemTime,
    ) {
        let Some(deployed) = self
            .resources
            .iter_mut()
            .find(|deployed| deployed.resource.logical_id == logical_id)
        else {
            return;
        };
        deployed.status = status.to_string();
        deployed.status_reason = reason;
        deployed.status_timestamp = at;
    }

    /// Leave each of the stack's resource records in the status a ROLLBACK left it in.
    ///
    /// Once a rollback has ended, a resource it put back reports the status of that revert
    /// and a resource the owning service would NOT put back reports the failed one with that
    /// service's own refusal — which is also the set `ContinueUpdateRollback` reads: "you can
    /// specify only resources that are in the UPDATE_FAILED state because a rollback failed".
    /// Only the transitions that ENDED are applied, through the one writer of a resource's
    /// current status ([`Self::record_resource_statuses`]).
    pub fn record_rollback_resource_statuses(
        &mut self,
        transitions: &[provisioning::ResourceTransition],
        at: SystemTime,
    ) {
        self.record_resource_statuses(transitions, at);
    }

    /// Leave each of the stack's resource records in the status a deployment that FAILED
    /// left it in.
    ///
    /// `StackResource$ResourceStatus` is the resource's "Current status", and a failed
    /// operation is not an operation that did nothing: the resource whose change its provider
    /// refused, and every resource the operation had already changed before it, report what
    /// became of them — which is what a client reads to see how far the deployment got and
    /// which resource stopped it. A resource the operation never touched keeps the status it
    /// carried, since nothing happened to it.
    ///
    /// Applied through the one writer of a resource's current status
    /// ([`Self::record_resource_statuses`]), so what the per-resource views report and what
    /// the event stream of that operation says cannot disagree.
    pub fn record_failed_resource_statuses(
        &mut self,
        transitions: &[provisioning::ResourceTransition],
        at: SystemTime,
    ) {
        self.record_resource_statuses(transitions, at);
    }
}

/// Whether a stack-level EVENT reports the failed status a failed transition records.
///
/// CloudFormation's event stream is what the stack DID: a failure the stack rests in is an
/// event of its own, while a failure whose rollback begins at the same instant is reported
/// by that rollback's event instead — the stack moved straight from the operation's
/// `*_IN_PROGRESS` into `ROLLBACK_IN_PROGRESS` / `UPDATE_ROLLBACK_IN_PROGRESS` and never
/// rested in the failed status for an event to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureReport {
    /// The stack rests in the failed status: the event log carries it.
    Rested,
    /// A rollback or a removal supersedes it at the same instant: no event of its own.
    Superseded,
    /// A PRE-DEPLOYMENT VALIDATION failed the operation: that stage ran before anything was
    /// asked of any owning service, so NO resource has an event that could carry the
    /// failure and the stack's own event is the one report of it — whether or not the
    /// rollback it owes begins at this instant. `rests` is whether the stack stays in the
    /// failed status, as it is for the two reports above.
    FailedValidation { rests: bool },
}

impl StoredStack {
    /// Record that the operation in flight could not be carried out, and that the stack
    /// RESTS in the failed status: it takes that status with the reason, the clock is owed
    /// nothing more, and the event log carries the failure.
    ///
    /// This is how CloudFormation reports a stack whose resources an owning service
    /// refused to create or delete and that is left in the failed state
    /// (`CREATE_FAILED` / `DELETE_FAILED`) — the operation
    /// does not stay pending and no terminal status it never reached is reported.
    ///
    /// [`Self::fail_transition_superseded`] is the other report: the same failure, for a
    /// stack that moves into its rollback at this very instant.
    ///
    /// The operation CONCLUDED at `now`, which for a failure an owning service answered is
    /// sooner than the span the walk over the resources was laid out across: the walk is
    /// therefore compressed into the span the operation really took, so what it had done is
    /// recorded in the order it did it, at its own instants, ahead of the failure that ends
    /// it ([`timeline::Timeline::compress_into`]).
    pub fn fail_transition(&mut self, now: SystemTime, failed_status: &str, reason: String) {
        self.record_failure(now, failed_status, reason, FailureReport::Rested);
    }

    /// Record a failure of the operation in flight that the stack does NOT rest in, because
    /// the removal that failure owes begins at the same instant — a create CloudFormation
    /// rolls back, a create it takes the stack away for, or an update it reverts.
    ///
    /// Everything [`Self::fail_transition`] records is recorded: the stack takes the failed
    /// status with the reason and the attempt is concluded as failed at `now`, which is what
    /// the removal that follows is then opened over. What is NOT recorded is a stack-level
    /// EVENT for that status: the stack moved straight from the operation's
    /// `*_IN_PROGRESS` into the rollback — whose own event carries the failure and the
    /// clause saying the rollback was asked for ([`Self::begin_rollback`]) — so no event
    /// reports a status the stack never rested in. The resources' own failure events are
    /// untouched: WHICH resource failed, and why, is reported on that resource, and a
    /// failed status the stack does rest in (`DisableRollback`, `OnFailure=DO_NOTHING`)
    /// still gets its event from [`Self::fail_transition`].
    pub fn fail_transition_superseded(
        &mut self,
        now: SystemTime,
        failed_status: &str,
        reason: String,
    ) {
        self.record_failure(now, failed_status, reason, FailureReport::Superseded);
    }

    /// Record a failure of the operation in flight that a PRE-DEPLOYMENT VALIDATION reached:
    /// the stage runs before anything is asked of any owning service, so no resource of this
    /// stack has an event that could carry the failure and the stack's own event is the one
    /// report of it.
    ///
    /// Everything the two reports above record is recorded, and `rests` says which of them
    /// this is: a stack that keeps its failed state (`DisableRollback`,
    /// `OnFailure=DO_NOTHING`) rests in the failed status, and one whose rollback begins at
    /// this instant does not. Either way the event is recorded, carrying the detailed status
    /// the model gives a validation failure (`VALIDATION_FAILED`, "template validation failed
    /// because of invalid properties in the template", whose `ResourceStatusReason` is what
    /// failed it) — and the validation's own detail stays on the result event
    /// `DescribeEvents` reports ([`Self::record_validation_failure`]).
    pub fn fail_transition_on_validation(
        &mut self,
        now: SystemTime,
        failed_status: &str,
        reason: String,
        rests: bool,
    ) {
        self.record_failure(
            now,
            failed_status,
            reason,
            FailureReport::FailedValidation { rests },
        );
    }

    /// The one writer of a failed transition, for every one of the reports above.
    fn record_failure(
        &mut self,
        now: SystemTime,
        failed_status: &str,
        reason: String,
        report: FailureReport,
    ) {
        // Whether the stack STAYS in the failed status, which is what decides the resources
        // it goes on reporting, and whether an event of its own reports that status.
        let rests = matches!(
            report,
            FailureReport::Rested | FailureReport::FailedValidation { rests: true }
        );
        // A CREATE whose failed state the stack RESTS in goes on reporting the resources it
        // had made ([`Self::keep_failed_resources`]), and what the walk left each of them in
        // is read off that walk — BEFORE the flush below retires it.
        let creating = self.status == status::CREATE_IN_PROGRESS;
        let walked = if creating && rests {
            self.walked_transitions()
        } else {
            Vec::new()
        };
        self.flush_resource_timeline(now);
        let pending = self.pending.take();
        // An operation that failed waits for nothing: whatever it was waiting at, it is
        // not going to proceed on a signal.
        self.wait_conditions.clear();
        self.stabilisations.clear();
        self.status = failed_status.to_string();
        self.status_reason = Some(reason.clone());
        let operation_id = pending.as_ref().and_then(|p| p.operation_id.clone());
        // The attempt FAILED at this instant, whatever its failure then owes.
        self.conclude_operation(operation_id.as_deref(), failed_status, now);
        // A create that failed is not a create that did nothing: where the failed state
        // STAYS — the caller asked for it to be kept, or nothing follows the failure — the
        // resources the create had already made go on being reported, each in the status
        // the walk left it. A create whose rollback begins at this instant installs nothing,
        // because that rollback removes what it had made.
        if creating && rests {
            self.keep_failed_resources(&walked, now);
        }
        if report == FailureReport::Superseded {
            return;
        }
        self.record_event(
            now,
            failed_status,
            Some(reason),
            operation_id,
            pending.and_then(|p| p.client_request_token),
        );
        // ...and where the PRE-DEPLOYMENT VALIDATION stage is what failed the operation, the
        // event just recorded is the one that reports it: it carries that detailed status
        // ([`super::detailed_status_of`]).
        if matches!(report, FailureReport::FailedValidation { .. }) {
            if let Some(recorded) = self.events.back_mut() {
                recorded.failed_validation = true;
            }
        }
    }

    /// Install the resources a FAILED create made, for a create whose failed state the
    /// caller asked to keep (`OnFailure=DO_NOTHING`, which `DisableRollback` also spells).
    ///
    /// A create that failed is not a create that did nothing: CloudFormation reports every
    /// resource whose creation it STARTED, each in the status the walk left it — the ones
    /// their owning services created as `CREATE_COMPLETE`, and the one a service refused as
    /// `CREATE_FAILED` — which is what a client reads to see how far the deployment got and
    /// which resource stopped it. A create that is ROLLED BACK instead installs nothing,
    /// because the rollback removes what it had made, so this is only called where the failed
    /// state stays.
    ///
    /// Each resource's status is the LAST transition the walk recorded for it
    /// ([`provisioning::ResourceTransition`]), so no status is decided here; a declaration the
    /// walk never reached has no transition and is not reported at all, exactly as
    /// CloudFormation reports a resource only once its creation has started. The pending
    /// deployment itself is LEFT in place, since it is also what the stack's own deletion
    /// deletes ([`Self::current_resources`]).
    ///
    /// That transition's REASON comes with its status: "Success/failure message associated
    /// with the resource" is where the owning service's refusal of the resource survives into
    /// the per-resource views, so the resource whose failure stopped the create reports why it
    /// failed and not merely that it did. A transition with nothing to explain carries no
    /// reason, so a resource in a success status reports none — the member is then absent, as
    /// it is for every other resource whose status speaks for itself.
    pub fn keep_failed_resources(
        &mut self,
        transitions: &[provisioning::ResourceTransition],
        now: SystemTime,
    ) {
        let Some(deployment) = self.pending_deployment.as_ref() else {
            return;
        };
        let finished_at = self.resource_finished_instants();
        let installed: Vec<DeployedResource> = deployment
            .resources
            .iter()
            .filter_map(|resource| {
                let status = transitions
                    .iter()
                    .rev()
                    .find(|transition| transition.logical_id == resource.logical_id)?;
                let at = finished_at
                    .get(&resource.logical_id)
                    .copied()
                    .unwrap_or(now);
                let mut record = DeployedResource::new(resource.clone(), &status.status, at);
                record.status_reason = status.reason.clone();
                Some(record)
            })
            .collect();
        self.resources = installed;
        self.reevaluate_outputs();
    }

    /// The transitions the walk in flight planned for the resources it ACTED ON, oldest
    /// first — the same records that walk was built from
    /// ([`Self::schedule_resource_events`]), read back so a failure that has no transitions
    /// of its own to hand still knows what the walk left each resource in.
    ///
    /// A failure the CLOCK materialises (a resource-signal wait that ran out, a resource an
    /// owning service never got ready) is recorded with the store locked and no walk's answer
    /// beside it, while the handler that drove the walk has those answers in hand. Both leave
    /// the stack in the same state, so both install the kept resources from the same records:
    /// this walk is the one place that still says which resources the operation got to and
    /// what became of each.
    fn walked_transitions(&self) -> Vec<provisioning::ResourceTransition> {
        let Some(walk) = self.resource_timeline.as_ref() else {
            return Vec::new();
        };
        walk.steps
            .iter()
            .flat_map(|step| {
                step.events.iter().map(|event| provisioning::ResourceTransition {
                    logical_id: step.logical_id.clone(),
                    resource_type: step.resource_type.clone(),
                    physical_resource_id: event.physical_resource_id.clone(),
                    status: event.status.clone(),
                    reason: event.reason.clone(),
                    resource_properties: event.resource_properties.clone(),
                })
            })
            .collect()
    }

    /// Open the ROLLBACK a failed create owes: the stack moves to `ROLLBACK_IN_PROGRESS`
    /// carrying the reason the create failed, and `ROLLBACK_COMPLETE` becomes due one
    /// [`STATUS_TRANSITION_DELAY`] later.
    ///
    /// "ROLLBACK — Roll back the stack creation. This is the default"
    /// (`CreateStackInput$OnFailure`), so a create whose resources an owning service
    /// refused does not stop at `CREATE_FAILED`: CloudFormation removes what the create
    /// had made and the stack reports the rolled-back status. A request that asked for the
    /// failed state to be kept (`DisableRollback`, `OnFailure=DO_NOTHING`) never reaches
    /// here — the caller said what it wanted left behind.
    ///
    /// Called AFTER [`Self::fail_transition`] recorded the create's own failure, so the
    /// event log carries the failure and then the rollback, in that order. The operation
    /// this rollback belongs to is the create's, which is what the stack already reports as
    /// its last operation, and REMOVING the resources is the caller's to drive
    /// ([`provisioning::delete`]) before the terminal status falls due.
    pub fn begin_rollback(&mut self, now: SystemTime, reason: String) {
        self.status = status::ROLLBACK_IN_PROGRESS.to_string();
        self.status_reason = Some(reason.clone());
        let operation_id = self
            .last_operation()
            .map(|operation| operation.operation_id.clone());
        let client_request_token = self.client_request_token.clone();
        self.record_event(
            now,
            status::ROLLBACK_IN_PROGRESS,
            // What the EVENT says is what the rollback is: the failure that caused it, and
            // that CloudFormation rolled the create back because that is what `OnFailure`
            // asked for — the clause the summary sentence leaves room for
            // ([`rollback_requested`]). The stack's own `StackStatusReason` keeps the
            // failure alone, which is the field a client parses to recover the set of failed
            // logical ids ([`resource_failure_summary`]).
            Some(rollback_requested(&reason)),
            operation_id.clone(),
            client_request_token.clone(),
        );
        self.pending = Some(PendingStatus {
            // The rolled-back stack keeps the reason its create failed for: that is what
            // the stack is in this status for, and a client reading it afterwards has
            // nothing else to go on.
            status: status::ROLLBACK_COMPLETE.to_string(),
            status_reason: Some(reason),
            started_at: now,
            due_at: now + STATUS_TRANSITION_DELAY,
            operation_id,
            client_request_token,
        });
    }

    /// Open the ROLLBACK a failed UPDATE owes: the stack moves to
    /// `UPDATE_ROLLBACK_IN_PROGRESS` carrying the reason the update failed, and
    /// `UPDATE_ROLLBACK_COMPLETE` becomes due one [`STATUS_TRANSITION_DELAY`] later.
    ///
    /// CloudFormation "rolls back the stack to the previous stack configuration" when an
    /// update fails: the update's own deployment is ABANDONED here — a failed update
    /// installs nothing, so the stack keeps the resources, outputs and template stage its
    /// last completed operation left — and putting back in the owning services what the
    /// update had already changed there is the caller's to drive
    /// ([`provisioning::reconcile`], against the resources in force) before the terminal
    /// status falls due. A revert an owning service refuses leaves the stack
    /// `UPDATE_ROLLBACK_FAILED` ([`Self::fail_transition`]), which is the state
    /// `ContinueUpdateRollback` resumes from.
    ///
    /// Called AFTER [`Self::fail_transition`] recorded the update's own failure, so the
    /// event log carries the failure and then the rollback, in that order.
    pub fn begin_update_rollback(&mut self, now: SystemTime, reason: String) {
        // A failed update deploys nothing: its pending deployment is what the stack would
        // have reported had it completed, and the rollback is the decision not to.
        self.pending_deployment = None;
        // ...and the stack's OWN record goes back with it: the template, the parameters and
        // the rest of the revision the update replaced are the previous stack
        // configuration, and the values the failed update submitted are discarded with that
        // update ([`Self::previous_configuration`]).
        self.roll_configuration_back();
        self.status = status::UPDATE_ROLLBACK_IN_PROGRESS.to_string();
        self.status_reason = Some(reason.clone());
        let operation_id = self
            .last_operation()
            .map(|operation| operation.operation_id.clone());
        let client_request_token = self.client_request_token.clone();
        self.record_event(
            now,
            status::UPDATE_ROLLBACK_IN_PROGRESS,
            Some(reason.clone()),
            operation_id.clone(),
            client_request_token.clone(),
        );
        self.pending = Some(PendingStatus {
            status: status::UPDATE_ROLLBACK_COMPLETE.to_string(),
            // The rolled-back stack keeps the reason its update failed for: that is what
            // the stack is in this status for.
            status_reason: Some(reason),
            started_at: now,
            due_at: now + STATUS_TRANSITION_DELAY,
            operation_id,
            client_request_token,
        });
    }

    /// Resume the rollback of a failed update, leaving the resources `skipped` names
    /// exactly as the failed update made them.
    ///
    /// `ContinueUpdateRollback` "continues rolling back a stack from
    /// UPDATE_ROLLBACK_FAILED to UPDATE_ROLLBACK_COMPLETE state", and of the resources the
    /// request names: "CloudFormation sets the status of the specified resources to
    /// UPDATE_COMPLETE and continues to roll back the stack" — so each skipped resource
    /// keeps the configuration it has and takes that status, with the event that says so and
    /// the reason that says WHY it is complete without having been put back
    /// ([`RESOURCE_SKIPPED_DURING_UPDATE_ROLLBACK`]), while the stack moves to
    /// `UPDATE_ROLLBACK_IN_PROGRESS` and reaches `UPDATE_ROLLBACK_COMPLETE` one
    /// [`STATUS_TRANSITION_DELAY`] later.
    pub fn continue_update_rollback(
        &mut self,
        now: SystemTime,
        skipped: &[String],
        client_request_token: Option<String>,
    ) {
        let operation_id = new_operation_id();
        // The resumption is the stack's own operation, of the type the model names for it:
        // "continues rolling back a stack from UPDATE_ROLLBACK_FAILED to
        // UPDATE_ROLLBACK_COMPLETE state".
        self.record_operation(operation_type::CONTINUE_ROLLBACK, &operation_id, now);
        self.client_request_token = client_request_token.clone();
        self.status = status::UPDATE_ROLLBACK_IN_PROGRESS.to_string();
        // A CLIENT asked for this rollback to be resumed, so the event that opens it is
        // `USER_INITIATED` — the reason every stack operation a client starts opens with
        // ([`Self::begin_transition`]), as against the rollback this one resumes, whose own
        // opening event carries the failure that forced it.
        self.status_reason = Some(USER_INITIATED.to_string());
        self.record_event(
            now,
            status::UPDATE_ROLLBACK_IN_PROGRESS,
            Some(USER_INITIATED.to_string()),
            Some(operation_id.clone()),
            client_request_token.clone(),
        );
        for logical_id in skipped {
            let Some(deployed) = self
                .resources
                .iter_mut()
                .find(|deployed| &deployed.resource.logical_id == logical_id)
            else {
                continue;
            };
            deployed.status = resource_status::UPDATE_COMPLETE.to_string();
            deployed.status_timestamp = now;
            deployed.status_reason = Some(RESOURCE_SKIPPED_DURING_UPDATE_ROLLBACK.to_string());
            let logical_resource_id = deployed.resource.logical_id.clone();
            let physical_resource_id = deployed.resource.physical_resource_id.clone();
            let resource_type = deployed.resource.resource_type.clone();
            self.record_resource_event(
                now,
                &logical_resource_id,
                Some(physical_resource_id),
                &resource_type,
                resource_status::UPDATE_COMPLETE,
                Some(RESOURCE_SKIPPED_DURING_UPDATE_ROLLBACK.to_string()),
                None,
                Some(operation_id.clone()),
                client_request_token.clone(),
            );
        }
        self.pending = Some(PendingStatus {
            status: status::UPDATE_ROLLBACK_COMPLETE.to_string(),
            status_reason: None,
            started_at: now,
            due_at: now + STATUS_TRANSITION_DELAY,
            operation_id: Some(operation_id),
            client_request_token,
        });
    }

    /// The LAST KNOWN STABLE STATE this stack settled in, if it has one.
    ///
    /// Recorded by [`Self::settle`], the one writer of a terminal status, and judged by the
    /// one rule that says which statuses are stable ([`is_stable_status`]): a stack whose
    /// create never completed has no such state, while one whose later operation failed
    /// settled in the status its last completed operation left, which names the
    /// configuration a rollback puts back. Kept on the stack rather than read back off the
    /// event log, which is bounded and evicts oldest-first
    /// ([`MAX_EVENTS_PER_STACK`]) — a busy stack must not forget that it was ever stable.
    pub fn last_stable_status(&self) -> Option<&str> {
        self.last_stable_status.as_deref()
    }

    /// Open the ROLLBACK a client asked for on a stack whose last operation FAILED, and
    /// answer the work it owes the services that own the stack's resources.
    ///
    /// `RollbackStack` "rolls back the specified stack to the last known stable state", and
    /// "will delete a stack if it doesn't contain a last known stable state", so what the
    /// rollback is depends on whether this stack HAS one ([`Self::last_stable_status`]):
    ///
    /// * with none, there is nothing to go back to — the resources the failed operation had
    ///   provisioned are removed again and the stack heads for
    ///   [`status::ROLLBACK_COMPLETE`], exactly as the rollback of a failed create does;
    /// * with one, the configuration in force before the failed operation is put back and
    ///   the stack heads for [`status::UPDATE_ROLLBACK_COMPLETE`] — the plan being the one
    ///   the operation that failed held ([`Self::rollback_plan`], kept for this call where
    ///   that operation preserved its failed state), or, for a stack this service holds no
    ///   plan for, the resources in force declared against themselves, which asks the owning
    ///   services for no change.
    ///
    /// The calls themselves are the caller's to drive with the store unlocked
    /// ([`RequestedRollback`]), before the terminal status falls due; a revert an owning
    /// service refuses leaves the stack `UPDATE_ROLLBACK_FAILED` and a delete it refuses
    /// `ROLLBACK_FAILED`, the same as for a rollback no client asked for.
    pub fn begin_requested_rollback(
        &mut self,
        now: SystemTime,
        operation_id: String,
        client_request_token: String,
    ) -> RequestedRollback {
        // "A unique identifier for this rollback operation that can be used to track the
        // operation's progress and events": the rollback is the stack's own operation, of
        // the type the model names for it, so that is what the stack reports as its last and
        // what the events of this rollback carry.
        self.record_operation(operation_type::ROLLBACK, &operation_id, now);
        // What the operation that failed worked out it would have to put back, read BEFORE
        // the transition below retires it.
        let held = self
            .rollback_plan
            .take()
            .or_else(|| self.unreverted_rollback.clone());
        if self.last_stable_status().is_none() {
            let discarded = self.current_resources();
            self.begin_transition(
                now,
                status::ROLLBACK_IN_PROGRESS,
                status::ROLLBACK_COMPLETE,
                Some(operation_id),
                Some(client_request_token),
            );
            return RequestedRollback::Discard(discarded);
        }
        let in_force: Vec<StoredResource> = self
            .resources
            .iter()
            .map(|deployed| deployed.resource.clone())
            .collect();
        let plan = held.unwrap_or_else(|| OwedRollback {
            previous: in_force.clone(),
            applied: in_force,
        });
        // The failed operation's own deployment is ABANDONED, as it is for a rollback the
        // failure itself owed ([`Self::begin_update_rollback`]): the stack keeps the
        // resources, outputs and template stage its last completed operation left.
        self.pending_deployment = None;
        // ...and the configuration record of the operation that failed goes back with it, to
        // the revision in force before it — the same restore a rollback the failure itself
        // owed performs, read before the transition below opens this one.
        self.roll_configuration_back();
        self.begin_transition(
            now,
            status::UPDATE_ROLLBACK_IN_PROGRESS,
            status::UPDATE_ROLLBACK_COMPLETE,
            Some(operation_id),
            Some(client_request_token),
        );
        // This rollback is now the outstanding one: whatever an earlier one left unreverted
        // is what it is about to ask for again.
        self.unreverted_rollback = None;
        RequestedRollback::Revert(plan)
    }
}

/// The `Name` a resource's own declaration asks for, if it declares one — read through
/// [`template::name_property_candidates`], the one list of the properties a type takes its
/// name from, so this comparison and the deployment's own naming cannot disagree.
///
/// For a type whose identifier the OWNING SERVICE assigns, the declaration no longer
/// carries that identifier, so the name it asks for is what is left to say WHICH resource
/// a re-deploy means: AWS replaces such a resource when the template renames it, while a
/// declaration that names none leaves the name to CloudFormation and so still means the
/// same resource on the next deployment. Read off the evaluated properties, never off the
/// physical id, which for such a type is what the owning service minted.
pub(crate) fn declared_name(resource: &StoredResource) -> Option<String> {
    template::name_property_candidates(&resource.resource_type)
        .into_iter()
        .find_map(|property| {
            resource
                .properties
                .get(&property)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
}

/// Whether two revisions of one declaration ask for the SAME resource: they state every
/// property whose change would RECREATE it alike ([`template::replacing_properties`] — the
/// type's create-only properties, and the name it takes its identity from where that name is
/// one of them), and the create-only VALUE inside each property whose change recreates it
/// conditionally ([`template::conditionally_replacing_paths`]).
///
/// A resource is created with such a property and no call of its owning service changes it,
/// so a declaration that states one differently asks for a NEW resource rather than for a
/// change to the one the stack has. A property the owning service's own update call DOES take
/// is not one of them, so restating it is a change to the resource that exists and the
/// identity in force carries forward: a service that identifies a resource by an id IT minted
/// renames the resource in place, so such a rename moves neither the resource nor the
/// identifier the stack publishes for it.
///
/// The same reading of "Update requires: `Replacement`" a change set's `RequiresRecreation`
/// details are computed from, so a preview cannot say a change recreates a resource that the
/// deployment then updates in place. Both revisions' lists are consulted, since the name
/// property either of them states is read off its own declaration; a type that publishes none
/// has nothing to compare, so the test is vacuous for it.
fn declares_the_same_replacing_properties(
    existing: &StoredResource,
    resource: &StoredResource,
) -> bool {
    let same_properties = template::replacing_properties(
        &resource.resource_type,
        &resource.properties,
    )
    .into_iter()
    .chain(template::replacing_properties(
        &existing.resource_type,
        &existing.properties,
    ))
    .all(|property| existing.properties.get(&property) == resource.properties.get(&property));
    same_properties
        && template::conditionally_replacing_paths(&resource.resource_type)
            .iter()
            .all(|path| {
                template::property_at(&existing.properties, path)
                    == template::property_at(&resource.properties, path)
            })
}

/// Whether the identifier a resource of `resource_type` is known by is one that this
/// deployment cannot recompute, so a re-deploy carries the one in force FORWARD rather than
/// treating the resource as a different one.
///
/// Two cases, for the one reason: the OWNING SERVICE assigned the identifier
/// ([`template::identity_is_service_assigned`]), or — for a CUSTOM RESOURCE — the resource's
/// own PROVIDER did. "Ref returns the physical ID of the resource", and a custom resource's
/// is "a required custom resource provider-defined physical ID that is unique for that
/// provider" — the value the naming rules compute is only the stand-in a declaration is
/// resolved against until the provider answers. Without this a re-declaration of a custom
/// resource would be read as a different resource and created all over again, instead of
/// being updated through its provider.
fn identity_is_assigned_elsewhere(resource_type: &str) -> bool {
    template::identity_is_service_assigned(resource_type)
        || resource_types::is_custom_resource(resource_type)
}

/// Whether a terminal status is the one a ROLLBACK settles into, so the reason that rollback
/// carries has already been reported by the event that opened it.
///
/// CloudFormation explains a failure once in a stack's event stream: the event of the
/// transition that accounts for it — the failed status the stack RESTS in
/// ([`StoredStack::fail_transition`]), or the rollback that supersedes that status and
/// carries the failure with the clause saying the rollback was asked for
/// ([`StoredStack::begin_rollback`]). The rollback's terminal event reports that the rollback
/// finished, nothing about why it ran, so a client counting the events that carry a failure
/// counts one failure per failure.
fn concludes_a_rollback(terminal: &str) -> bool {
    matches!(
        terminal,
        status::ROLLBACK_COMPLETE
            | status::UPDATE_ROLLBACK_COMPLETE
            | status::IMPORT_ROLLBACK_COMPLETE
    )
}

/// The status an operation that cannot be carried out leaves the stack in, by the
/// `*_IN_PROGRESS` status it is in.
///
/// One rule, read off the operation in flight: a failure leaves the stack in the `*_FAILED`
/// status of the operation that failed — which is what a client polling that operation stops
/// on, and, for an update's rollback, the one status `ContinueUpdateRollback` resumes from
/// ("a stack goes into the UPDATE_ROLLBACK_FAILED state when CloudFormation can't roll back
/// all changes after a failed stack update"). An update that has applied every change and is
/// removing the resources it replaced is still that UPDATE, so its own failed status is the
/// update's. Anything else is a create, the status a stack that never completed one reports.
pub(crate) fn failed_status_of(in_progress: &str) -> &'static str {
    match in_progress {
        status::UPDATE_IN_PROGRESS | status::UPDATE_COMPLETE_CLEANUP_IN_PROGRESS => {
            status::UPDATE_FAILED
        }
        status::DELETE_IN_PROGRESS => status::DELETE_FAILED,
        status::ROLLBACK_IN_PROGRESS => status::ROLLBACK_FAILED,
        status::UPDATE_ROLLBACK_IN_PROGRESS => status::UPDATE_ROLLBACK_FAILED,
        _ => status::CREATE_FAILED,
    }
}

/// The `ResourceStatusReason` CloudFormation reports for a resource a resumed rollback
/// SKIPPED: the resource reaches `UPDATE_COMPLETE` without having been put back, and this is
/// the sentence that says why.
///
/// "CloudFormation sets the status of the specified resources to UPDATE_COMPLETE and
/// continues to roll back the stack" (`ContinueUpdateRollbackInput$ResourcesToSkip`) is the
/// STATUS; the reason beside it is recorded behaviour — CloudFormation writes this sentence,
/// verbatim and entire, on the skipped resource's `UPDATE_COMPLETE` event, which is what
/// distinguishes a resource the rollback really put back from one it was told to leave as the
/// failed update made it. Entirely FIXED text: no part of the stack, the resource or the
/// request reaches it, and the model documents no event wording at all.
pub const RESOURCE_SKIPPED_DURING_UPDATE_ROLLBACK: &str =
    "Resource skipped during UpdateRollback";

/// The `ResourceStatusReason` CloudFormation reports for a resource whose work has been
/// STARTED and has not finished: it records that the resource's creation — or, for an
/// update, its update — was initiated, which is what distinguishes a resource being worked
/// on from one merely queued behind another.
fn initiated_reason(in_progress_status: &str) -> &'static str {
    match in_progress_status {
        resource_status::UPDATE_IN_PROGRESS => "Resource update Initiated",
        _ => "Resource creation Initiated",
    }
}

/// The resource records a stack keeps once an operation has DISPOSED of its resources: a
/// deletion that completed, and the rollback of a create that removed again what it was
/// deploying.
///
/// Every resource the stack had installed, and every resource the disposed-of deployment
/// had ACTED ON ([`PendingDeployment::attempted`]), keeps a record at `DELETE_COMPLETE` as
/// of that instant. CloudFormation reports a resource of such a stack rather than
/// forgetting it — "For deleted stacks, ListStackResources returns resource information
/// for up to 90 days after the stack has been deleted" — and for a stack whose create was
/// rolled back those rows are the only per-resource view left: they are what names the
/// resource the rollback disposed of, under the logical id and the type the template
/// declared, and what a client waiting for a resource's cleanup to be confirmed observes.
/// A declaration the deployment never reached is not among them: nothing was created under
/// it and nothing disposed of it.
///
/// The record states no reason: the status says what became of the resource, and the
/// reason the create failed with belongs to the create's own event and to the stack's
/// `StackStatusReason`, not to the disposal that followed it.
///
/// The instant a record's status was updated is that RESOURCE's own: `finished_at` is when
/// the operation's walk finished each one ([`timeline`]), and `at` is the fallback for a
/// resource the walk recorded nothing for — one no owning service holds, which has no
/// deletion to go through.
///
/// A disposed record no longer claims the resource EXISTS anywhere: the operation took it
/// away in the service that owns it, so the record is reported with its identity and
/// without `materialised` — a later deletion of the stack asks nothing about it
/// ([`StoredStack::current_resources`]) and a later deployment cannot adopt it as a
/// resource that is already there.
fn disposed_resources(
    installed: &[DeployedResource],
    deployment: Option<PendingDeployment>,
    at: SystemTime,
    finished_at: &BTreeMap<String, SystemTime>,
) -> Vec<DeployedResource> {
    let mut records: Vec<DeployedResource> = installed.to_vec();
    let PendingDeployment {
        resources,
        attempted,
        ..
    } = deployment.unwrap_or_default();
    let acted_on = resources
        .into_iter()
        .filter(|resource| attempted.contains(&resource.logical_id));
    for resource in acted_on {
        match records
            .iter_mut()
            .find(|record| record.resource.logical_id == resource.logical_id)
        {
            // The stack already had a record under that logical id: the operation disposed
            // of the resource as the deployment last computed it, so that is the record.
            Some(record) => record.resource = resource,
            None => records.push(DeployedResource::new(
                resource,
                resource_status::DELETE_COMPLETE,
                at,
            )),
        }
    }
    for record in &mut records {
        record.status = resource_status::DELETE_COMPLETE.to_string();
        record.status_timestamp = finished_at
            .get(&record.resource.logical_id)
            .copied()
            .unwrap_or(at);
        record.status_reason = None;
        record.resource.materialised = false;
    }
    records
}

/// The resources a completed create or update leaves the stack with, each carrying the
/// resource-level status that operation left it in.
///
/// `StackResource$ResourceStatus` is the resource's "Current status" and
/// `StackResource$Timestamp` the "Time the status was updated", so a resource the
/// operation materialised for the first time reports the create's completion, one it
/// changed reports that completion as an update, and one it left exactly as it found it
/// keeps the status and the instant it already carried: nothing about that resource's
/// status was updated, so its timestamp must not move.
///
/// The instant a resource's status was updated is that RESOURCE's own: `finished_at` is
/// when the operation's walk finished each one ([`timeline`]), and `at` is the fallback for
/// a resource the walk recorded nothing for.
fn installed_resources(
    previous: &[DeployedResource],
    deployed: Vec<StoredResource>,
    at: SystemTime,
    finished_at: &BTreeMap<String, SystemTime>,
) -> Vec<DeployedResource> {
    deployed
        .into_iter()
        .map(|resource| {
            let updated_at = finished_at
                .get(&resource.logical_id)
                .copied()
                .unwrap_or(at);
            match previous
                .iter()
                .find(|before| before.resource.logical_id == resource.logical_id)
            {
                // Deployed before and identical: this operation did not touch it.
                Some(before) if before.resource == resource => before.clone(),
                // Deployed before under this logical id and now different: the operation
                // updated it.
                Some(_) => DeployedResource::new(
                    resource,
                    resource_status::UPDATE_COMPLETE,
                    updated_at,
                ),
                // Not deployed before: the operation created it.
                None => DeployedResource::new(
                    resource,
                    resource_status::CREATE_COMPLETE,
                    updated_at,
                ),
            }
        })
        .collect()
}

/// The instant a per-resource event the walk owes falls due, or `None` while the walk has
/// not reached it.
///
/// `flush` is the operation CONCLUDING: an event the walk had not reached by then is
/// recorded at that instant rather than dropped or stamped after it.
fn due_instant(computed: Option<SystemTime>, now: SystemTime, flush: bool) -> Option<SystemTime> {
    match computed {
        Some(at) if at <= now => Some(at),
        _ if flush => Some(now),
        _ => None,
    }
}
