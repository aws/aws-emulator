// SPDX-License-Identifier: CC0-1.0

//! The records a SERVICE is made of, and the projections each is reported through: the
//! service itself, the deployments and service revisions its rollouts mint, the
//! lifecycle hooks a rollout waits on, and the express-gateway configuration a managed
//! service carries.

use super::*;

/// An ECS service, as `CreateService` created it and `UpdateService` left it.
///
/// The record holds the service's CONFIGURATION only. Everything a response counts —
/// `runningCount`, `pendingCount`, the deployment's `rolloutState` — is derived from the
/// tasks its scheduler actually placed (see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_view`]), so a
/// reported count can never drift from the task records that exist.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    pub arn: String,
    pub name: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    pub status: String,
    /// `Service$desiredCount`: "The desired number of instantiations of the task
    /// definition to keep running on the service."
    pub desired_count: i32,
    /// The count the SCHEDULER has taken on for this service, which is what its deployment
    /// and its service revision report
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_view`]).
    ///
    /// `Service$desiredCount` is the caller's standing instruction and moves with the call
    /// that states it, while `Deployment$desiredCount` is "the most recent desired count of
    /// tasks that was specified for the service to deploy or maintain" — the count the
    /// scheduler is working to, which it takes on when it next runs
    /// ([`Self::scheduler_due_at`]). So the call that changes what a service wants never
    /// answers with the scheduler's own account of it, and a caller watching a scale-up sees
    /// the deployment take the step after the service did.
    pub scheduled_count: i32,
    /// When the scheduler's next pass over this service falls due, or `None` when it has
    /// nothing outstanding to take on. Set by every writer that changes
    /// [`Self::desired_count`] ([`Self::wants_task_count`]) and cleared by the pass that
    /// takes the count on, so the catch-up happens once and on the clock.
    pub scheduler_due_at: Option<SystemTime>,
    pub task_definition_arn: Option<String>,
    /// The Region, family and revision the ARN above names, kept as the key the store holds
    /// that revision under, so the scheduler can read the revision back when it places a
    /// task.
    pub task_definition: Option<(String, String, i32)>,
    pub launch_type: Option<String>,
    /// The launch type this service ran on before a capacity-provider strategy was put on
    /// it, kept while that strategy places its tasks and `None` for a service that never
    /// came off a launch type.
    ///
    /// A service reports either a strategy or a launch type, so moving it onto a strategy
    /// takes the launch type off [`Self::launch_type`] — but it is not gone, because the
    /// strategy can be taken off again: `UpdateService$capacityProviderStrategy` lists
    /// "Update the Auto Scaling group or Fargate capacity provider back to the launch type.
    /// Pass an empty list in the `capacityProviderStrategy` parameter" among the valid
    /// transitions, and the launch type that empty list takes the service back to is the one
    /// it held here ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::launch_type_off_strategy`]).
    pub launch_type_before_strategy: Option<String>,
    pub capacity_provider_strategy: Vec<CapacityProviderStrategyItem>,
    pub platform_version: Option<String>,
    /// The IAM role the CREATE named, resolved to an ARN, and `None` when it named none
    /// — in which case the service-linked role is reported for exactly the services that
    /// need one (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_role_arn`]).
    pub role_arn: Option<String>,
    pub load_balancers: Vec<LoadBalancer>,
    pub service_registries: Vec<ServiceRegistry>,
    pub deployment_configuration: DeploymentConfiguration,
    pub deployment_controller: DeploymentController,
    pub placement_constraints: Vec<PlacementConstraint>,
    pub placement_strategy: Vec<PlacementStrategy>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub service_connect_configuration: Option<ServiceConnectConfiguration>,
    pub volume_configurations: Vec<ServiceVolumeConfiguration>,
    pub vpc_lattice_configurations: Vec<VpcLatticeConfiguration>,
    pub health_check_grace_period_seconds: Option<i32>,
    pub scheduling_strategy: String,
    pub propagate_tags: String,
    pub enable_ecs_managed_tags: bool,
    pub enable_execute_command: bool,
    /// `Service$availabilityZoneRebalancing`, as the last request that stated it left it.
    ///
    /// The member's doc: "For create service requests, when no value is specified for
    /// AvailabilityZoneRebalancing, Amazon ECS defaults the value to ENABLED. For update
    /// service requests, when no value is specified ... Amazon ECS defaults to the existing
    /// service's AvailabilityZoneRebalancing value. If the service never had an
    /// AvailabilityZoneRebalancing value set, Amazon ECS treats this as DISABLED." Every
    /// service this API creates comes out of its create with one of the two enum values —
    /// the one the create named, else the create's own default
    /// ([`default_az_rebalancing`]) — so the value a record holds IS "the existing service's
    /// value" an update that names none defaults to, and a service that was never given one
    /// carries the value that closing sentence reads it as.
    pub availability_zone_rebalancing: String,
    pub tags: Vec<Tag>,
    /// `Service$createdBy`: "The principal that created the service" — the authenticated
    /// caller of the request that created it, resolved by [`caller_identity_arn`].
    pub created_by: String,
    /// `Service$events`: "The event stream for your service", OLDEST first. The
    /// scheduler's own narrative: an entry is appended only when a transition the clock
    /// owed actually happened (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]), never
    /// because somebody read the service.
    pub events: Vec<ServiceEventRecord>,
    /// The `clientToken` the create carried, so repeating that create is idempotent
    /// ("An identifier that you provide to ensure the idempotency of the request").
    pub client_token: Option<String>,
    pub created_at: DateTime,
    /// The id of the service's current (`PRIMARY`) deployment. Every task the scheduler
    /// places for it carries this id as its `startedBy`, which is how a superseded
    /// deployment's tasks are told apart from the current one's.
    pub deployment_id: String,
    pub deployment_created_at: DateTime,
    pub deployment_updated_at: DateTime,
    /// The ARN of the service revision the current deployment rolls out, minted with
    /// that deployment. `ServiceCurrentRevisionSummary$arn`: "The ARN of the current
    /// service revision."
    pub service_revision_arn: String,
    /// When the `DRAINING` this service was left in is owed its `INACTIVE`, or `None` for
    /// a service no delete has started draining (and once the drain has finished).
    /// `DeleteService`: "After all tasks have transitioned to either STOPPING or STOPPED
    /// status, the service status moves from DRAINING to INACTIVE" — a span the CLOCK
    /// settles ([`TRANSITION_SPAN`]), so the call that deletes the service never answers
    /// with the terminal status and a caller watching for `DRAINING` -> `INACTIVE` sees
    /// both.
    pub drain_due_at: Option<SystemTime>,
    /// When the current deployment's rollout is owed its steady state, or `None` once it
    /// has reached one. `Deployment$rolloutState`: "When a service deployment is started,
    /// it begins in an IN_PROGRESS state. When the service reaches a steady state, the
    /// deployment transitions to a COMPLETED state" — so a rollout is never COMPLETED in
    /// the same instant the call that started it answers; the CLOCK settles it.
    pub deployment_steady_due_at: Option<SystemTime>,
    /// Why the current deployment's rollout FAILED, for a rollout the deployment circuit
    /// breaker failed, and `None` for every other rollout.
    ///
    /// `Deployment$rolloutState`: "If the service fails to reach a steady state and circuit
    /// breaker is turned on, the deployment transitions to a `FAILED` state" — a terminal
    /// state, so the rollout owes no steady state once this is set (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_deployment_circuit_breakers`])
    /// and the reason stored here is the `rolloutStateReason` it is reported with.
    pub deployment_rollout_failure: Option<String>,
    /// When the rollback the circuit breaker started on that failed rollout falls due, or
    /// `None` for a rollout nothing is rolling back.
    ///
    /// `DeploymentCircuitBreaker$rollback`: "If rollback is on, when a service deployment
    /// fails, the service is rolled back to the last deployment that completed
    /// successfully" — the rollback FOLLOWS the failure, which is a span the CLOCK settles
    /// ([`TRANSITION_SPAN`]), so a caller reading in between sees the rollout `FAILED`
    /// before the service is taken back.
    pub deployment_rollback_due_at: Option<SystemTime>,
    /// When the tasks this rollout is REPLACING are owed their retirement, or `None` when
    /// none is owed (the rollout has nothing left to retire, or the retirement that was
    /// owed has been carried out).
    ///
    /// The step between a replacement coming up and the task it replaces being stopped. The
    /// `minimumHealthyPercent` doc: "the service scheduler will wait ... after a task
    /// reaches a `RUNNING` state before the task is counted towards the minimum healthy
    /// percent total" — so the scheduler does not retire a task in the same instant its
    /// replacement comes up, and a caller polling through a rolling update sees the service
    /// above its desired count while the two overlap ([`super::rolling_update`]). A span the
    /// CLOCK settles ([`ROLLOUT_OVERLAP_SPAN`]), stored rather than slept on.
    pub retirement_due_at: Option<SystemTime>,
    /// Whether the current deployment's rollout has reached its steady state at least
    /// once, and so whether that deployment has TAKEN OVER the service: the share of the
    /// service's desired count it is scaled to (`Deployment$scale`) follows from it.
    ///
    /// Kept beside [`Self::deployment_steady_due_at`] rather than read off it, because
    /// scaling a service that has already settled puts the scheduler back to work
    /// (re-opening that instant) without handing the service back to an earlier rollout.
    pub deployment_rollout_completed: bool,
    /// When the scheduler's confirmation that the current deployment is STABLE falls due,
    /// or `None` once it has been confirmed.
    ///
    /// `Deployment$stabilityStatus` is the deployment-level counterpart of
    /// `TaskSet$stabilityStatus` ("whether the task set has reached a steady state"), and
    /// the scheduler re-confirms it after every write that touches the deployment, so a
    /// caller that reads the service straight after writing it sees `STABILIZING` — a span
    /// the CLOCK settles ([`TRANSITION_SPAN`]), stored rather than slept on.
    pub deployment_stable_due_at: Option<SystemTime>,
    /// Whether this service has reached a steady state at least once, and so whether its
    /// run status is one the scheduler has anything to say about (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_run_status`]).
    /// Sticky: a later deployment does not take it back.
    pub reached_steady_state: bool,
    /// The WATCH the current rollout's CloudWatch alarms hold it under, for a service whose
    /// deployment configuration turned that method on; `None` for a rollout no alarm this
    /// emulator holds is watching.
    ///
    /// Written by the pass that reads those alarms out of the store that owns them
    /// ([`super::deployment_alarms`]), because how long they need to speak is their own
    /// configuration's and not this service's.
    pub deployment_alarm_watch: Option<AlarmWatchWindow>,
    /// `Service$version`: the version the service reports. It stays at the modelled default
    /// `0`: the recorded services report that default throughout their lives, after updates
    /// that roll a new deployment and mint a new service revision as much as after the ones
    /// that change nothing, so nothing here moves it.
    pub version: i64,
    /// Whether a task set was ever created in this service, and so whether the service HAS
    /// a deployment of its own to lose.
    ///
    /// Sticky: deleting the task sets is what takes the deployment away, which is the
    /// condition this answers (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_has_outstanding_work`]),
    /// so it is not taken back with them. A service nothing ever created a task set in has
    /// no deployment to lose and nothing outstanding for the want of one.
    pub held_task_set: bool,
    /// The deployments this one REPLACED and whose tasks are still being replaced,
    /// newest first. `Deployment$status`: "ACTIVE: A service deployment that still has
    /// running tasks, but are in the process of being replaced with a new launch type,
    /// platform version, task definition, or Docker image" — so an entry lives here for
    /// the span that replacement takes, and the clock pass that finds that span elapsed
    /// drops it ("INACTIVE: A deployment that has been completely replaced").
    pub superseded_deployments: Vec<SupersededDeployment>,
}

/// The window ONE rollout's named CloudWatch alarms WATCH it over.
///
/// `ServiceDeploymentRollbackMonitorsStatus` has a state for a watch that is still running
/// (`MONITORING`) beside the one for a watch that is over (`MONITORING_COMPLETE`), and
/// `ServiceDeploymentAlarms` says what ends a rollout the watch catches: "Amazon ECS considers
/// the service deployment as failed when any of the alarms move to the `ALARM` state". An alarm
/// reports what a revision is doing only once it has graded the series it watches, which takes
/// its own evaluation configuration's span — so a watch that ended the instant the rollout's
/// tasks were in place could never catch one, and a rolling deployment is not complete when its
/// tasks are: "After this percentage of tasks is healthy AND THE BAKE TIME ELAPSES, Amazon ECS
/// completes the deployment" (`DeploymentEarlySuccessCriteria$healthyPercent`). This is that
/// bake: the span the rollout's own alarms need to speak about the revision now running.
#[derive(Debug, Clone)]
pub struct AlarmWatchWindow {
    /// The `Deployment$id` of the rollout being watched, so a window left behind by an
    /// earlier rollout is never read as this one's.
    pub deployment_id: String,
    /// How long the alarms `DeploymentAlarms$alarmNames` names need to reach a verdict about
    /// the revision this rollout released, as the service that OWNS those alarms computes it
    /// from their `Period` and `EvaluationPeriods`
    /// (`cloudwatch::alarms_verdict_span_secs`).
    pub verdict_span: Duration,
    /// When this watch ENDS: one verdict span after the instant the rollout's tasks reached
    /// the steady state it was rolling out. `None` while they have not reached it — the watch
    /// runs for the whole of the rollout before that, so it has no end yet.
    pub ends_at: Option<SystemTime>,
}

/// A deployment a later one replaced, kept beside the service while the task set it
/// placed is still being replaced.
///
/// It holds the configuration that deployment rolled out, which is NOT the service's
/// current configuration any more — that is the point of reporting it: a client watching a
/// rolling update reads the old deployment's task definition, launch type and network
/// configuration beside the new one's.
#[derive(Debug, Clone)]
pub struct SupersededDeployment {
    /// The `Deployment$id` it was reported under, and the `startedBy` of every task it
    /// placed — which is how its remaining tasks are counted.
    pub id: String,
    /// The ARN of the service revision this deployment rolled out, so a client diffing the
    /// service's revisions across a rollout reads the one being replaced beside the new
    /// one (`Service$currentServiceRevisions`, "The list of the service revisions").
    pub service_revision_arn: String,
    pub task_definition_arn: Option<String>,
    /// The count the scheduler had taken on for this deployment when it was superseded
    /// ([`ServiceRecord::scheduled_count`]).
    pub desired_count: i32,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub launch_type: Option<String>,
    pub capacity_provider_strategy: Vec<CapacityProviderStrategyItem>,
    pub platform_version: Option<String>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub service_connect_configuration: Option<ServiceConnectConfiguration>,
    pub volume_configurations: Vec<ServiceVolumeConfiguration>,
    pub vpc_lattice_configurations: Vec<VpcLatticeConfiguration>,
    /// Whether this rollout had already reached its steady state when it was superseded,
    /// so the `rolloutState` it is reported with is the one it actually got to rather
    /// than one asserted now.
    pub rollout_completed: bool,
    /// Why this rollout FAILED, for one the deployment circuit breaker failed before it was
    /// superseded — the terminal `rolloutState` it keeps while the rollout that replaced it
    /// takes the service over, and `None` for a rollout that did not fail.
    pub rollout_failure: Option<String>,
    /// When the task set this deployment placed has been replaced, and so when the
    /// deployment stops being one of the service's: the clock-owed span of the stop the
    /// scheduler asked its tasks for ([`TRANSITION_SPAN`]), stored rather than slept on.
    pub replaced_due_at: SystemTime,
}

/// One entry of a service's event stream (`ServiceEvent`), as stored.
#[derive(Debug, Clone)]
pub struct ServiceEventRecord {
    pub id: String,
    pub created_at: DateTime,
    pub message: String,
}

impl ServiceEventRecord {
    /// The modelled shape a response carries for this event.
    pub fn view(&self) -> ServiceEvent {
        ServiceEvent {
            id: Some(self.id.clone()),
            created_at: Some(self.created_at),
            message: Some(self.message.clone()),
        }
    }
}

impl ServiceRecord {
    /// The deployment controller the service uses, defaulting to `ECS`.
    pub fn controller(&self) -> &str {
        self.deployment_controller.r#type.as_str()
    }

    /// Whether the ECS service scheduler places this service's tasks itself.
    ///
    /// Only the `ECS` deployment controller does: the `Service$taskSets` doc describes
    /// the other two as holding "a set of Amazon ECS tasks in either an CodeDeploy or an
    /// `EXTERNAL` deployment", whose tasks come from `CreateTaskSet` rather than from
    /// the service.
    pub fn places_tasks(&self) -> bool {
        self.controller() == DEPLOYMENT_CONTROLLER_ECS
    }

    /// Whether this service counts towards the cluster's `activeServicesCount`.
    pub fn is_active(&self) -> bool {
        self.status == SERVICE_ACTIVE
    }

    /// Who manages this service (`Service$resourceManagementType`). Every service this
    /// slice holds came from `CreateService`, so it is the customer's
    /// ([`RESOURCE_MANAGEMENT_CUSTOMER`]); an ECS-managed Express service comes from
    /// `CreateExpressGatewayService`, which is not part of this slice. Read by both the
    /// projection and the `ListServices` `resourceManagementType` filter, so the value
    /// reported and the value filtered on cannot drift.
    pub fn resource_management_type(&self) -> &'static str {
        RESOURCE_MANAGEMENT_CUSTOMER
    }

    /// Whether this service's tasks run on Fargate — the launch type the request named, or
    /// the Fargate capacity providers a strategy names, through the ONE predicate every
    /// reader of that question shares ([`places_on_fargate`]).
    ///
    /// This is what gates the members AWS documents as Fargate-only: `platformFamily`
    /// ("A platform family is specified only for tasks using the Fargate launch type")
    /// and `platformVersion` ("A platform version is only specified for tasks using the
    /// Fargate launch type").
    pub fn runs_on_fargate(&self) -> bool {
        places_on_fargate(
            self.launch_type.as_deref(),
            &self.capacity_provider_strategy,
        )
    }

    /// Append `events` to this service's event stream, keeping only the latest of them.
    ///
    /// `Service$events`: "The event stream for your service. A maximum of 100 of the latest
    /// events are displayed" — so only the latest are kept at all
    /// ([`MAX_SERVICE_EVENTS`]), which is also what keeps a long-lived service's record from
    /// growing without bound. The ONE place that trims the stream, so every writer of an
    /// event trims it the same way.
    pub fn record_events(&mut self, events: impl IntoIterator<Item = ServiceEventRecord>) {
        self.events.extend(events);
        if self.events.len() > MAX_SERVICE_EVENTS {
            let first_kept = self.events.len() - MAX_SERVICE_EVENTS;
            self.events.drain(..first_kept);
        }
    }

    /// State that this service now WANTS `count` tasks, as of `now`.
    ///
    /// `Service$desiredCount` is the caller's standing instruction, so it takes the new
    /// count here and now. What the SCHEDULER is working to is a step behind it
    /// ([`Self::scheduled_count`]): taking the new count on is the scheduler's own work, so
    /// it is owed from this instant and the next clock pass materialises it
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]) — which
    /// is never this call's own pass, since that ran before this write. So the call that
    /// changes what a service wants is answered with the count the scheduler was still
    /// working to, and the calls after it read the count it has taken on. A count the
    /// scheduler is already working to owes nothing, so re-stating it moves nothing.
    ///
    /// The ONE place that writes the desired count, so no writer can leave the scheduler's
    /// own count behind without the pass that catches it up.
    pub fn wants_task_count(&mut self, count: i32, now: SystemTime) {
        self.desired_count = count;
        self.scheduler_due_at = (self.scheduled_count != count).then_some(now);
    }

    /// Move the deployment this service currently reports as `PRIMARY` onto
    /// [`Self::superseded_deployments`], as the deployment a new one replaces.
    ///
    /// Called by the operation that starts a new deployment, BEFORE the record is moved
    /// onto the new deployment's id and configuration, so the entry holds what that
    /// rollout was actually rolling out.
    pub fn supersede_deployment(&mut self, now: SystemTime) {
        self.superseded_deployments.insert(
            0,
            SupersededDeployment {
                id: self.deployment_id.clone(),
                service_revision_arn: self.service_revision_arn.clone(),
                task_definition_arn: self.task_definition_arn.clone(),
                desired_count: self.scheduled_count,
                created_at: self.deployment_created_at,
                updated_at: self.deployment_updated_at,
                launch_type: self.launch_type.clone(),
                capacity_provider_strategy: self.capacity_provider_strategy.clone(),
                platform_version: self.platform_version.clone(),
                network_configuration: self.network_configuration.clone(),
                service_connect_configuration: self.service_connect_configuration.clone(),
                volume_configurations: self.volume_configurations.clone(),
                vpc_lattice_configurations: self.vpc_lattice_configurations.clone(),
                rollout_completed: self.deployment_rollout_completed,
                // The terminal state the circuit breaker left that rollout in travels with
                // it: the deployment the service reports as replaced is the one that
                // failed, and the NEW rollout starts from no failure at all.
                rollout_failure: self.deployment_rollout_failure.take(),
                replaced_due_at: now + TRANSITION_SPAN,
            },
        );
        self.superseded_deployments
            .truncate(MAX_SUPERSEDED_DEPLOYMENTS);
        // The rollback the circuit breaker owed the superseded rollout is not owed to the
        // new one: whatever starts a new rollout decides where the service goes from here.
        self.deployment_rollback_due_at = None;
    }
}

/// One SERVICE DEPLOYMENT: the rollout of ONE service revision, as the call that started
/// it wrote it and the clock (or a later deployment) left it.
///
/// `ListServiceDeployments`: "A service deployment happens when you release a software
/// update for the service. You route traffic from the running service revisions to the new
/// service revison" — so a record is written by the call that STARTS a rollout
/// (`CreateService`, and an `UpdateService` that forces a new deployment or moves the
/// service onto another task definition), and never by a read. Scaling a service places
/// or stops tasks without changing the revision it runs, so it starts no deployment.
///
/// The record is the service's own `PRIMARY` deployment seen as a resource of its own:
/// [`Self::deployment_id`] is the `Deployment$id` the service reports, so the status this
/// record carries and the `rolloutState` the service's `Deployment` carries are two views
/// of one rollout and settle on the same instant (see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::settle_services`]). The one
/// `Deployment` of a service no record carries the id of is the redeployment a ROLLBACK
/// performs, which belongs to the deployment being rolled back rather than to one of its
/// own ([`Self::rollback_deployment_id`]).
#[derive(Debug, Clone)]
pub struct ServiceDeploymentRecord {
    /// `arn:aws:ecs:<region>:<account>:service-deployment/<cluster>/<service>/<id>`.
    pub arn: String,
    /// The `Deployment$id` (`ecs-svc/...`) of the service deployment this records.
    pub deployment_id: String,
    pub service_arn: String,
    pub service_name: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    /// When the call that started the rollout created it. The rollout begins with that
    /// same call — the ECS scheduler starts replacing tasks as soon as it is given the
    /// new revision — so it is also the `startedAt` the brief reports.
    pub created_at: DateTime,
    /// `ServiceDeploymentBrief$finishedAt`: "The time that the service deployment
    /// completed", and so `None` for exactly the deployments still under way.
    pub finished_at: Option<DateTime>,
    /// `ServiceDeploymentBrief$targetServiceRevisionArn`: "The ARN of the service revision
    /// being deplyed" — the revision minted with this deployment.
    pub target_service_revision_arn: String,
    /// One of `ServiceDeploymentStatus`; see [`SERVICE_DEPLOYMENT_IN_PROGRESS`].
    pub status: String,
    /// `ServiceDeploymentBrief$statusReason`: "Information about why the service deployment
    /// is in the current status."
    pub status_reason: String,
    /// The deployment configuration this rollout is being carried out under, as the call
    /// that started it left the service.
    ///
    /// `ServiceDeployment$deploymentConfiguration` belongs to ONE rollout, and a later
    /// update can change the service's configuration without releasing a new revision, so
    /// the configuration the rollout runs under is stored with it rather than read off the
    /// service afterwards. The failure-detection mechanisms this record reports (the
    /// circuit breaker and the CloudWatch alarms) are the ones it turned on.
    pub deployment_configuration: DeploymentConfiguration,
    /// The region this rollout was started in, so the redeployment a ROLLBACK performs
    /// mints its ARNs in the same region as the rollout it undoes.
    pub region: String,
    /// The task-definition revision this rollout released, as the
    /// `(region, family, revision)` key the store holds it under.
    /// `ServiceDeployment$targetServiceRevision` is "the workload
    /// configuration being deployed", and the revision is the part of it a ROLLBACK has to
    /// put back when a later rollout is rolled back to this one.
    pub task_definition: Option<(String, String, i32)>,
    /// The PAUSE lifecycle hooks this rollout has REACHED, oldest first — what
    /// `ServiceDeployment$lifecycleHookDetails` reports ("the details of the lifecycle
    /// hooks for the current service deployment").
    pub lifecycle_hooks: Vec<LifecycleHookRecord>,
    /// The PAUSE stops still AHEAD of this rollout, in the order it reaches them, as the
    /// deployment configuration it runs under configured them
    /// ([`lifecycle_hook_stops`]).
    pub pending_hook_stops: Vec<LifecycleHookStop>,
    /// When the next of [`Self::pending_hook_stops`] is REACHED, or `None` when no stage is
    /// being approached. A rollout does not stand at a hooked stage the instant the call
    /// that started or resumed it answers: the CLOCK takes it there
    /// ([`TRANSITION_SPAN`]), stored rather than slept on.
    pub hook_due_at: Option<SystemTime>,
    /// When this rollout, resumed by a [`LIFECYCLE_HOOK_ACTION_CONTINUE`] with no stop left
    /// ahead of it, reaches its end, or `None` for a rollout no hook action resumed.
    pub complete_due_at: Option<SystemTime>,
    /// When the ROLLBACK this rollout was ended by has run its course, or `None` for a
    /// rollout that is not rolling back.
    pub rollback_due_at: Option<SystemTime>,
    /// `Deployment$id` of the REDEPLOYMENT that puts the previous revision back, for a
    /// rollout a rollback ended, and `None` for every other rollout.
    ///
    /// A rollback is carried out by running that revision again, which is a rollout of its
    /// own ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::roll_back_service_deployment_to`]),
    /// so WHICH rollout the rollback is waiting on is this id and not whatever the service
    /// happens to be rolling out later
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::finish_rolled_back_deployments`]).
    pub rollback_deployment_id: Option<String>,
    /// `ServiceDeployment$rollback`: "The rollback options the service deployment uses when
    /// the deployment fails" — carried by exactly the rollouts a rollback ended.
    pub rollback: Option<Rollback>,
    /// When the stop a caller asked for is CARRIED OUT, and `None` once it has been (or
    /// for a rollout nobody asked to stop).
    ///
    /// A stop does not take effect in the call that asks for it: the rollout reports the
    /// stop as REQUESTED ([`SERVICE_DEPLOYMENT_STOP_REQUESTED`]) and the CLOCK carries it
    /// out [`TRANSITION_SPAN`] later, stored rather than slept on.
    pub stop_due_at: Option<SystemTime>,
    /// The `StopServiceDeploymentStopType` a caller asked this rollout to stop with — it
    /// decides where the stop leaves the service — kept once asked, so a rollout that is
    /// stopping under a request a caller already made is told apart from one that ended
    /// without anybody asking.
    pub stop_type: Option<String>,
    /// Whether the DEPLOYMENT CIRCUIT BREAKER fired on this rollout.
    ///
    /// `ServiceDeploymentCircuitBreaker$status` is one of
    /// `ServiceDeploymentRollbackMonitorsStatus`, whose [`ROLLBACK_MONITORS_TRIGGERED`] member
    /// is what a breaker that FAILED the rollout it was watching reports — a different end
    /// from the watch that ran its course over a rollout nothing went wrong with, so the
    /// outcome is stored rather than guessed from the status.
    pub circuit_breaker_triggered: bool,
    /// The alarms of this rollout's own `DeploymentAlarms$alarmNames` that MOVED into their
    /// `ALARM` state while it was deploying.
    ///
    /// `ServiceDeploymentAlarms$triggeredAlarmNames`: "one or more CloudWatch alarm names
    /// that have been triggered during the service deployment" — and, through it, which
    /// monitor failed this rollout: a non-empty list is what an alarms status of
    /// [`ROLLBACK_MONITORS_TRIGGERED`] reports ([`super::deployment_alarms`]). Empty for
    /// every rollout no alarm fired on, the circuit breaker's failures included.
    pub triggered_alarms: Vec<String>,
}

/// One PAUSE lifecycle hook of a rollout, as the rollout reached it.
///
/// `DeploymentLifecycleHookDetail` is "the details of a deployment lifecycle hook that is
/// active during a service deployment", and its `hookId` is what
/// `ContinueServiceDeployment` acts on: "Use this value when calling
/// `ContinueServiceDeployment` to continue or roll back a paused deployment."
#[derive(Debug, Clone)]
pub struct LifecycleHookRecord {
    /// `DeploymentLifecycleHookDetail$hookId`, minted when the rollout reached this hook so
    /// every hook of every rollout is distinct.
    pub hook_id: String,
    /// The `ServiceDeploymentLifecycleStage` the rollout stands at while this hook waits.
    pub stage: String,
    /// One of `DeploymentLifecycleHookStatus`; see [`LIFECYCLE_HOOK_AWAITING_ACTION`].
    pub status: String,
    /// `DeploymentLifecycleHookDetail$expiresAt`: "The time when the lifecycle hook times
    /// out. If the hook has not been completed by this time, Amazon ECS takes the timeout
    /// action" — the instant the hook was reached plus the timeout its configuration set.
    pub expires_at: SystemTime,
    /// `DeploymentLifecycleHookDetail$timeoutAction`: "The action Amazon ECS takes when the
    /// lifecycle hook times out."
    pub timeout_action: String,
    /// The `DeploymentLifecycleHookAction` a caller took on this hook, once one has been
    /// taken — what makes a REPEAT of that same action a call that has already had its
    /// effect, and an action that differs from it one the rollout can no longer take
    /// ([`crate::state::ComAmazonawsEcsAmazonec2containerservicev20141113State::act_on_lifecycle_hook`]).
    /// `None` for a hook nobody has acted on, and for one the clock's timeout action
    /// resumed.
    pub taken_action: Option<String>,
}

/// A PAUSE stop a rollout has ahead of it: the stage it will pause at, with the timeout
/// the hook configured for that wait.
#[derive(Debug, Clone)]
pub struct LifecycleHookStop {
    pub stage: String,
    /// `DeploymentLifecycleHookTimeoutConfiguration$timeoutInMinutes`, defaulted to
    /// [`DEFAULT_LIFECYCLE_HOOK_TIMEOUT_MINUTES`].
    pub timeout_minutes: i32,
    /// That configuration's `action`, defaulted to
    /// [`DEFAULT_LIFECYCLE_HOOK_TIMEOUT_ACTION`].
    pub timeout_action: String,
}

/// Why the lifecycle-hook action a request asks for cannot be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleHookRefusal {
    /// No stored rollout carries the service-deployment ARN the request named.
    NoSuchDeployment,
    /// That rollout holds no hook with the id the request named waiting for an action.
    NotAwaitingAction,
    /// A ROLLBACK was asked for on the FIRST rollout of a service, which has no earlier
    /// service revision to put back.
    NoPreviousRevision,
}

/// Why the stop a request asks for cannot be taken on the rollout it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceDeploymentStopRefusal {
    /// No stored rollout carries the service-deployment ARN the request named.
    NoSuchDeployment,
    /// That rollout is not ONGOING: it ended without anybody asking it to stop.
    NotOngoing,
}

/// End one rollout at `at` with `status` and `reason`, dropping every wait it still owed.
///
/// A rollout that has ended waits for nothing: the hook it stood at never got its action,
/// the stages ahead of it are never reached, and a stop or a rollback that had not run its
/// course does not run afterwards — this end is the one the rollout keeps.
pub(super) fn finish_deployment(
    deployment: &mut ServiceDeploymentRecord,
    at: DateTime,
    status: &str,
    reason: &str,
) {
    deployment.status = status.to_string();
    deployment.status_reason = reason.to_string();
    deployment.finished_at = Some(at);
    for hook in deployment
        .lifecycle_hooks
        .iter_mut()
        .filter(|hook| hook.status == LIFECYCLE_HOOK_AWAITING_ACTION)
    {
        hook.status = LIFECYCLE_HOOK_FAILED.to_string();
    }
    deployment.pending_hook_stops.clear();
    deployment.hook_due_at = None;
    deployment.complete_due_at = None;
    deployment.rollback_due_at = None;
    deployment.stop_due_at = None;
}

/// The PAUSE stops one rollout has, in the order it reaches them.
///
/// `DeploymentConfiguration$lifecycleHooks` is "an array of deployment lifecycle hook
/// objects to run custom logic or pause the deployment at specific stages of the deployment
/// lifecycle", and each hook names every stage it runs at
/// (`DeploymentLifecycleHook$lifecycleStages`), so one hook configured for two stages stops
/// the rollout twice. Only a [`LIFECYCLE_HOOK_TARGET_PAUSE`] hook stops it, and a stage
/// named by two hooks stops it once: the rollout stands at a stage, not at a configuration
/// entry.
pub(super) fn lifecycle_hook_stops(configuration: &DeploymentConfiguration) -> Vec<LifecycleHookStop> {
    let mut stops: Vec<LifecycleHookStop> = configuration
        .lifecycle_hooks
        .iter()
        .flatten()
        .filter(|hook| hook.target_type.as_deref() == Some(LIFECYCLE_HOOK_TARGET_PAUSE))
        .flat_map(|hook| {
            let timeout = hook.timeout_configuration.as_ref();
            let minutes = timeout
                .and_then(|timeout| timeout.timeout_in_minutes)
                .unwrap_or(DEFAULT_LIFECYCLE_HOOK_TIMEOUT_MINUTES);
            let action = timeout
                .and_then(|timeout| timeout.action.clone())
                .unwrap_or_else(|| DEFAULT_LIFECYCLE_HOOK_TIMEOUT_ACTION.to_string());
            hook.lifecycle_stages
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(move |stage| LifecycleHookStop {
                    stage,
                    timeout_minutes: minutes,
                    timeout_action: action.clone(),
                })
        })
        .collect();
    // The rollout reaches the stages in the documented order
    // ([`LIFECYCLE_HOOK_STAGE_ORDER`]), whatever order the configuration lists them in.
    stops.sort_by_key(|stop| lifecycle_stage_index(&stop.stage).unwrap_or(usize::MAX));
    stops.dedup_by(|left, right| left.stage == right.stage);
    stops
}

impl ServiceDeploymentRecord {
    /// Whether this rollout has ended — which is exactly when it carries a `finishedAt`,
    /// so a reported status and a reported finish time can never disagree.
    pub fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }

    /// Whether this rollout is STOPPING: a stop has been asked for and not carried out
    /// yet, or the rollback that is undoing it has not run its course. Either way the
    /// rollout is already on its way to an end, so it reaches no steady state and no
    /// further stop can be asked for on it.
    pub fn is_stopping(&self) -> bool {
        self.stop_due_at.is_some() || self.rollback_due_at.is_some()
    }

    /// Whether a caller has asked this rollout to STOP — whether or not that stop has been
    /// carried out yet. A rollout that ended without anybody asking (it reached the steady
    /// state it was rolling out, a later deployment replaced it, its service was deleted)
    /// carries no such request.
    pub fn stop_requested(&self) -> bool {
        self.stop_type.is_some()
    }

    /// The PAUSE hook this rollout is waiting on, if it stands at one.
    pub fn awaiting_hook(&self) -> Option<&LifecycleHookRecord> {
        self.lifecycle_hooks
            .iter()
            .find(|hook| hook.status == LIFECYCLE_HOOK_AWAITING_ACTION)
    }

    /// Whether this rollout still owes a wait at a PAUSE hook — one it stands at now, or
    /// one it has not reached yet.
    ///
    /// `ContinueServiceDeployment`: "When a service deployment reaches a lifecycle stage
    /// that has a PAUSE hook configured, the deployment pauses and waits for an explicit
    /// action" — so such a rollout does not reach its own end until that action is taken,
    /// however settled the task set it placed is.
    pub fn awaits_lifecycle_action(&self) -> bool {
        self.awaiting_hook().is_some()
            || !self.pending_hook_stops.is_empty()
            || self.hook_due_at.is_some()
    }

    /// The key a listing orders deployments by: the `createdAt` the brief reports, with
    /// the ARN breaking a tie between two deployments created in the same instant (its
    /// last segment is minted from a monotonic fixed-width counter, so the later
    /// deployment always sorts after the earlier one).
    pub fn order_key(&self) -> ((i64, u32), &str) {
        (timestamp_key(self.created_at), self.arn.as_str())
    }

    /// The `ServiceDeploymentBrief` `ListServiceDeployments` reports for this deployment,
    /// built from what is stored.
    pub fn brief(&self) -> ServiceDeploymentBrief {
        ServiceDeploymentBrief {
            service_deployment_arn: Some(self.arn.clone()),
            service_arn: Some(self.service_arn.clone()),
            cluster_arn: Some(self.cluster_arn.clone()),
            started_at: Some(self.created_at),
            created_at: Some(self.created_at),
            finished_at: self.finished_at,
            target_service_revision_arn: Some(self.target_service_revision_arn.clone()),
            status: Some(self.status.clone()),
            status_reason: Some(self.status_reason.clone()),
        }
    }
}

/// One SERVICE REVISION: the workload configuration ONE rollout released, as the call that
/// released it left the service.
///
/// `DescribeServiceRevisions`: "A service revision is a version of the service that includes
/// the values for the Amazon ECS resources (for example, task definition) and the
/// environment resources (for example, load balancers, subnets, and security groups)", and
/// `ServiceRevision`: "Whenever you create or deploy a service, Amazon ECS automatically
/// creates and captures the configuration that you're trying to deploy in the service
/// revision." So the record is written by the call that RELEASES a revision and never
/// changed afterwards: what it holds is what that call was deploying, not what the service
/// happens to run now.
#[derive(Debug, Clone)]
pub struct ServiceRevisionRecord {
    pub arn: String,
    pub service_arn: String,
    pub cluster_arn: String,
    /// `ServiceRevision$taskDefinition`: "The task definition the service revision uses."
    pub task_definition_arn: Option<String>,
    /// The capacity provider strategy the revision uses, for a service that named one
    /// instead of a launch type, and `None` for one that named a launch type.
    pub capacity_provider_strategy: Option<Vec<CapacityProviderStrategyItem>>,
    pub launch_type: Option<String>,
    /// The Fargate-only members, carried by exactly the revisions of a service that runs
    /// on Fargate: the platform version the revision's tasks run and the platform family
    /// they run on.
    pub platform_version: Option<String>,
    pub platform_family: Option<String>,
    pub load_balancers: Vec<LoadBalancer>,
    pub service_registries: Vec<ServiceRegistry>,
    pub network_configuration: Option<NetworkConfiguration>,
    /// `ServiceRevision$containerImages`: "The container images the service revision
    /// uses", read off the task-definition revision this one released.
    pub container_images: Vec<ContainerImage>,
    pub service_connect_configuration: Option<ServiceConnectConfiguration>,
    pub volume_configurations: Vec<ServiceVolumeConfiguration>,
    pub vpc_lattice_configurations: Vec<VpcLatticeConfiguration>,
    /// `ServiceRevision$createdAt`: "The time that the service revision was created."
    pub created_at: DateTime,
}

/// `ServiceRevision$resolvedConfiguration`: "The resolved configuration for the service
/// revision which contains the actual resources your service revision uses, such as which
/// target groups serve traffic."
///
/// The target groups are the ones the revision's own configuration names, so a revision
/// that names none has no resolved configuration at all. A listener rule belongs to the
/// load balancer rather than to ECS, and nothing here provisions one, so only a rule the
/// configuration itself stated is reported.
pub(super) fn resolved_configuration(record: &ServiceRevisionRecord) -> Option<ResolvedConfiguration> {
    if record.load_balancers.is_empty() && record.vpc_lattice_configurations.is_empty() {
        return None;
    }
    Some(ResolvedConfiguration {
        load_balancers: Some(
            record
                .load_balancers
                .iter()
                .map(|balancer| ServiceRevisionLoadBalancer {
                    target_group_arn: balancer.target_group_arn.clone(),
                    production_listener_rule: balancer
                        .advanced_configuration
                        .as_ref()
                        .and_then(|advanced| advanced.production_listener_rule.clone()),
                })
                .collect(),
        ),
        vpc_lattice_configurations: Some(
            record
                .vpc_lattice_configurations
                .iter()
                .map(|configuration| ServiceRevisionVpcLatticeConfiguration {
                    target_group_arn: Some(configuration.target_group_arn.clone()),
                    production_listener_rule: configuration
                        .advanced_configuration
                        .as_ref()
                        .and_then(|advanced| advanced.production_listener_rule.clone()),
                })
                .collect(),
        ),
    })
}

/// Which optional service fields a response carries, as `DescribeServices$include`
/// (the `ServiceField` enum: `TAGS`) selects them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServiceFields {
    pub tags: bool,
}

impl ServiceFields {
    /// Every optional field — what the operation that CREATED or MUTATED the service
    /// returns, since the caller just supplied them and `CreateService`/`UpdateService`
    /// have no `include` member to narrow them with.
    pub fn all() -> Self {
        Self { tags: true }
    }

    /// The fields the `include` list of a describe request asks for. Unknown entries
    /// cannot reach here: `include` targets the closed `ServiceField` enum.
    pub fn from_include(include: Option<&Vec<String>>) -> Self {
        let mut fields = Self::default();
        for field in include.into_iter().flatten() {
            if field == "TAGS" {
                fields.tags = true;
            }
        }
        fields
    }
}

/// What [`ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_service`]
/// answers when the placement it was asked for would push this emulator past
/// [`MAX_TASKS`]: nothing was changed, so the caller refuses the whole request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskCapacityExceeded;

/// Which optional task-set fields a response carries, as `DescribeTaskSets$include`
/// (the `TaskSetField` enum: `TAGS`) selects them.
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskSetFields {
    pub tags: bool,
}

impl TaskSetFields {
    /// Every optional field — what the operation that CREATED the task set returns,
    /// since the caller just supplied them and `CreateTaskSet` has no `include` member
    /// to narrow them with.
    pub fn all() -> Self {
        Self { tags: true }
    }

    /// The fields the `include` list of a describe request asks for. Unknown entries
    /// cannot reach here: `include` targets the closed `TaskSetField` enum.
    pub fn from_include(include: Option<&Vec<String>>) -> Self {
        let mut fields = Self::default();
        for field in include.into_iter().flatten() {
            if field == "TAGS" {
                fields.tags = true;
            }
        }
        fields
    }
}

/// A service's `include` selection carries over to the task sets reported INSIDE it:
/// `DescribeServices$include` and `DescribeTaskSets$include` name the same `TAGS` field,
/// so a request that asked for the service's tags asked for its task sets' too.
impl From<ServiceFields> for TaskSetFields {
    fn from(fields: ServiceFields) -> Self {
        Self { tags: fields.tags }
    }
}

/// A set of tasks of ONE service, as `CreateTaskSet` created it.
///
/// The record holds the task set's CONFIGURATION only. `computedDesiredCount` is derived
/// from the service's `desiredCount` and this scale, and `runningCount`/`pendingCount`
/// are counted off the tasks this task set actually placed (see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::task_set_view`]), so a
/// reported count can never drift from the records that exist.
#[derive(Debug, Clone)]
pub struct TaskSetRecord {
    /// `TaskSet$id`, an `ecs-svc/` identifier like a deployment's.
    pub id: String,
    pub arn: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    pub service_name: String,
    /// `TaskSet$serviceArn`, as a task set reports it ([`task_set_service_arn`]).
    pub service_arn: String,
    pub external_id: Option<String>,
    pub status: String,
    pub task_definition_arn: String,
    /// The Region, family and revision the ARN above names, kept so the scheduler can read
    /// the revision back when it places a task — as [`ServiceRecord::task_definition`] is.
    pub task_definition: (String, String, i32),
    pub launch_type: Option<String>,
    pub capacity_provider_strategy: Vec<CapacityProviderStrategyItem>,
    pub platform_version: Option<String>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub load_balancers: Vec<LoadBalancer>,
    pub service_registries: Vec<ServiceRegistry>,
    /// `TaskSet$scale`: "A floating-point percentage of your desired number of tasks to
    /// place and keep running in the task set."
    pub scale: Scale,
    pub tags: Vec<Tag>,
    /// The `clientToken` the create carried, so repeating that create is idempotent
    /// ("An identifier that you provide to ensure the idempotency of the request").
    pub client_token: Option<String>,
    /// When the scheduler's confirmation that this task set has reached a steady state
    /// falls due, or `None` once it has been confirmed.
    ///
    /// `TaskSet$stabilityStatus` "indicates whether the task set has reached a steady
    /// state", and the scheduler re-confirms it after every write that touches the task
    /// set, so a caller that reads a task set straight after creating, scaling or
    /// promoting it sees `STABILIZING` — a span the CLOCK settles ([`TRANSITION_SPAN`]),
    /// stored rather than slept on, exactly as a service's deployment stability is
    /// ([`ServiceRecord::deployment_stable_due_at`]).
    pub stable_due_at: Option<SystemTime>,
    /// When the drain a `DeleteTaskSet` started falls due, or `None` for a task set nobody
    /// has deleted.
    ///
    /// `DeleteTaskSet` leaves the task set `DRAINING` — "the tasks in the task set are
    /// being stopped, and their corresponding targets are being deregistered from their
    /// target group" — so the task set is still one the service holds while that drain is
    /// under way, and the CLOCK is what finishes it ([`TRANSITION_SPAN`]): the record is
    /// gone once this instant passes, whether or not anybody called to look.
    pub removal_due_at: Option<SystemTime>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

impl TaskSetRecord {
    /// Whether a `DeleteTaskSet` has already taken this task set: it is draining away and
    /// the clock will remove it ([`Self::removal_due_at`]), so the delete has had it and
    /// no write modifies it any more.
    pub fn is_deleted(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// The key this record is stored under: the cluster, the service and the task set's
    /// own id — the three things every task-set operation addresses one by.
    pub fn key(&self) -> (String, String, String) {
        (
            self.cluster_name.clone(),
            self.service_name.clone(),
            self.id.clone(),
        )
    }

    /// Whether `reference` names this task set. The `taskSets` doc: "The ID or full
    /// Amazon Resource Name (ARN) of task sets to describe."
    pub fn is_named_by(&self, reference: &str) -> bool {
        let reference = reference.trim();
        reference == self.id || reference == self.arn
    }
}

/// Where ONE placement takes its capacity from, and whose task it is.
///
/// A service's own scheduler and a task set place tasks the same way and differ only in
/// this: the launch type or capacity provider the placement runs under, the platform
/// version it asks for, and the `startedBy` the placed task carries.
#[derive(Debug, Clone)]
pub struct Placement {
    pub launch_type: Option<String>,
    pub capacity_provider_name: Option<String>,
    pub platform_version: Option<String>,
    /// The networking every task of this placement is launched with, which is what an
    /// `awsvpc` task's own interface is allocated out of: the task set's when a task set
    /// places the task, the service's otherwise ("If a `launchType` is specified" the task
    /// set carries its own configuration).
    pub network_configuration: Option<NetworkConfiguration>,
    /// `Task$startedBy` of every task this placement places: the deployment id for a
    /// service's own scheduler, the task-set id for a task set's.
    pub started_by: String,
}

/// `Attachment$type` of the elastic network interface an `awsvpc` task is allocated —
/// `RegisterTaskDefinition$networkMode`: "If the network mode is `awsvpc`, the task is
/// allocated an elastic network interface". The member's own doc names this type first
/// ("such as `ElasticNetworkInterface`, `Service Connect`, and
/// `AmazonElasticBlockStorage`").
pub const ATTACHMENT_TYPE_ELASTIC_NETWORK_INTERFACE: &str = "ElasticNetworkInterface";

/// `Attachment$status` of an attachment Amazon ECS has just created for a task. The
/// member's own doc lists the statuses: "Valid values are `PRECREATED`, `CREATED`,
/// `ATTACHING`, `ATTACHED`, `DETACHING`, `DETACHED`, `DELETED`, and `FAILED`" — and what
/// moves it on is the CLOCK, which attaches the resource
/// ([`INTERFACE_ATTACH_LIFECYCLE`]), or the agent's own report
/// (`SubmitAttachmentStateChanges`: "Sent to acknowledge that an attachment changed
/// states"), never a read.
pub const ATTACHMENT_STATUS_PRECREATED: &str = "PRECREATED";

/// The `KeyValuePair` key an elastic-network-interface attachment reports its subnet
/// under. `Attachment$details` "for elastic network interfaces ... includes the network
/// interface ID, the MAC address, the subnet ID, and the private IPv4 address"; the subnet
/// is the one detail that is known the instant the attachment is created, because the
/// launch supplied it (`AwsVpcConfiguration$subnets`) — the other three belong to the
/// interface itself ([`AttachmentRecord::report_allocated_interface`]).
pub const ATTACHMENT_DETAIL_SUBNET_ID: &str = "subnetId";

/// One attachment of a task: the resource Amazon ECS allocated for it, in the state the
/// agent last acknowledged.
///
/// `Attachment` is "an object representing a container instance or task attachment" — for
/// a task, the elastic network interface an `awsvpc` launch gets. The record is what
/// `SubmitAttachmentStateChanges` resolves a reported `attachmentArn` against, and the
/// status it carries is the one that report left.
#[derive(Debug, Clone)]
pub struct AttachmentRecord {
    pub id: String,
    /// Which attachment of this store's whole history this is, from the counter the id was
    /// minted out of (`next_attachment_id`), which is what makes every attachment of every
    /// task distinct.
    pub sequence: u64,
    pub attachment_type: String,
    pub status: String,
    pub details: Vec<KeyValuePair>,
    /// When the next step of this attachment's own progression falls due
    /// ([`INTERFACE_ATTACH_LIFECYCLE`]), and `None` once the clock owes it nothing more:
    /// the resource is in place, or an agent has reported a status that progression does
    /// not pass through.
    pub next_status_due_at: Option<SystemTime>,
    /// The interface AMAZON VPC created for this attachment, as that store holds it
    /// ([`super::awsvpc_interfaces`]) — the three details that belong to the interface
    /// itself, which is why they are reported from here rather than derived
    /// ([`AttachmentRecord::report_allocated_interface`]). `None` until the allocation pass
    /// has created it, and for an attachment whose subnet Amazon VPC holds no record of.
    pub allocated: Option<AllocatedInterface>,
    /// Whether the interface above has been DELETED in Amazon VPC, which the stop of the
    /// task that held it is what does: the record keeps the details it reported (they are
    /// what a client reading a stopped task resolves the interface it held by), and this is
    /// what keeps a later pass from asking for the same deletion twice.
    pub interface_released: bool,
}

impl AttachmentRecord {
    /// The `Attachment` a task-returning operation reports for this record.
    pub fn view(&self) -> Attachment {
        Attachment {
            id: Some(self.id.clone()),
            r#type: Some(self.attachment_type.clone()),
            status: Some(self.status.clone()),
            details: Some(self.details.clone()),
        }
    }
}

/// `ExpressGatewayServiceStatusCode` of an Express service that is serving traffic.
pub const EXPRESS_GATEWAY_ACTIVE: &str = "ACTIVE";

/// `ExpressGatewayServiceStatusCode` of an Express service a delete has started tearing
/// down — the `DeleteExpressGatewayService` doc: "The service enters a DRAINING state
/// where existing tasks complete current requests without starting new tasks. After all
/// tasks stop, the service and infrastructure are permanently removed."
pub const EXPRESS_GATEWAY_DRAINING: &str = "DRAINING";

/// The CPU units an Express service's task takes when the request names none —
/// `CreateExpressGatewayService$cpu`: "The default value for an Express service is 256
/// (.25 vCPU)."
pub const DEFAULT_EXPRESS_CPU: &str = "256";

/// The memory (MiB) an Express service's task takes when the request names none —
/// `CreateExpressGatewayService$memory`: "The default value for an express service is
/// 512 MiB."
pub const DEFAULT_EXPRESS_MEMORY: &str = "512";

/// The CPU architecture an Express service's task runs on when the request names none —
/// `CreateExpressGatewayService$cpuArchitecture`: "If you don't specify a value, the
/// default is X86_64."
pub const DEFAULT_EXPRESS_CPU_ARCHITECTURE: &str = "X86_64";

/// The load balancer's health-check path when the request names none —
/// `CreateExpressGatewayService$healthCheckPath`: "If not specified, the default health
/// check path is /ping."
pub const DEFAULT_EXPRESS_HEALTH_CHECK_PATH: &str = "/ping";

/// The container port the load balancer sends traffic to when the request names none —
/// `CreateExpressGatewayService$primaryContainer`: "You can also configure the container
/// port (defaults to 80)".
pub const DEFAULT_EXPRESS_CONTAINER_PORT: i32 = 80;

/// The auto-scaling target value when the request names none —
/// `CreateExpressGatewayService$scalingTarget`: "If not specified, the default target
/// value for an Express service is 60."
pub const DEFAULT_EXPRESS_SCALING_TARGET_VALUE: i32 = 60;

/// The refusal for a `healthCheckPath` that is not a path —
/// `CreateExpressGatewayService$healthCheckPath`: "The health check path must start with
/// a forward slash and can include query parameters."
pub fn express_health_check_path_message(path: &str) -> String {
    format!("The health check path must start with a forward slash, but was {path}")
}

/// The refusal for an Express service that is not there, as the recorded
/// `DeleteExpressGatewayService` refusal words it — the Express operations' fixed text
/// for the class, with no trailing period and no part of it from the request.
pub const EXPRESS_GATEWAY_SERVICE_NOT_FOUND_MESSAGE: &str = "Service not found";

/// The refusal for an Express service ARN that resolves to no service, as the recorded
/// `UpdateExpressGatewayService` refusal words it: the fixed `Resource not found: `
/// followed by the identifier the request named.
pub fn express_resource_not_found_message(reference: &str) -> String {
    format!("Resource not found: {reference}")
}

/// The members `taskDefinitionArn` cannot be combined with, in the order the
/// `taskDefinitionArn` doc lists them: "If you provide a task definition ARN, you cannot
/// also specify primaryContainer, executionRoleArn, taskRoleArn, cpu, memory, or
/// cpuArchitecture."
pub const EXPRESS_TASK_DEFINITION_EXCLUSIVE_MEMBERS: [&str; 6] = [
    "primaryContainer",
    "executionRoleArn",
    "taskRoleArn",
    "cpu",
    "memory",
    "cpuArchitecture",
];

/// The configuration members a `CreateExpressGatewayService` or
/// `UpdateExpressGatewayService` request carries, as the two operations share them.
///
/// Both calls RENDER one service revision's configuration out of these
/// (`UpdateExpressGatewayService`: "Amazon ECS creates a new service revision with
/// updated configuration"), so the defaults, the task-definition exclusivity and the
/// "keep what the service already runs" merge live here once rather than in each handler.
#[derive(Debug, Clone, Default)]
pub struct ExpressGatewayConfigurationRequest {
    pub execution_role_arn: Option<String>,
    pub task_role_arn: Option<String>,
    pub task_definition_arn: Option<String>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub cpu_architecture: Option<String>,
    pub network_configuration: Option<ExpressGatewayServiceNetworkConfiguration>,
    pub health_check_path: Option<String>,
    pub primary_container: Option<ExpressGatewayContainer>,
    pub scaling_target: Option<ExpressGatewayScalingTarget>,
}

impl ExpressGatewayConfigurationRequest {
    /// The members of [`EXPRESS_TASK_DEFINITION_EXCLUSIVE_MEMBERS`] this request names,
    /// in that order — the ones a caller-managed task definition already carries.
    pub(super) fn named_exclusive_members(&self) -> Vec<&'static str> {
        [
            self.primary_container.is_some(),
            self.execution_role_arn.is_some(),
            self.task_role_arn.is_some(),
            self.cpu.is_some(),
            self.memory.is_some(),
            self.cpu_architecture.is_some(),
        ]
        .into_iter()
        .zip(EXPRESS_TASK_DEFINITION_EXCLUSIVE_MEMBERS)
        .filter_map(|(named, member)| named.then_some(member))
        .collect()
    }

    /// The refusal for a request that names `taskDefinitionArn` beside a member the task
    /// definition itself carries, naming the ones it did: "If you provide a task
    /// definition ARN, you cannot also specify primaryContainer, executionRoleArn,
    /// taskRoleArn, cpu, memory, or cpuArchitecture."
    pub fn task_definition_conflict(&self) -> Option<String> {
        self.task_definition_arn.as_ref()?;
        let named = self.named_exclusive_members();
        if named.is_empty() {
            return None;
        }
        Some(format!(
            "taskDefinitionArn can not be specified with {}",
            named.join(", ")
        ))
    }

    /// The configuration of the revision this request produces, given the configuration
    /// the service runs NOW (`None` for a create).
    ///
    /// A member the request names is the one it named; a member it leaves out keeps what
    /// the service already ran ("If you don't specify a value, the service keeps its
    /// current architecture"), and falls back to the documented default where it ran
    /// none. A revision runs EITHER a task definition the caller manages or the container
    /// this service renders one for, never both, so the two member sets never appear in
    /// one configuration.
    pub fn resolve(
        &self,
        current: Option<&ExpressGatewayServiceConfiguration>,
        service_revision_arn: String,
        created_at: DateTime,
    ) -> ExpressGatewayServiceConfiguration {
        let kept = |pick: fn(&ExpressGatewayServiceConfiguration) -> Option<String>| {
            current.and_then(pick)
        };
        let network_configuration = self
            .network_configuration
            .clone()
            .or_else(|| current.and_then(|held| held.network_configuration.clone()));
        let health_check_path = self
            .health_check_path
            .clone()
            .or_else(|| kept(|held| held.health_check_path.clone()))
            .unwrap_or_else(|| DEFAULT_EXPRESS_HEALTH_CHECK_PATH.to_string());
        let scaling_target = express_scaling_target(
            self.scaling_target.as_ref(),
            current.and_then(|held| held.scaling_target.as_ref()),
        );

        // The request's own `taskDefinitionArn` puts the revision on a caller-managed
        // task definition; naming any container member moves it OFF one, and naming
        // neither keeps whichever the service already ran.
        let task_definition_arn = match (&self.task_definition_arn, self.named_exclusive_members()) {
            (Some(arn), _) => Some(arn.clone()),
            (None, named) if named.is_empty() => kept(|held| held.task_definition_arn.clone()),
            (None, _) => None,
        };
        if let Some(task_definition_arn) = task_definition_arn {
            return ExpressGatewayServiceConfiguration {
                service_revision_arn: Some(service_revision_arn),
                task_definition_arn: Some(task_definition_arn),
                network_configuration,
                health_check_path: Some(health_check_path),
                scaling_target: Some(scaling_target),
                created_at: Some(created_at),
                ..ExpressGatewayServiceConfiguration::default()
            };
        }

        ExpressGatewayServiceConfiguration {
            service_revision_arn: Some(service_revision_arn),
            execution_role_arn: self
                .execution_role_arn
                .clone()
                .or_else(|| kept(|held| held.execution_role_arn.clone())),
            task_role_arn: self
                .task_role_arn
                .clone()
                .or_else(|| kept(|held| held.task_role_arn.clone())),
            task_definition_arn: None,
            cpu: Some(
                self.cpu
                    .clone()
                    .or_else(|| kept(|held| held.cpu.clone()))
                    .unwrap_or_else(|| DEFAULT_EXPRESS_CPU.to_string()),
            ),
            memory: Some(
                self.memory
                    .clone()
                    .or_else(|| kept(|held| held.memory.clone()))
                    .unwrap_or_else(|| DEFAULT_EXPRESS_MEMORY.to_string()),
            ),
            cpu_architecture: Some(
                self.cpu_architecture
                    .clone()
                    .or_else(|| kept(|held| held.cpu_architecture.clone()))
                    .unwrap_or_else(|| DEFAULT_EXPRESS_CPU_ARCHITECTURE.to_string()),
            ),
            network_configuration,
            health_check_path: Some(health_check_path),
            primary_container: express_primary_container(
                self.primary_container.as_ref(),
                current.and_then(|held| held.primary_container.as_ref()),
            ),
            scaling_target: Some(scaling_target),
            // `ingressPaths` is "the entry point into this service revision": nothing here
            // provisions the Application Load Balancer that would answer at one, so the
            // revision reports no entry point rather than an endpoint no client can reach.
            ingress_paths: None,
            created_at: Some(created_at),
        }
    }
}

/// The primary container a revision runs: the one the request named, else the one the
/// service already ran, with the container port defaulted ("defaults to 80").
pub fn express_primary_container(
    requested: Option<&ExpressGatewayContainer>,
    current: Option<&ExpressGatewayContainer>,
) -> Option<ExpressGatewayContainer> {
    let mut container = requested.or(current)?.clone();
    container.container_port =
        Some(container.container_port.unwrap_or(DEFAULT_EXPRESS_CONTAINER_PORT));
    Some(container)
}

/// The auto-scaling configuration a revision runs: the one the request named, else the
/// one the service already ran, with the target value defaulted
/// ([`DEFAULT_EXPRESS_SCALING_TARGET_VALUE`]).
pub fn express_scaling_target(
    requested: Option<&ExpressGatewayScalingTarget>,
    current: Option<&ExpressGatewayScalingTarget>,
) -> ExpressGatewayScalingTarget {
    let mut target = requested.or(current).cloned().unwrap_or_default();
    target.auto_scaling_target_value = Some(
        target
            .auto_scaling_target_value
            .unwrap_or(DEFAULT_EXPRESS_SCALING_TARGET_VALUE),
    );
    target
}

/// The CLUSTER and the SERVICE NAME an Express service's `serviceArn` names.
///
/// An ECS service ARN carries the cluster that hosts it in its resource segment
/// (`service/<cluster>/<name>`), so the ARN names the cluster as well as the service —
/// which is what lets the three Express operations that take only a `serviceArn` resolve
/// the cluster they act in. Read with [`cluster_and_service_from_reference`], the crate's
/// one reader for that pair, so a reference is accepted here exactly where it is accepted
/// by the operations that take a `service` member; a value that names no
/// cluster-and-service pair resolves to nothing.
pub fn express_gateway_reference(reference: &str) -> Option<(String, String)> {
    let (cluster_name, name) = cluster_and_service_from_reference(reference);
    let cluster_name = cluster_name.filter(|cluster| !cluster.is_empty())?;
    if name.is_empty() {
        return None;
    }
    Some((cluster_name, name))
}

/// Why a `serviceArn` resolves to no stored Express service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpressGatewayRefusal {
    /// The ARN names a cluster this account does not hold as one an operation may act in.
    /// The operation words it from that cluster's own stored record, exactly as the
    /// operations that take a `cluster` member do (see [`cluster_refusal_message`]).
    ClusterNotFound,
    /// The ARN names no Express service of this account: the cluster it names holds none
    /// of that name, or the value names no cluster-and-service pair at all.
    NotFound,
}

/// One EXPRESS service: the record `CreateExpressGatewayService` wrote and the calls and
/// the clock since have left.
///
/// `ECSExpressGatewayService`: "Express services use a service revision architecture where
/// each service can have multiple active configurations, enabling blue-green deployments
/// and gradual rollouts", so the record holds the configurations themselves, newest
/// first, and every response is projected off them.
#[derive(Debug, Clone)]
pub struct ExpressGatewayServiceRecord {
    pub arn: String,
    pub name: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    pub infrastructure_role_arn: String,
    pub status_code: String,
    /// The configurations that are ACTIVE for this service, newest first. A rolling
    /// deployment holds the one it is replacing beside the one it rolls out until the
    /// rollout is due.
    pub configurations: Vec<ExpressGatewayServiceConfiguration>,
    pub current_deployment: String,
    pub tags: Vec<Tag>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    /// When the rolling deployment an update started finishes, after which only the
    /// configuration it rolled out is still active. Stored, never slept on:
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]
    /// materialises it.
    pub rollout_due_at: Option<SystemTime>,
    /// When the drain a delete started finishes — "After all tasks stop, the service and
    /// infrastructure are permanently removed", which is when the record goes.
    pub removal_due_at: Option<SystemTime>,
}

impl ExpressGatewayServiceRecord {
    /// At most this many configurations are active at once: the one a rolling deployment
    /// rolls out and the one it is replacing.
    pub const MAX_ACTIVE_CONFIGURATIONS: usize = 2;

    /// The configuration this service is rolling out — the newest one.
    pub fn target_configuration(&self) -> Option<&ExpressGatewayServiceConfiguration> {
        self.configurations.first()
    }

    /// Whether this service is still in a status a call may change it from.
    pub fn is_active(&self) -> bool {
        self.status_code == EXPRESS_GATEWAY_ACTIVE
    }

    /// The `ExpressGatewayServiceStatus` every projection of this record reports.
    pub(super) fn status(&self) -> ExpressGatewayServiceStatus {
        ExpressGatewayServiceStatus {
            status_code: Some(self.status_code.clone()),
            // `statusReason` explains a status that needs explaining; nothing here has
            // left this service in one.
            status_reason: None,
        }
    }

    /// The `ECSExpressGatewayService` the create, describe and delete operations report.
    ///
    /// `include_tags` is what `DescribeExpressGatewayService$include` gates ("Valid values
    /// are TAGS to include resource tags associated with the Express service"); an
    /// operation with no `include` member reports the full description, tags and all.
    pub fn view(&self, include_tags: bool) -> ECSExpressGatewayService {
        ECSExpressGatewayService {
            cluster: Some(self.cluster_arn.clone()),
            service_name: Some(self.name.clone()),
            service_arn: Some(self.arn.clone()),
            infrastructure_role_arn: Some(self.infrastructure_role_arn.clone()),
            status: Some(self.status()),
            current_deployment: Some(self.current_deployment.clone()),
            active_configurations: Some(self.configurations.clone()),
            tags: include_tags.then(|| self.tags.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
        }
    }

    /// The `UpdatedExpressGatewayService` an update reports: the service, plus the
    /// configuration the deployment it started is rolling out.
    pub fn updated_view(&self) -> UpdatedExpressGatewayService {
        UpdatedExpressGatewayService {
            service_arn: Some(self.arn.clone()),
            cluster: Some(self.cluster_arn.clone()),
            service_name: Some(self.name.clone()),
            status: Some(self.status()),
            target_configuration: self.target_configuration().cloned(),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
        }
    }
}

/// What one pass over the task map counts for ONE owner of tasks — a service, one of its
/// deployments or task sets, or a container instance: the RUNNING/PENDING split its own
/// counters report.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServiceTally {
    pub running: i32,
    pub pending: i32,
}

/// What one pass over the task map counts for ONE deployment of a service, beside its
/// RUNNING/PENDING split: see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::deployment_counters`].
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DeploymentCounters {
    /// The deployment's tasks that count as healthy (`Deployment$healthyTaskCount`).
    pub(super) healthy: i32,
    /// The tasks it lost while it still wanted them (`Deployment$totalFailedTasks`).
    pub(super) total_failed: i32,
    /// Of those, the ones that never reached `RUNNING`
    /// (`Deployment$failedLaunchTaskCount`).
    pub(super) failed_launch: i32,
    /// The tasks of an earlier deployment it replaced (`Deployment$replacedTaskCount`).
    pub(super) replaced: i32,
}

/// `sequence` spelled as a UUID: the five hyphen-separated groups of 8, 4, 4, 4 and 12
/// hexadecimal digits Amazon ECS spells an identifier of this kind in, with the counter
/// carried in the first and last group so every id is distinct.
///
/// Defined once, so every member whose id AWS spells as a UUID takes the same shape.
pub(super) fn uuid_from_sequence(sequence: u64) -> String {
    format!("{sequence:08x}-0000-0000-0000-{sequence:012x}")
}

/// The widths of the five hyphen-separated groups of a UUID, in order — the shape
/// [`uuid_from_sequence`] spells and the one a value is read back against.
const UUID_GROUP_WIDTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// Whether `value` is spelled as a UUID: five hyphen-separated groups of hexadecimal digits
/// of the widths [`UUID_GROUP_WIDTHS`] names, in either case.
///
/// The check of a value a client sends back for an identifier this service spells as a UUID,
/// so a value it minted always reads as one. The HYPHENATED form is the only one accepted,
/// because it is the only one AWS mints such an id in: a general UUID parser would also take
/// the simple and braced spellings, which no identifier of this service is ever handed out
/// in and which a refusal would then have to accept.
pub(super) fn is_uuid(value: &str) -> bool {
    let mut groups = value.split('-');
    for width in UUID_GROUP_WIDTHS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return false;
        }
    }
    groups.next().is_none()
}

/// The `Scale` a deployment is reported with: the share of the service's `desiredCount`
/// that deployment is scaled to, as the percentage `Scale` measures
/// ([`SCALE_UNIT_PERCENT`]).
///
/// A deployment that has taken the service over holds the whole of it
/// ([`MAX_SCALE_PERCENT`]); one that has not — a rollout still under way, or one a newer
/// deployment has replaced — holds none of it ([`MIN_SCALE_PERCENT`]).
pub(super) fn deployment_scale(taken_over: bool) -> Scale {
    Scale {
        value: if taken_over {
            MAX_SCALE_PERCENT
        } else {
            MIN_SCALE_PERCENT
        },
        unit: Some(SCALE_UNIT_PERCENT.to_string()),
    }
}
