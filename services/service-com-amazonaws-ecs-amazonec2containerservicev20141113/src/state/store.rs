// SPDX-License-Identifier: CC0-1.0

//! The store's own operations: every read and write the handlers make against the maps
//! the state struct holds, plus the clock pass that materialises the transitions already
//! due.

use super::*;

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Materialise every lifecycle transition the clock has already made due, as of
    /// `now`. Safe to call from a reporting operation: it only applies changes that
    /// were owed at this instant whether or not anybody called.
    pub fn advance_clock(&mut self, now: SystemTime) {
        // The containers whose programs are still being performed, read once before the task
        // pass below: the end of a standalone task's work waits on its essential containers
        // having exited ([`TaskRecord::essential_programs_running`]).
        let performing = self.performing_containers();
        // The capacity-provider association a cluster mutation started: once the update
        // is due the cluster's attachments have "successfully updated", whether or not
        // anybody called to look.
        for cluster in self.clusters.values_mut() {
            if let Some(due) = cluster.attachments_update_due_at {
                if due <= now {
                    cluster.attachments_update_due_at = None;
                    cluster.attachments_status = Some(ATTACHMENTS_UPDATE_COMPLETE.to_string());
                }
            }
        }
        // The capacity a Managed Instances provider's create had Amazon ECS stand up: once
        // the create is due the provider is usable, whether or not anybody called to look,
        // so it reports `ACTIVE` with its create complete.
        for provider in self.capacity_providers.values_mut() {
            if let Some(due) = provider.active_due_at {
                if due <= now {
                    provider.active_due_at = None;
                    provider.status = CAPACITY_PROVIDER_ACTIVE.to_string();
                    provider.update_status =
                        Some(CAPACITY_PROVIDER_CREATE_COMPLETE.to_string());
                }
            }
        }
        for task in self.tasks.values_mut() {
            // The elastic network interface an `awsvpc` task was allocated is created and
            // then ATTACHED on its own clock, one documented step at a time
            // ([`INTERFACE_ATTACH_LIFECYCLE`]): the steps were owed at these instants
            // whether or not anybody called to look. A task whose interface has just been
            // attached is on the network from that instant
            // ([`TaskRecord::report_connected`]).
            //
            // Only a task that has been PLACED is being given its interface: a task whose
            // capacity a provider still has to acquire has not been placed anywhere
            // ([`TaskRecord::is_awaiting_placement`]), so nothing is attaching an interface
            // for it and it is on no network — the attach begins when the placement does
            // ([`Self::place_tasks_awaiting_capacity`]).
            let mut attached_at = None;
            if task.desired_status == STATUS_RUNNING && !task.is_awaiting_placement() {
                for attachment in &mut task.attachments {
                    if let Some(at) = attachment.advance_interface(now) {
                        attached_at =
                            Some(attached_at.map_or(at, |seen: SystemTime| seen.max(at)));
                    }
                }
            }
            if let Some(at) = attached_at {
                task.report_connected(at);
                task.version += 1;
            }
            // The placement a parked task was owed from the instant capacity registered
            // into its cluster: it has been placed, so it is `PENDING` and its start is
            // owed from there like any other placed task's. A task Amazon ECS cannot
            // launch before the interface it was allocated is in place waits where it is
            // until that interface IS in place, however long the agent that reports on it
            // takes ([`TaskRecord::interfaces_attached`]).
            if let Some(due) = task.pending_due_at {
                if due <= now && task.interfaces_attached() {
                    task.pending_due_at = None;
                    task.last_status = STATUS_PENDING.to_string();
                    for container in &mut task.containers {
                        container.advance_to(STATUS_PENDING);
                    }
                    // The start a placed task is owed falls due one span after the instant
                    // the placement was owed at, so a caller reading in between still
                    // sees the task `PENDING`.
                    task.running_due_at = Some(due + TRANSITION_SPAN);
                    task.version += 1;
                }
            }
            if let Some(due) = task.running_due_at {
                // The start of a task whose launch steps have NOT been taken yet is not
                // owed to anybody: Amazon ECS assumes the roles the launch resolved and
                // fetches the images its containers run before any container of it starts
                // ([`TaskRecord::launch_steps_taken`]), so a pass that has not taken those
                // steps withholds the start rather than reporting a task running that
                // nothing has decided it may. The instant it falls due is kept, so the pass
                // that HAS taken them materialises the transition at the instant it was
                // owed at rather than at its own.
                if due <= now && task.launch_steps_taken() {
                    task.running_due_at = None;
                    // The images a task's containers run are fetched where the task is
                    // placed, and this placement reaches no registry that holds one
                    // ([`evaluate_image_pulls`]): the containers never start, so Amazon
                    // ECS stops the task instead of running it, with the stop code for a
                    // task that failed before it started.
                    match task.launch_failure.clone() {
                        Some(reason) => task.fail_launch(&reason, due),
                        None => {
                            task.last_status = STATUS_RUNNING.to_string();
                            task.started_at = Some(DateTime::from(due));
                            // A task placed on its own runs the work it was placed for
                            // and then ends, so the instant that work is finished is owed
                            // from the instant it started ([`CONTAINER_WORK_SPAN`]); a
                            // task a service's scheduler placed is kept in place by that
                            // scheduler instead.
                            if task.service_name.is_none() {
                                task.work_done_due_at = Some(due + CONTAINER_WORK_SPAN);
                            }
                            // The containers of a task that has started are the containers
                            // whose dependencies are met: one that waits on another
                            // (`ContainerDefinition$dependsOn`) stays `PENDING` until they
                            // are, which the pass below materialises for every running task
                            // ([`Self::start_due_containers`]).
                            // A task does not run before the interface it was allocated is
                            // in place, so by this instant Amazon ECS has created and
                            // attached it: the attachment reports `ATTACHED` and the
                            // interface's own id, MAC address and private IPv4 address
                            // ([`AttachmentRecord::attach_allocated_interface`]).
                            for attachment in &mut task.attachments {
                                attachment.attach_allocated_interface();
                            }
                            // A task that shares the networking of the host that runs it
                            // is on the network once it has started there.
                            task.report_connected(due);
                            task.version += 1;
                        }
                    }
                }
            }
            // The work a STANDALONE task was placed to do, finished at the instant it was
            // owed at whether or not anybody called to look: its essential containers have
            // exited, so Amazon ECS stops the task ([`TaskRecord::finish_work`]). Only a
            // task whose work is still going takes that stop — one already on its way out
            // is being stopped for the reason that put it there
            // ([`TaskRecord::work_is_running`]) — and only one whose essential containers
            // really have exited: a container still performing its program has not, so the
            // task runs on for as long as that program does.
            if let Some(due) = task.work_done_due_at {
                if due <= now
                    && task.work_is_running()
                    && !task.essential_programs_running(&performing)
                {
                    task.finish_work(due);
                }
            }
            if let Some(due) = task.stopped_due_at {
                if due <= now {
                    task.stopped_due_at = None;
                    // A task that was still PENDING when it was stopped never ran, so
                    // it gets no startedAt — only the stop timestamps.
                    task.running_due_at = None;
                    task.last_status = STATUS_STOPPED.to_string();
                    task.stopped_at = Some(DateTime::from(due));
                    task.execution_stopped_at = Some(DateTime::from(due));
                    for container in &mut task.containers {
                        container.advance_to(STATUS_STOPPED);
                    }
                    task.version += 1;
                }
            }
            // The scale-in protection a task was given lapses on its own: "Task
            // protection, by default, expires after 2 hours at which point Amazon ECS
            // clears the protectionEnabled property making the task eligible for
            // termination by a subsequent scale-in event."
            if let Some(expires) = task.protection_expires_at {
                if expires <= now {
                    task.protection_expires_at = None;
                    task.version += 1;
                }
            }
        }
        // The containers a running task has NOT started yet, started now that what each of
        // them waits on holds: the dependencies a container declares are a transition the
        // clock owes it, not work a caller asks for ([`super::container_health`]). Run after
        // the task pass above, so a task that has just reached `RUNNING` has its independent
        // containers started by the same pass.
        self.start_due_containers(now);
        // The scheduler's confirmation that a task set has reached a steady state, owed at
        // the instant the write that touched it put on the record: once it is due the task
        // set is stable whether or not anybody called to look.
        for task_set in self.task_sets.values_mut() {
            if task_set.stable_due_at.is_some_and(|due| due <= now) {
                task_set.stable_due_at = None;
            }
        }
        // The drain a `DeleteTaskSet` started: once it is due the task set it was draining
        // away is gone, whether or not anybody called to look.
        self.task_sets
            .retain(|_, task_set| task_set.removal_due_at.is_none_or(|due| due > now));
        // The rolling deployment an `UpdateExpressGatewayService` started ("performs a
        // rolling deployment to replace existing tasks"): once it is due the revision it
        // rolled out has replaced the one before it, so only the newest configuration is
        // still active.
        for service in self.express_gateway_services.values_mut() {
            if let Some(due) = service.rollout_due_at {
                if due <= now {
                    service.rollout_due_at = None;
                    service.configurations.truncate(1);
                }
            }
        }
        // The drain a `DeleteExpressGatewayService` started: "After all tasks stop, the
        // service and infrastructure are permanently removed", so once the drain is due
        // the record is gone — whether or not anybody called to look.
        self.express_gateway_services
            .retain(|_, service| service.removal_due_at.is_none_or(|due| due > now));
        // The container-agent update `UpdateContainerAgent` accepted: once it is due the
        // agent has been updated, whether or not anybody called to look, so the instance
        // reports the version it was brought to and the terminal stage of the update.
        for instance in self.container_instances.values_mut() {
            if let Some(due) = instance.agent_update_due_at {
                if due <= now {
                    instance.agent_update_due_at = None;
                    instance.agent_update_status = Some(AGENT_UPDATE_UPDATED.to_string());
                    let mut version_info = instance.version_info.clone().unwrap_or_default();
                    version_info.agent_version =
                        Some(LATEST_CONTAINER_AGENT_VERSION.to_string());
                    instance.version_info = Some(version_info);
                    // "Every time a container instance experiences a change ... the
                    // version counter is incremented."
                    instance.version += 1;
                }
            }
        }
        // The delete a `DeleteDaemonTaskDefinition` call asked for: once it is due the
        // revision has been deleted, whether or not anybody called to look. Terminal, so
        // the due instant is cleared with it and the status never moves again.
        for record in self.daemon_task_definitions.values_mut() {
            if let Some(due) = record.deleted_due_at {
                if due <= now {
                    record.deleted_due_at = None;
                    record.status = DAEMON_TASK_DEFINITION_DELETED.to_string();
                }
            }
        }
        // The deployment circuit breaker's own pass: it counts the launches the current
        // rollout lost, replaces them while it tolerates them, and fails the rollout once it
        // does not — before the rollout's record is advanced, so the rollback that failure
        // starts is carried out by the same pass once it falls due.
        self.advance_deployment_circuit_breakers(now);
        self.advance_service_deployments(now);
        self.settle_services(now);
        // The scheduler's steady-state work: a service that is short of the tasks its
        // caller asked it to maintain has the shortfall placed, which was owed from the
        // instant it fell short ([`super::service_steady_state`]). Run AFTER the stops this
        // pass materialised, so a task that has just reached `STOPPED` is already out of
        // the count its replacement is weighed against.
        self.maintain_service_task_counts(now);
        // Every status a task moved into during THIS pass becomes an event here, under the
        // same lock that materialised it. A transition the clock owes is observable for one
        // span only, so a pass that materialises one and a later writer that supersedes it
        // leave the comparison in `record_task_state_changes` — which reads the task's
        // CURRENT reported pair — with no trace of the status in between: the task really
        // passed through it, and no event announced it. Recording where the transition is
        // applied is what keeps the published stream the task's own history (see that
        // method's own documentation). Queueing only; the publication stays the pump's.
        self.record_task_state_changes(now);
    }

    /// Materialise the SERVICE-level transitions the clock owes, as of `now`: the steady
    /// state a started rollout settles into — which is also where the SERVICE DEPLOYMENT
    /// that rollout is recorded as becomes [`SERVICE_DEPLOYMENT_SUCCESSFUL`] — and the
    /// `INACTIVE` a drained service reaches once the tasks it was still cleaning up have
    /// stopped.
    ///
    /// Both were owed at this instant whether or not anybody called, so a reporting
    /// operation may run them — and each happens at most once, because the instant it
    /// settles is cleared with it.
    pub(super) fn settle_services(&mut self, now: SystemTime) {
        let keys: Vec<(String, String)> = self.services.keys().cloned().collect();
        for key in keys {
            if !self.services.contains_key(&key) {
                continue;
            }
            // `Deployment$rolloutState`: the rollout completes "when the service reaches
            // a steady state" — which needs BOTH the instant it falls due and the task
            // set the service wants.
            let tally = self.service_tally(&key.0, &key.1);
            let service = &self.services[&key];
            let settled = service.deployment_steady_due_at.filter(|due| {
                *due <= now
                    && service.places_tasks()
                    && tally.running == service.desired_count
                    && tally.pending == 0
            });
            // `DeleteService`: "After all tasks have transitioned to either STOPPING or
            // STOPPED status, the service status moves from DRAINING to INACTIVE" — which
            // needs BOTH the instant the drain falls due and the tasks it was cleaning up
            // actually gone.
            let drained = service.status == SERVICE_DRAINING
                && service.drain_due_at.is_some_and(|due| due <= now)
                && !self.service_has_active_tasks(&key.0, &key.1);
            // The SAME rollout, as the service-deployment resource: it finishes with the
            // steady state the service reaches, not a moment before or after.
            let settled_deployment = settled.and_then(|_| {
                self.current_service_deployment(service)
                    // A rollout that still owes a wait at a PAUSE lifecycle hook has not
                    // reached its end, however settled the task set it placed is: it ends
                    // where the action taken on that hook takes it. Nor does one that is
                    // already stopping: the end it reaches is the stop's.
                    .filter(|deployment| {
                        !deployment.awaits_lifecycle_action() && !deployment.is_stopping()
                    })
                    .map(|deployment| deployment.arn.clone())
            });
            // A superseded deployment is reported while it is "in the process of being
            // replaced"; once that replacement is due it "has been completely replaced"
            // and is no longer one of the service's deployments. The instant it falls due
            // is stored with the deployment, so this pass applies what the clock already
            // owed whether or not anybody called.
            let replaced: Vec<String> = service
                .superseded_deployments
                .iter()
                .filter(|superseded| superseded.replaced_due_at <= now)
                .map(|superseded| superseded.id.clone())
                .collect();

            let events = match settled {
                // The two events the scheduler writes when a rollout reaches its steady
                // state, in the order it writes them. The wording follows the service
                // event messages Amazon ECS documents ("Amazon ECS service event
                // messages" in the Developer Guide).
                Some(due) => {
                    let name = service.name.clone();
                    let deployment_id = service.deployment_id.clone();
                    vec![
                        self.mint_service_event(
                            due,
                            format!(
                                "(service {name}) (deployment {deployment_id}) deployment \
                                 completed."
                            ),
                        ),
                        self.mint_service_event(
                            due,
                            format!("(service {name}) has reached a steady state."),
                        ),
                    ]
                }
                None => Vec::new(),
            };

            let Some(service) = self.services.get_mut(&key) else {
                continue;
            };
            if settled.is_some() {
                service.deployment_steady_due_at = None;
                // The rollout has taken the service over, and the service has reached a
                // steady state at least once — neither of which a later rollout undoes.
                service.deployment_rollout_completed = true;
                service.reached_steady_state = true;
                service.record_events(events);
            }
            // The scheduler's confirmation that the deployment is stable, owed at the
            // instant the write that touched it put on the record: once it is due the
            // deployment is stable whether or not anybody called.
            if service.deployment_stable_due_at.is_some_and(|due| due <= now) {
                service.deployment_stable_due_at = None;
            }
            if drained {
                service.status = SERVICE_INACTIVE.to_string();
                service.drain_due_at = None;
                // The deletion is over, and the tombstone it leaves keeps no event
                // stream: a deleted service reports none of the events of the service it
                // used to be.
                service.events.clear();
            }
            if !replaced.is_empty() {
                service
                    .superseded_deployments
                    .retain(|superseded| !replaced.contains(&superseded.id));
            }
            // The rollout's own record ends where the rollout did. Guarded on an OPEN
            // deployment, so a rollout the clock already finished is not re-finished when
            // a later scale-out puts the service back through a steady state of the same
            // deployment.
            if let (Some(due), Some(arn)) = (settled, settled_deployment) {
                if let Some(deployment) = self.service_deployments.get_mut(&arn) {
                    if !deployment.is_finished() {
                        deployment.status = SERVICE_DEPLOYMENT_SUCCESSFUL.to_string();
                        deployment.status_reason =
                            rollout_state_reason(&deployment.deployment_id, true);
                        deployment.finished_at = Some(DateTime::from(due));
                    }
                }
            }
            if drained {
                // The drain is over, so the task sets that were draining away with the
                // service are no longer its task sets: a task set belongs to the service
                // it was created in, and that service has finished being deleted.
                self.task_sets.retain(|(cluster, service, _), _| {
                    (cluster, service) != (&key.0, &key.1)
                });
            }
        }
    }

    /// Mint one service event at `at`, with the next event id.
    ///
    /// `ServiceEvent$id` is "The ID string of the event"; Amazon ECS spells it as a UUID
    /// ([`uuid_from_sequence`]), and the counter is what makes every event of every
    /// service distinct.
    pub(super) fn mint_service_event(&mut self, at: SystemTime, message: String) -> ServiceEventRecord {
        self.next_service_event_seq += 1;
        ServiceEventRecord {
            id: uuid_from_sequence(self.next_service_event_seq),
            created_at: DateTime::from(at),
            message,
        }
    }

    /// Whether the service still holds a task that has not reached `STOPPED`.
    pub fn service_has_active_tasks(&self, cluster_name: &str, service_name: &str) -> bool {
        self.tasks.values().any(|task| {
            task.cluster_name == cluster_name
                && task.service_name.as_deref() == Some(service_name)
                && task.is_active()
        })
    }

    /// Mint the next task id (32 lowercase hex digits, the shape of an ECS task id).
    pub fn next_task_id(&mut self) -> String {
        self.next_task_seq += 1;
        format!("{:032x}", self.next_task_seq)
    }

    /// Mint the next attachment id. `Attachment$id` is "the unique identifier for the
    /// attachment", which Amazon ECS spells as a UUID ([`uuid_from_sequence`]); the
    /// counter is what makes every attachment of every task distinct.
    pub fn next_attachment_id(&mut self) -> String {
        self.next_attachment_seq += 1;
        uuid_from_sequence(self.next_attachment_seq)
    }

    /// The attachments Amazon ECS allocates to a task it is placing, from the task
    /// definition's network mode and the launch's own network configuration.
    ///
    /// `RegisterTaskDefinition$networkMode`: "If the network mode is `awsvpc`, the task is
    /// allocated an elastic network interface" — that interface is the attachment, and a
    /// task in any other network mode shares its host's networking and is allocated none.
    /// The attachment starts [`ATTACHMENT_STATUS_PRECREATED`] — Amazon ECS has CREATED the
    /// interface, so it already carries the id, the MAC address and the private IPv4 address
    /// it was given beside the subnet the launch named — and the CLOCK takes it on from
    /// there, one documented step at a time ([`INTERFACE_ATTACH_LIFECYCLE`]), the first of
    /// them owed [`INTERFACE_STEP_SPAN`] from `now`.
    pub fn mint_task_attachments(
        &mut self,
        network_mode: &str,
        configuration: Option<&NetworkConfiguration>,
        now: SystemTime,
    ) -> Vec<AttachmentRecord> {
        if network_mode != NETWORK_MODE_AWSVPC {
            return Vec::new();
        }
        let subnet = configuration
            .and_then(|configuration| configuration.awsvpc_configuration.as_ref())
            .and_then(|awsvpc| awsvpc.subnets.first().cloned());
        let details = subnet
            .map(|subnet| {
                vec![KeyValuePair {
                    name: Some(ATTACHMENT_DETAIL_SUBNET_ID.to_string()),
                    value: Some(subnet),
                }]
            })
            .unwrap_or_default();
        let mut attachment = AttachmentRecord {
            id: self.next_attachment_id(),
            sequence: self.next_attachment_seq,
            attachment_type: ATTACHMENT_TYPE_ELASTIC_NETWORK_INTERFACE.to_string(),
            status: ATTACHMENT_STATUS_PRECREATED.to_string(),
            details,
            next_status_due_at: Some(now + INTERFACE_STEP_SPAN),
        };
        attachment.report_allocated_interface();
        vec![attachment]
    }

    /// The ARN of the task of `cluster_name` that holds the attachment `attachment_id`, or
    /// `None` when no task of that cluster holds one — the lookup a report of an attachment
    /// state change resolves its `attachmentArn` through.
    pub fn task_arn_holding_attachment(
        &self,
        cluster_name: &str,
        attachment_id: &str,
    ) -> Option<String> {
        self.tasks
            .values()
            .find(|task| {
                task.cluster_name == cluster_name
                    && task
                        .attachments
                        .iter()
                        .any(|attachment| attachment.id == attachment_id)
            })
            .map(|task| task.task_arn.clone())
    }

    /// Record the status the agent reported for ONE attachment of ONE task, as
    /// `SubmitAttachmentStateChanges` does ("Sent to acknowledge that an attachment changed
    /// states"): the agent is what attaches the resource, so its word on the status is
    /// stored as given, and the clock carries the attachment on from wherever the report
    /// left it ([`AttachmentRecord::advance_interface`]).
    ///
    /// A report that puts the task's interface in place is also what puts the task on the
    /// network, so the task is `CONNECTED` from the instant of the report
    /// ([`TaskRecord::report_connected`]).
    ///
    /// `Task$version`: "Every time a task experiences a change ... the version counter is
    /// incremented" — an attachment of it changing state is such a change, so a report that
    /// moves the status bumps the counter and one that restates it does not.
    pub fn record_attachment_status(
        &mut self,
        task_arn: &str,
        attachment_id: &str,
        status: &str,
        now: SystemTime,
    ) {
        let Some(task) = self.tasks.get_mut(task_arn) else {
            return;
        };
        let Some(attachment) = task
            .attachments
            .iter_mut()
            .find(|attachment| attachment.id == attachment_id)
        else {
            return;
        };
        if attachment.status == status {
            return;
        }
        attachment.status = status.to_string();
        task.report_connected(now);
        task.version += 1;
    }

    /// Mint the next container id (32 lowercase hex digits, the shape of an ECS
    /// container id).
    pub fn next_container_id(&mut self) -> String {
        self.next_container_seq += 1;
        format!("{:032x}", self.next_container_seq)
    }

    /// Mint the next container-instance id (32 lowercase hex digits, the shape of an ECS
    /// container-instance id).
    pub fn next_container_instance_id(&mut self) -> String {
        self.next_container_instance_seq += 1;
        format!("{:032x}", self.next_container_instance_seq)
    }

    /// Materialise the DEFAULT cluster, where nobody has created one.
    ///
    /// An account holds the default cluster without anybody creating it — the Amazon ECS
    /// clusters doc: "When you first use Amazon ECS, a default cluster is created for
    /// you" — which is what makes "If you do not specify a cluster, the default cluster
    /// is assumed" resolve to a cluster that is there. A `default` cluster the store
    /// already holds is left exactly as the calls so far left it, tombstone included: this
    /// materialises the one nobody created, it never re-creates a deleted one.
    ///
    /// It is materialised on use rather than seeded when the store is built because the
    /// ARN a cluster carries is REGIONAL and the store is built before any request's
    /// Region is known, so the cluster has to take the Region of the call that first
    /// resolves it. Called today only by `CreateExpressGatewayService`, whose recorded
    /// units resolve the default cluster with no `CreateCluster` before them; the other
    /// operations that assume the default cluster each have their own recorded refusals
    /// and their own open issues, so they adopt this call as those are fixed rather than
    /// having their answers changed from here.
    pub fn ensure_default_cluster(&mut self, region: &str) {
        if self.clusters.contains_key(DEFAULT_CLUSTER_NAME) {
            return;
        }
        self.clusters.insert(
            DEFAULT_CLUSTER_NAME.to_string(),
            ClusterRecord {
                name: DEFAULT_CLUSTER_NAME.to_string(),
                arn: cluster_arn(region, DEFAULT_CLUSTER_NAME),
                status: CLUSTER_ACTIVE.to_string(),
                // Nothing created it, so nothing has tagged, configured or associated
                // anything with it.
                tags: Vec::new(),
                settings: Vec::new(),
                configuration: None,
                capacity_providers: Vec::new(),
                default_capacity_provider_strategy: Vec::new(),
                service_connect_defaults: None,
                attachments_status: None,
                attachments_update_due_at: None,
            },
        );
    }

    /// Mint the name Amazon ECS generates for an Express service whose request named
    /// none — `CreateExpressGatewayService$serviceName`: "If you don't specify a service
    /// name, Amazon ECS generates a unique name for the service." The counter is what
    /// makes it unique.
    pub fn next_express_gateway_service_name(&mut self) -> String {
        self.next_express_gateway_seq += 1;
        format!("express-{:019}", self.next_express_gateway_seq)
    }

    /// The `(cluster, name)` key a `serviceArn` resolves to, or WHY it resolves to no
    /// stored Express service.
    ///
    /// The three Express operations that address a service by ARN alone resolve it the
    /// same way, so the cluster the ARN names is checked before the service it names: a
    /// cluster that is not there is reported as that, exactly as it is for the operations
    /// that take a `cluster` member.
    pub fn express_gateway_key(
        &self,
        reference: &str,
    ) -> Result<(String, String), ExpressGatewayRefusal> {
        let Some((cluster_name, name)) = express_gateway_reference(reference) else {
            // A value that names no cluster-and-service pair names no service, which is
            // the same answer as a cluster that holds no service of that name.
            return Err(ExpressGatewayRefusal::NotFound);
        };
        if !self.has_active_cluster(&cluster_name) {
            return Err(ExpressGatewayRefusal::ClusterNotFound);
        }
        let key = (cluster_name, name);
        if !self.express_gateway_services.contains_key(&key) {
            return Err(ExpressGatewayRefusal::NotFound);
        }
        Ok(key)
    }

    /// The stored Express service a `serviceArn` names, for a reporting operation.
    pub fn express_gateway_service(
        &self,
        reference: &str,
    ) -> Result<&ExpressGatewayServiceRecord, ExpressGatewayRefusal> {
        let key = self.express_gateway_key(reference)?;
        self.express_gateway_services
            .get(&key)
            .ok_or(ExpressGatewayRefusal::NotFound)
    }

    /// The stored Express service a `serviceArn` names, for an operation that changes it.
    pub fn express_gateway_service_mut(
        &mut self,
        reference: &str,
    ) -> Result<&mut ExpressGatewayServiceRecord, ExpressGatewayRefusal> {
        let key = self.express_gateway_key(reference)?;
        self.express_gateway_services
            .get_mut(&key)
            .ok_or(ExpressGatewayRefusal::NotFound)
    }

    /// Mint the id of a service's next deployment. `Deployment$id` is an `ecs-svc/`
    /// identifier; the counter is what makes every deployment of every service distinct.
    pub fn next_deployment_id(&mut self) -> String {
        self.next_deployment_seq += 1;
        format!("ecs-svc/{:019}", self.next_deployment_seq)
    }

    /// Mint the id of the next task set. `TaskSet$id` is "The ID of the task set", an
    /// `ecs-svc/` identifier like a deployment's; the counter is what makes every task
    /// set of every service distinct.
    pub fn next_task_set_id(&mut self) -> String {
        self.next_task_set_seq += 1;
        format!("ecs-svc/{:019}", self.next_task_set_seq)
    }

    /// Mint the ARN of the service revision a deployment rolls out. A service revision is
    /// "a version of your service that includes the task definition and configuration"
    /// and is created with the deployment that rolls it out, so the counter is what makes
    /// every revision of every service distinct.
    pub fn next_service_revision_arn(
        &mut self,
        region: &str,
        cluster_name: &str,
        service_name: &str,
    ) -> String {
        self.next_service_revision_seq += 1;
        service_revision_arn(
            region,
            cluster_name,
            service_name,
            &format!("{:019}", self.next_service_revision_seq),
        )
    }

    /// Mint the ARN of the DAEMON DEPLOYMENT that places a daemon's tasks — the
    /// `CreateDaemon$deploymentArn` doc: "the initial daemon deployment. This deployment
    /// places daemon tasks on each container instance of the specified capacity
    /// providers". The counter is what makes every deployment of every daemon distinct,
    /// and it is fixed-width, so sorting two of these ARNs orders them oldest first.
    pub fn next_daemon_deployment_arn(
        &mut self,
        region: &str,
        cluster_name: &str,
        daemon_name: &str,
    ) -> String {
        self.next_daemon_deployment_seq += 1;
        daemon_deployment_arn(
            region,
            cluster_name,
            daemon_name,
            &format!("{:019}", self.next_daemon_deployment_seq),
        )
    }

    /// Mint the ARN of the SERVICE DEPLOYMENT that records one rollout. The counter is
    /// what makes every deployment of every service distinct, and it is fixed-width, so
    /// sorting two of these ARNs orders the deployments they name oldest first.
    pub fn next_service_deployment_arn(
        &mut self,
        region: &str,
        cluster_name: &str,
        service_name: &str,
    ) -> String {
        self.next_service_deployment_seq += 1;
        service_deployment_arn(
            region,
            cluster_name,
            service_name,
            &format!(
                "{:0width$}",
                self.next_service_deployment_seq,
                width = SERVICE_DEPLOYMENT_ID_WIDTH
            ),
        )
    }

    /// Record the SERVICE DEPLOYMENT of the rollout the CALLER just started for `key`,
    /// and answer its ARN.
    ///
    /// Called by `CreateService` and by an `UpdateService` that starts a new deployment,
    /// AFTER the service record carries the deployment id and the service revision that
    /// rollout belongs to — those two are what the record is built from, so the
    /// deployment a listing reports and the one the service reports cannot drift.
    ///
    /// Only the ECS scheduler's own rollout is a service deployment: the other deployment
    /// controllers' tasks come from a task set ([`ServiceRecord::places_tasks`]), so there
    /// is no rollout of a service revision to record and this answers `None`.
    pub fn start_service_deployment(
        &mut self,
        key: &(String, String),
        region: &str,
        at: DateTime,
        now: SystemTime,
    ) -> Option<String> {
        // The revision this rollout releases is captured with it, whether or not the ECS
        // scheduler is the one rolling it out: "Whenever you create or deploy a service,
        // Amazon ECS automatically creates and captures the configuration that you're
        // trying to deploy in the service revision."
        self.record_service_revision(key, at);
        let service = self.services.get(key)?;
        if !service.places_tasks() {
            return None;
        }
        let deployment_id = service.deployment_id.clone();
        let service_arn = service.arn.clone();
        let service_name = service.name.clone();
        let cluster_name = service.cluster_name.clone();
        let cluster_arn = service.cluster_arn.clone();
        let target_service_revision_arn = service.service_revision_arn.clone();
        let deployment_configuration = service.deployment_configuration.clone();
        let task_definition = service.task_definition.clone();
        // The stages this rollout pauses at, as the configuration it runs under configured
        // them. The FIRST of them is not reached in this call: the rollout travels to it,
        // which is a span the clock settles.
        let pending_hook_stops = lifecycle_hook_stops(&deployment_configuration);
        let hook_due_at = (!pending_hook_stops.is_empty()).then(|| now + TRANSITION_SPAN);
        // A rollout this one replaces never reaches the steady state it was rolling out:
        // its tasks are the ones `scale_service` stops as superseded, so it ends here
        // rather than staying open for a steady state that can no longer arrive.
        self.finish_open_deployments(
            &service_arn,
            at,
            SERVICE_DEPLOYMENT_STOPPED,
            &format!("ECS deployment {deployment_id} replaced this deployment."),
        );
        let arn = self.next_service_deployment_arn(region, &cluster_name, &service_name);
        self.service_deployments.insert(
            arn.clone(),
            ServiceDeploymentRecord {
                arn: arn.clone(),
                status_reason: rollout_state_reason(&deployment_id, false),
                deployment_id,
                service_arn: service_arn.clone(),
                service_name,
                cluster_name,
                cluster_arn,
                created_at: at,
                finished_at: None,
                target_service_revision_arn,
                status: SERVICE_DEPLOYMENT_IN_PROGRESS.to_string(),
                deployment_configuration,
                region: region.to_string(),
                task_definition,
                lifecycle_hooks: Vec::new(),
                pending_hook_stops,
                hook_due_at,
                complete_due_at: None,
                rollback_due_at: None,
                rollback: None,
                stop_due_at: None,
                stop_type: None,
                // Nothing has fired the breaker on a rollout that is only now starting.
                circuit_breaker_triggered: false,
            },
        );
        self.prune_service_deployments(&service_arn);
        Some(arn)
    }

    /// Capture what `key`'s current service revision IS, under the ARN the service carries
    /// for it.
    ///
    /// Captured once: a revision records the configuration the call that released it was
    /// deploying, so a rollout that runs a revision which was already released (the
    /// redeployment a rollback performs) leaves the capture it already has alone.
    pub(super) fn record_service_revision(&mut self, key: &(String, String), at: DateTime) {
        let Some(service) = self.services.get(key) else {
            return;
        };
        let arn = service.service_revision_arn.clone();
        if self.service_revisions.contains_key(&arn) {
            return;
        }
        let record = self.captured_service_revision(service, at);
        self.service_revisions.insert(arn, record);
    }

    /// The revision record a release of `service`'s current configuration captures.
    ///
    /// Every member is read off the service as it stands in this instant — the same
    /// configuration its own `Deployment` reports for the rollout
    /// ([`Self::service_view`]), so the revision and the deployment that rolls it out
    /// cannot disagree.
    pub(super) fn captured_service_revision(
        &self,
        service: &ServiceRecord,
        at: DateTime,
    ) -> ServiceRevisionRecord {
        // "A service will return either a capacityProviderStrategy or launchType
        // parameter, but not both, depending where one was specified when it was created."
        let (launch_type, capacity_provider_strategy) = match &service.launch_type {
            Some(launch_type) => (Some(launch_type.clone()), None),
            None => (None, Some(service.capacity_provider_strategy.clone())),
        };
        // The Fargate-only members: "A platform family is specified only for tasks using
        // the Fargate launch type", and so is a platform version — the version the tasks
        // of this revision run, which is what the alias a request named resolves to.
        let fargate = service.runs_on_fargate();
        let os_family = self.service_os_family(service);
        let platform_family = fargate.then(|| os_family_display(&os_family));
        let platform_version = fargate.then(|| {
            let requested = service
                .platform_version
                .clone()
                .unwrap_or_else(|| PLATFORM_VERSION_LATEST.to_string());
            resolved_platform_version(&requested, &os_family)
        });
        // "The container images the service revision uses", read off the task-definition
        // revision this one releases.
        let container_images = service
            .task_definition
            .as_ref()
            .and_then(|key| self.task_definitions.get(key))
            .map(|definition| {
                definition
                    .container_definitions
                    .iter()
                    .map(|container| ContainerImage {
                        container_name: container.name.clone(),
                        // "The container image digest": a digest is what a pull of the
                        // image resolves, and nothing here pulls one.
                        image_digest: None,
                        image: container.image.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        ServiceRevisionRecord {
            arn: service.service_revision_arn.clone(),
            service_arn: service.arn.clone(),
            cluster_arn: service.cluster_arn.clone(),
            task_definition_arn: service.task_definition_arn.clone(),
            capacity_provider_strategy,
            launch_type,
            platform_version,
            platform_family,
            load_balancers: service.load_balancers.clone(),
            service_registries: service.service_registries.clone(),
            network_configuration: service.network_configuration.clone(),
            container_images,
            service_connect_configuration: service.service_connect_configuration.clone(),
            volume_configurations: service.volume_configurations.clone(),
            vpc_lattice_configurations: service.vpc_lattice_configurations.clone(),
            created_at: at,
        }
    }

    /// The `ServiceRevision` `DescribeServiceRevisions` reports for ONE captured revision:
    /// what the call that released it captured, and nothing a later call changed.
    pub fn service_revision_view(&self, record: &ServiceRevisionRecord) -> ServiceRevision {
        ServiceRevision {
            service_revision_arn: Some(record.arn.clone()),
            service_arn: Some(record.service_arn.clone()),
            cluster_arn: Some(record.cluster_arn.clone()),
            task_definition: record.task_definition_arn.clone(),
            capacity_provider_strategy: record.capacity_provider_strategy.clone(),
            launch_type: record.launch_type.clone(),
            platform_version: record.platform_version.clone(),
            platform_family: record.platform_family.clone(),
            load_balancers: Some(record.load_balancers.clone()),
            service_registries: Some(record.service_registries.clone()),
            network_configuration: record.network_configuration.clone(),
            container_images: Some(record.container_images.clone()),
            // "Indicates whether Runtime Monitoring is turned on": nothing here turns
            // GuardDuty Runtime Monitoring on, so no revision runs with it.
            guard_duty_enabled: false,
            service_connect_configuration: record.service_connect_configuration.clone(),
            volume_configurations: Some(record.volume_configurations.clone()),
            // The Fargate ephemeral-storage encryption a service configures, which
            // nothing in this slice lets a service configure (the same member the
            // service's own `Deployment` reports empty).
            fargate_ephemeral_storage: None,
            created_at: Some(record.created_at),
            vpc_lattice_configurations: Some(record.vpc_lattice_configurations.clone()),
            resolved_configuration: resolved_configuration(record),
            // "The resources created and managed by Amazon ECS when you create an Express
            // service": an Express service is a resource of its own here, and no revision
            // of an ECS service owns any of them.
            ecs_managed_resources: None,
            // "The effective runtime overrides that Amazon ECS applies to this service
            // revision. This value is present only when Amazon ECS detects a difference
            // between the task definition and the actual runtime" — nothing runs a
            // container here, so no such difference is detected.
            overrides: None,
            gateway_connectors: None,
            // The optional monitoring configuration a service sets, which nothing in this
            // slice lets a service set.
            monitoring: None,
        }
    }

    /// Drop every captured revision that no stored service and no stored rollout names.
    ///
    /// A revision is read back through the service or the rollout that names it, so the
    /// ones worth keeping are exactly those; the rest went with the history that named
    /// them ([`Self::prune_service_deployments`]).
    pub(super) fn prune_service_revisions(&mut self) {
        let mut named: HashSet<String> = self
            .services
            .values()
            .map(|service| service.service_revision_arn.clone())
            .collect();
        named.extend(
            self.service_deployments
                .values()
                .map(|deployment| deployment.target_service_revision_arn.clone()),
        );
        self.service_revisions
            .retain(|arn, _| named.contains(arn));
    }

    /// Mint the id of one PAUSE lifecycle hook of a rollout.
    ///
    /// `DeploymentLifecycleHookDetail$hookId` is the id `ContinueServiceDeployment` acts
    /// on, and it carries the hook's type beside an id of its own
    /// ([`LIFECYCLE_HOOK_ID_FORMAT`]). Every hook this service reaches is a
    /// [`LIFECYCLE_HOOK_TARGET_PAUSE`] hook; Amazon ECS spells an id of this kind as a UUID
    /// ([`uuid_from_sequence`]), and the counter is what makes every hook of every rollout
    /// distinct.
    pub(super) fn next_lifecycle_hook_id(&mut self) -> String {
        self.next_lifecycle_hook_seq += 1;
        lifecycle_hook_id(
            LIFECYCLE_HOOK_TARGET_PAUSE,
            &uuid_from_sequence(self.next_lifecycle_hook_seq),
        )
    }

    /// Materialise the SERVICE-DEPLOYMENT transitions the clock owes, as of `now`: the
    /// PAUSE lifecycle stage a rollout reaches, the timeout action a hook nobody acted on
    /// in time falls to, and the end of a rollout a hook action resumed or rolled back.
    ///
    /// Each was owed at this instant whether or not anybody called, so a reporting
    /// operation may run them, and each happens at most once because the instant it falls
    /// due is cleared with it.
    pub(super) fn advance_service_deployments(&mut self, now: SystemTime) {
        let arns: Vec<String> = self.service_deployments.keys().cloned().collect();
        for arn in arns {
            // The stop a caller asked for: once it is due the rollout has been stopped,
            // whether or not anybody called to look. Where it leaves the service is the
            // stop type's: a ROLLBACK runs the previous service revision again, and any
            // other stop ends the rollout where it stood.
            let stopping = self
                .service_deployments
                .get(&arn)
                .and_then(|record| {
                    record
                        .stop_due_at
                        .map(|due| (due, record.stop_type.clone(), record.deployment_id.clone()))
                })
                .filter(|(due, _, _)| *due <= now);
            if let Some((due, stop_type, deployment_id)) = stopping {
                if let Some(record) = self.service_deployments.get_mut(&arn) {
                    record.stop_due_at = None;
                }
                if stop_type.as_deref() == Some(STOP_TYPE_ROLLBACK) {
                    let refusal = self
                        .roll_back_service_deployment(&arn, due, &stop_rollback_reason())
                        .err();
                    // A rollback whose previous revision is no longer there to put back
                    // cannot be carried out, and nobody is left to refuse it to.
                    if refusal.is_some() {
                        if let Some(record) = self.service_deployments.get_mut(&arn) {
                            let reason = no_previous_revision_reason(&record.service_arn);
                            finish_deployment(
                                record,
                                DateTime::from(due),
                                SERVICE_DEPLOYMENT_ROLLBACK_FAILED,
                                &reason,
                            );
                        }
                    }
                } else if let Some(record) = self.service_deployments.get_mut(&arn) {
                    finish_deployment(
                        record,
                        DateTime::from(due),
                        SERVICE_DEPLOYMENT_STOPPED,
                        &stopped_by_request_reason(&deployment_id),
                    );
                }
            }
            // The hooked stage the rollout was travelling to: once that is due it stands at
            // the stage, and the hook there is waiting for its action.
            let reached = self
                .service_deployments
                .get(&arn)
                .and_then(|record| record.hook_due_at)
                .filter(|due| *due <= now);
            if let Some(due) = reached {
                if let Some(stop) = self
                    .service_deployments
                    .get_mut(&arn)
                    .and_then(|record| {
                        record.hook_due_at = None;
                        (!record.pending_hook_stops.is_empty())
                            .then(|| record.pending_hook_stops.remove(0))
                    })
                {
                    // Minted only once the stop it belongs to is the rollout's: an id is
                    // never burned on a hook nothing reached.
                    let hook_id = self.next_lifecycle_hook_id();
                    if let Some(record) = self.service_deployments.get_mut(&arn) {
                        record.lifecycle_hooks.push(LifecycleHookRecord {
                            hook_id,
                            stage: stop.stage,
                            status: LIFECYCLE_HOOK_AWAITING_ACTION.to_string(),
                            // "The number of minutes Amazon ECS waits for the lifecycle
                            // hook to complete before taking the timeout action", counted
                            // from the instant the rollout reached the stage.
                            expires_at: minutes_from(due, stop.timeout_minutes),
                            timeout_action: stop.timeout_action,
                        });
                    }
                }
            }
            // The hook whose wait ran out: "If the hook has not been completed by this
            // time, Amazon ECS takes the timeout action."
            let expired = self.service_deployments.get(&arn).and_then(|record| {
                record
                    .awaiting_hook()
                    .filter(|hook| hook.expires_at <= now)
                    .map(|hook| (hook.hook_id.clone(), hook.timeout_action.clone(), hook.expires_at))
            });
            if let Some((hook_id, action, at)) = expired {
                let refusal = self
                    .resume_service_deployment(&arn, &action, at, &hook_rollback_reason())
                    .err();
                if let Some(record) = self.service_deployments.get_mut(&arn) {
                    if let Some(hook) = record
                        .lifecycle_hooks
                        .iter_mut()
                        .find(|hook| hook.hook_id == hook_id)
                    {
                        hook.status = LIFECYCLE_HOOK_TIMED_OUT.to_string();
                    }
                    // A rollback the service has no earlier revision for cannot be carried
                    // out, and nobody is left to refuse it to: the rollout ends there.
                    if refusal.is_some() {
                        record.status = SERVICE_DEPLOYMENT_ROLLBACK_FAILED.to_string();
                        record.status_reason = no_previous_revision_reason(&record.service_arn);
                        record.finished_at = Some(DateTime::from(at));
                    }
                }
            }
            // The end of a rollout a CONTINUE resumed with no stop left ahead of it: it
            // reached the steady state it was rolling out.
            let completed = self
                .service_deployments
                .get(&arn)
                .and_then(|record| record.complete_due_at)
                .filter(|due| *due <= now);
            if let Some(due) = completed {
                if let Some(record) = self.service_deployments.get_mut(&arn) {
                    record.complete_due_at = None;
                    if !record.is_finished() {
                        record.status = SERVICE_DEPLOYMENT_SUCCESSFUL.to_string();
                        record.status_reason = rollout_state_reason(&record.deployment_id, true);
                        record.finished_at = Some(DateTime::from(due));
                    }
                }
            }
            // The end of a rollout a ROLLBACK undid: the previous service revision is the
            // one the service runs again.
            let rolled_back = self
                .service_deployments
                .get(&arn)
                .and_then(|record| record.rollback_due_at)
                .filter(|due| *due <= now);
            if let Some(due) = rolled_back {
                if let Some(record) = self.service_deployments.get_mut(&arn) {
                    record.rollback_due_at = None;
                    if !record.is_finished() {
                        record.status = SERVICE_DEPLOYMENT_ROLLBACK_SUCCESSFUL.to_string();
                        record.finished_at = Some(DateTime::from(due));
                    }
                }
            }
        }
    }

    /// Take `action` on the PAUSE lifecycle hook `hook_id` of the rollout `arn` records, as
    /// `ContinueServiceDeployment` asks: "Use this API to either continue the deployment to
    /// the next stage or roll back to the previous service revision."
    ///
    /// Only a hook the rollout has REACHED and that is still waiting can be acted on
    /// (`ContinueServiceDeployment$hookId`: "The ID of the PAUSED lifecycle hook to act
    /// on"), so a rollout that stands at no hook, a hook that is not the one waiting and a
    /// hook whose action was already taken are each refused and change nothing.
    pub fn act_on_lifecycle_hook(
        &mut self,
        arn: &str,
        hook_id: &str,
        action: &str,
        now: SystemTime,
    ) -> Result<(), LifecycleHookRefusal> {
        let record = self
            .service_deployments
            .get(arn)
            .ok_or(LifecycleHookRefusal::NoSuchDeployment)?;
        if record
            .awaiting_hook()
            .is_none_or(|hook| hook.hook_id != hook_id)
        {
            return Err(LifecycleHookRefusal::NotAwaitingAction);
        }
        // The action is taken BEFORE the hook is marked, so a rollback that cannot be
        // carried out leaves the hook waiting for one that can.
        self.resume_service_deployment(arn, action, now, &hook_rollback_reason())?;
        if let Some(record) = self.service_deployments.get_mut(arn) {
            if let Some(hook) = record
                .lifecycle_hooks
                .iter_mut()
                .find(|hook| hook.hook_id == hook_id)
            {
                hook.status = LIFECYCLE_HOOK_SUCCEEDED.to_string();
            }
        }
        Ok(())
    }

    /// Resume the rollout `arn` records with `action`, the hook action a caller took or a
    /// hook's own timeout fell to.
    ///
    /// [`LIFECYCLE_HOOK_ACTION_CONTINUE`] "proceeds the deployment to the next lifecycle
    /// stage" — the next stage it pauses at, or, when it has none left, its own end. Both
    /// are spans the CLOCK settles: the rollout travels on from where it stood.
    pub(super) fn resume_service_deployment(
        &mut self,
        arn: &str,
        action: &str,
        at: SystemTime,
        rollback_reason: &str,
    ) -> Result<(), LifecycleHookRefusal> {
        if action == LIFECYCLE_HOOK_ACTION_ROLLBACK {
            return self.roll_back_service_deployment(arn, at, rollback_reason);
        }
        let record = self
            .service_deployments
            .get_mut(arn)
            .ok_or(LifecycleHookRefusal::NoSuchDeployment)?;
        if record.pending_hook_stops.is_empty() {
            record.complete_due_at = Some(at + TRANSITION_SPAN);
        } else {
            record.hook_due_at = Some(at + TRANSITION_SPAN);
        }
        Ok(())
    }

    /// Roll the rollout `arn` records back: "ROLLBACK - Rolls back the deployment to the
    /// previous service revision."
    ///
    /// The previous revision is the one the rollout just before this one released
    /// (`ServiceDeployment$sourceServiceRevisions`: "the currently deployed workload
    /// configuration"), so a service's FIRST rollout has none and the rollback is refused.
    /// What putting it back means is [`Self::roll_back_service_deployment_to`]'s, which the
    /// deployment circuit breaker reaches with the rollout IT chose to put back.
    pub(super) fn roll_back_service_deployment(
        &mut self,
        arn: &str,
        at: SystemTime,
        rollback_reason: &str,
    ) -> Result<(), LifecycleHookRefusal> {
        let record = self
            .service_deployments
            .get(arn)
            .ok_or(LifecycleHookRefusal::NoSuchDeployment)?
            .clone();
        let source = self
            .previous_service_deployment(&record)
            .cloned()
            .ok_or(LifecycleHookRefusal::NoPreviousRevision)?;
        self.roll_back_service_deployment_to(&record, &source, at, rollback_reason)
    }

    /// Roll `record` back to the revision `source` released — the rollback itself, whichever
    /// rollout a caller or the deployment circuit breaker chose to put back.
    ///
    /// Putting that revision back means running it again: the scheduler replaces the task
    /// set this rollout placed with one from `source`'s revision, which is a rollout of
    /// its own and is recorded like any other ([`Self::start_service_deployment`]). The
    /// rollout being undone keeps its own identity and ends in
    /// [`SERVICE_DEPLOYMENT_ROLLBACK_SUCCESSFUL`], carrying the `rollback` that names the
    /// revision which was deployed in its place.
    pub(super) fn roll_back_service_deployment_to(
        &mut self,
        record: &ServiceDeploymentRecord,
        source: &ServiceDeploymentRecord,
        at: SystemTime,
        rollback_reason: &str,
    ) -> Result<(), LifecycleHookRefusal> {
        let arn = record.arn.as_str();
        let definition = source
            .task_definition
            .as_ref()
            .and_then(|key| self.task_definitions.get(key))
            .cloned()
            .ok_or(LifecycleHookRefusal::NoPreviousRevision)?;
        let key = (record.cluster_name.clone(), record.service_name.clone());
        let timestamp = DateTime::from(at);
        let deployment_id = self.next_deployment_id();
        // The service a rollout belongs to outlives it, but a rollout of a service that is
        // gone has nothing to put a revision back onto.
        let Some(service) = self.services.get_mut(&key) else {
            return Err(LifecycleHookRefusal::NoPreviousRevision);
        };
        // The rollout being undone is the one whose task set is now being replaced, exactly
        // as a forced redeployment replaces the one it supersedes.
        service.supersede_deployment(at);
        service.deployment_id = deployment_id;
        service.deployment_created_at = timestamp;
        service.deployment_updated_at = timestamp;
        service.deployment_rollout_completed = false;
        service.deployment_steady_due_at = Some(at + TRANSITION_SPAN);
        service.deployment_stable_due_at = Some(at + TRANSITION_SPAN);
        service.version += 1;
        service.task_definition = source.task_definition.clone();
        service.task_definition_arn = Some(definition.arn.clone());
        // A service revision IS a workload configuration, so running the previous one again
        // runs that same revision rather than minting a new one — which is the revision
        // `Rollback$serviceRevisionArn` names: "The ARN of the service revision deployed as
        // part of the rollback."
        service.service_revision_arn = source.target_service_revision_arn.clone();
        // The scheduler brings the task set back in line with the revision the service runs
        // again. A placement this emulator cannot hold leaves the task set alone; the
        // rollback itself is recorded either way, as the deployment it undoes is over.
        let _ = self.scale_service(&key, &definition, &record.region, at);
        let redeployed = self.start_service_deployment(&key, &record.region, timestamp, at);
        // That redeployment is how this rollback puts the previous revision back, not a
        // release somebody asked for, so it waits at no lifecycle hook: a rollback that
        // paused again would leave the service short of the revision the caller asked to go
        // back to.
        if let Some(record) = redeployed
            .as_deref()
            .and_then(|arn| self.service_deployments.get_mut(arn))
        {
            record.pending_hook_stops.clear();
            record.hook_due_at = None;
        }
        // Written AFTER that redeployment, which ends every open rollout of the service:
        // this one did not merely stop, it was rolled back, and its rollback is still under
        // way until the clock settles it.
        if let Some(record) = self.service_deployments.get_mut(arn) {
            record.status = SERVICE_DEPLOYMENT_ROLLBACK_IN_PROGRESS.to_string();
            record.status_reason = format!(
                "ECS deployment {} was rolled back to service revision {}.",
                record.deployment_id, source.target_service_revision_arn
            );
            record.finished_at = None;
            record.complete_due_at = None;
            record.hook_due_at = None;
            record.pending_hook_stops.clear();
            record.rollback_due_at = Some(at + TRANSITION_SPAN);
            record.rollback = Some(Rollback {
                // "The reason the rollback happened. For example, the circuit breaker
                // initiated the rollback operation" — whichever of them this rollback is,
                // as the caller of this rollback states it.
                reason: Some(rollback_reason.to_string()),
                started_at: Some(timestamp),
                service_revision_arn: Some(source.target_service_revision_arn.clone()),
            });
        }
        Ok(())
    }

    /// The rollout of the same service just BEFORE `record`: the one that released the
    /// service revision a rollback of `record` puts back
    /// (`ServiceDeployment$sourceServiceRevisions`: "the currently deployed workload
    /// configuration"), and so `None` for a service's FIRST rollout.
    pub(super) fn previous_service_deployment(
        &self,
        record: &ServiceDeploymentRecord,
    ) -> Option<&ServiceDeploymentRecord> {
        self.service_deployments
            .values()
            .filter(|other| {
                other.service_arn == record.service_arn && other.order_key() < record.order_key()
            })
            .max_by(|left, right| left.order_key().cmp(&right.order_key()))
    }

    /// Record the stop a caller asked for on the rollout `arn` records:
    /// `StopServiceDeployment` "stops an ongoing service deployment".
    ///
    /// ONGOING is the whole precondition: a rollout that has already ended without anybody
    /// asking it to is refused and changed in no way. A rollout a caller HAS asked to stop
    /// is already stopping under that request, so asking again is answered with the stop
    /// already in effect and changes nothing — the second request asks for what the first
    /// one already got.
    ///
    /// The stop itself is a span the CLOCK settles
    /// ([`Self::advance_service_deployments`]) — this call records that it was asked for,
    /// and until it is carried out the rollout reports the REQUESTED status of the stop
    /// type it was asked with.
    pub fn request_service_deployment_stop(
        &mut self,
        arn: &str,
        stop_type: &str,
        at: SystemTime,
    ) -> Result<(), ServiceDeploymentStopRefusal> {
        let record = self
            .service_deployments
            .get(arn)
            .ok_or(ServiceDeploymentStopRefusal::NoSuchDeployment)?;
        if record.stop_requested() {
            return Ok(());
        }
        if record.is_finished() || record.is_stopping() {
            return Err(ServiceDeploymentStopRefusal::NotOngoing);
        }
        let deployment_id = record.deployment_id.clone();
        let rollback = stop_type == STOP_TYPE_ROLLBACK;
        // Whether the rollback this stop asks for can be carried out is NOT part of that
        // precondition: "You can use this option even if you didn't configure the service
        // deployment for the rollback option", so the stop is taken on an ongoing rollout
        // whatever lies behind it, and a rollback with no earlier service revision to put
        // back ends the rollout in [`SERVICE_DEPLOYMENT_ROLLBACK_FAILED`] where the clock
        // carries it out.
        if let Some(record) = self.service_deployments.get_mut(arn) {
            record.status = if rollback {
                SERVICE_DEPLOYMENT_ROLLBACK_REQUESTED.to_string()
            } else {
                SERVICE_DEPLOYMENT_STOP_REQUESTED.to_string()
            };
            record.status_reason = stop_requested_reason(&deployment_id, rollback);
            record.stop_type = Some(stop_type.to_string());
            record.stop_due_at = Some(at + TRANSITION_SPAN);
            // A rollout that is stopping waits at no lifecycle hook any more: the stop is
            // the end it is heading for, so the hook it stood at never gets its action and
            // the stages ahead of it are never reached.
            for hook in record
                .lifecycle_hooks
                .iter_mut()
                .filter(|hook| hook.status == LIFECYCLE_HOOK_AWAITING_ACTION)
            {
                hook.status = LIFECYCLE_HOOK_FAILED.to_string();
            }
            record.pending_hook_stops.clear();
            record.hook_due_at = None;
            record.complete_due_at = None;
        }
        Ok(())
    }

    /// End every rollout of `service_arn` that has not finished yet, at `at`.
    ///
    /// The two things that end one early are a deployment that REPLACES it and the DELETE
    /// of the service it was rolling out; both are calls, so neither is a transition the
    /// clock owes.
    pub(super) fn finish_open_deployments(
        &mut self,
        service_arn: &str,
        at: DateTime,
        status: &str,
        reason: &str,
    ) {
        for deployment in self.service_deployments.values_mut() {
            if deployment.service_arn == service_arn && !deployment.is_finished() {
                finish_deployment(deployment, at, status, reason);
            }
        }
    }

    /// Drop the oldest of `service_arn`'s deployments down to
    /// [`MAX_SERVICE_DEPLOYMENTS_PER_SERVICE`], the way a service's event stream keeps
    /// only its latest entries.
    pub(super) fn prune_service_deployments(&mut self, service_arn: &str) {
        let mut arns: Vec<String> = self
            .service_deployments
            .values()
            .filter(|deployment| deployment.service_arn == service_arn)
            .map(|deployment| deployment.arn.clone())
            .collect();
        if arns.len() <= MAX_SERVICE_DEPLOYMENTS_PER_SERVICE {
            return;
        }
        // The ARN ends with a fixed-width monotonic id, so sorting orders them oldest
        // first and the ones to drop are the leading entries.
        arns.sort();
        let first_kept = arns.len() - MAX_SERVICE_DEPLOYMENTS_PER_SERVICE;
        for arn in &arns[..first_kept] {
            self.service_deployments.remove(arn);
        }
        // The revisions those rollouts released are read back through them, so the ones
        // nothing names any more go with them.
        self.prune_service_revisions();
    }

    /// The stored SERVICE DEPLOYMENT of `service`'s current (`PRIMARY`) deployment, if
    /// this service has one — `Service$currentServiceDeployment` names it, and the
    /// rollout it records is the one the service's `Deployment` reports.
    pub fn current_service_deployment(
        &self,
        service: &ServiceRecord,
    ) -> Option<&ServiceDeploymentRecord> {
        self.service_deployments.values().find(|deployment| {
            deployment.service_arn == service.arn
                && deployment.deployment_id == service.deployment_id
        })
    }

    /// The `ServiceDeployment` `DescribeServiceDeployments` reports for ONE stored
    /// rollout: what the call that started it wrote, the revisions it moves the service
    /// between with their task counts COUNTED off the task map, and the state of the
    /// failure-detection mechanisms its own deployment configuration turned on.
    ///
    /// The same rollout the brief reports ([`ServiceDeploymentRecord::brief`]) in less
    /// detail, read from the same record, so the two views cannot disagree.
    pub fn service_deployment_view(&self, record: &ServiceDeploymentRecord) -> ServiceDeployment {
        let service = self
            .services
            .get(&(record.cluster_name.clone(), record.service_name.clone()));
        let finished = record.is_finished();
        // "The service deployment stops when ... a user manually stops the deployment" or
        // the service fails with no rollback in use — the ends this slice records as
        // [`SERVICE_DEPLOYMENT_STOPPED`], while a rollout that reached the steady state it
        // was rolling out COMPLETED instead. One stored instant ends the rollout either
        // way, and the status it ended in is what decides which member reports it.
        let stopped = record.status == SERVICE_DEPLOYMENT_STOPPED;
        let target_service_revision = match service {
            Some(service) => self.service_revision_summary(
                service,
                &record.deployment_id,
                &record.target_service_revision_arn,
            ),
            // The service record outlives its deployments, so this is the tombstone-free
            // case of a history whose service is gone: the revision is still named, and
            // nothing holds a task of it.
            None => ServiceRevisionSummary {
                arn: Some(record.target_service_revision_arn.clone()),
                ..ServiceRevisionSummary::default()
            },
        };
        ServiceDeployment {
            service_deployment_arn: Some(record.arn.clone()),
            service_arn: Some(record.service_arn.clone()),
            cluster_arn: Some(record.cluster_arn.clone()),
            created_at: Some(record.created_at),
            // The rollout begins with the call that created it — the same instant the
            // brief reports as its `startedAt`.
            started_at: Some(record.created_at),
            finished_at: record.finished_at,
            stopped_at: record.finished_at.filter(|_| stopped),
            // "The time that the service deployment was last updated": the record is
            // written by the call that starts the rollout and written again only where
            // that rollout ends, so its last change is one of those two instants.
            updated_at: Some(record.finished_at.unwrap_or(record.created_at)),
            source_service_revisions: Some(
                service
                    .map(|service| self.source_service_revisions(service, record))
                    .unwrap_or_default(),
            ),
            target_service_revision: Some(target_service_revision),
            status: Some(record.status.clone()),
            status_reason: Some(record.status_reason.clone()),
            // "The current lifecycle stage of the deployment": the stage a rollout stands
            // at is the one its waiting PAUSE hook pauses it at. The stages between two
            // hooked ones are passed through within the span the rollout takes to reach the
            // next hook, so a rollout that waits at none stands at none.
            lifecycle_stage: record
                .awaiting_hook()
                .map(|hook| hook.stage.clone()),
            // "The details of the lifecycle hooks for the current service deployment" —
            // every PAUSE hook this rollout has reached, and nothing at all for a rollout
            // whose configuration set none.
            lifecycle_hook_details: (!record.lifecycle_hooks.is_empty()).then(|| {
                record
                    .lifecycle_hooks
                    .iter()
                    .map(lifecycle_hook_detail)
                    .collect()
            }),
            deployment_configuration: Some(record.deployment_configuration.clone()),
            // "The rollback options the service deployment uses when the deployment
            // fails": carried by exactly the rollouts a rollback ended.
            rollback: record.rollback.clone(),
            deployment_circuit_breaker: Some(deployment_circuit_breaker_view(
                &record.deployment_configuration,
                // "the latest service desired count" — the count the threshold is
                // computed from, which is the service's own and not a count this
                // deployment once asked for.
                service.map_or(0, |service| service.desired_count),
                // "The number of times the circuit breaker detected a service deploymeny
                // failure": the launches THIS rollout lost, counted off its own tasks under
                // the reset rule its own configuration set.
                service.map_or(0, |service| {
                    self.deployment_launch_failures(
                        service,
                        &record.deployment_id,
                        circuit_breaker_resets_on_healthy_task(&record.deployment_configuration),
                    )
                    .count
                }),
                record.circuit_breaker_triggered,
                finished,
            )),
            alarms: Some(deployment_alarms_view(
                &record.deployment_configuration,
                finished,
            )),
        }
    }

    /// The `ServiceRevisionSummary` of the revision `revision_arn` names, as the
    /// deployment `deployment_id` of `service` rolls it out.
    ///
    /// Every count is read off the records that exist: the tasks that deployment holds are
    /// COUNTED off the task map, and the count it was asked for is the desired count of
    /// the service while it is the current rollout, the count the superseded entry kept
    /// while its task set is still being replaced, and none once it holds nothing.
    pub(super) fn service_revision_summary(
        &self,
        service: &ServiceRecord,
        deployment_id: &str,
        revision_arn: &str,
    ) -> ServiceRevisionSummary {
        let tally = self.deployment_tally(&service.cluster_name, deployment_id);
        let requested_task_count = if service.deployment_id == deployment_id {
            service.desired_count
        } else {
            service
                .superseded_deployments
                .iter()
                .find(|superseded| superseded.id == deployment_id)
                .map_or(0, |superseded| superseded.desired_count)
        };
        ServiceRevisionSummary {
            arn: Some(revision_arn.to_string()),
            requested_task_count,
            running_task_count: tally.running,
            pending_task_count: tally.pending,
            // "The percentage of test traffic that is directed to this service revision" /
            // "... of production traffic": a share of a traffic shift between two running
            // revisions, which the rollout this slice carries out does not perform, so
            // neither weight is reported.
            requested_test_traffic_weight: None,
            requested_production_traffic_weight: None,
        }
    }

    /// The revisions a rollout of `service` is moving it FROM.
    ///
    /// `ServiceDeployment$sourceServiceRevisions` is "the currently deployed workload
    /// configuration" — the revision the service was running when this rollout started,
    /// which is the one the rollout just before it released. The FIRST rollout of a
    /// service replaces nothing, so it has no source at all.
    pub(super) fn source_service_revisions(
        &self,
        service: &ServiceRecord,
        record: &ServiceDeploymentRecord,
    ) -> Vec<ServiceRevisionSummary> {
        self.service_deployments
            .values()
            .filter(|other| {
                other.service_arn == record.service_arn && other.order_key() < record.order_key()
            })
            .max_by(|left, right| left.order_key().cmp(&right.order_key()))
            .map(|previous| {
                self.service_revision_summary(
                    service,
                    &previous.deployment_id,
                    &previous.target_service_revision_arn,
                )
            })
            .into_iter()
            .collect()
    }

    /// Mint the container records of ONE task of `definition`, each with its own ARN and
    /// id. Shared by every path that places a task — `RunTask` and the service
    /// scheduler — so a task's containers are derived from the stored revision the same
    /// way whoever placed it.
    pub fn mint_containers(
        &mut self,
        region: &str,
        cluster_name: &str,
        task_id: &str,
        task_arn: &str,
        definition: &TaskDefinitionRecord,
        last_status: &str,
    ) -> Vec<ContainerRecord> {
        definition
            .container_definitions
            .iter()
            .map(|container| {
                let container_id = self.next_container_id();
                ContainerRecord {
                    container_arn: container_arn(region, cluster_name, task_id, &container_id),
                    task_arn: task_arn.to_string(),
                    name: container.name.clone(),
                    image: container.image.clone(),
                    last_status: last_status.to_string(),
                    // `Container$cpu`: "The value is 0 if no value was specified in the
                    // container definition" — which is what the modelled non-optional
                    // `cpu` already holds.
                    cpu: Some(container.cpu.to_string()),
                    memory: container.memory.map(|memory| memory.to_string()),
                    memory_reservation: container
                        .memory_reservation
                        .map(|memory| memory.to_string()),
                    // Nothing has run the container yet, so the agent has reported
                    // neither a runtime id nor an outcome for it.
                    runtime_id: None,
                    exit_code: None,
                    reason: None,
                    network_bindings: Vec::new(),
                    // Nothing has pulled the image or started a managed agent of the
                    // container either, so the agent has reported neither.
                    image_digest: None,
                    managed_agents: Vec::new(),
                    // `ContainerDefinition$essential`: "If this parameter is omitted, a
                    // container is assumed to be essential."
                    essential: container.essential.unwrap_or(true),
                    depends_on: container.depends_on.clone().unwrap_or_default(),
                    // The health check the REVISION declared, copied onto the container it
                    // is of, so the probes a task's container answers are the ones the
                    // revision that task runs declared
                    // ([`ContainerHealthRecord::from_definition`]).
                    health: ContainerHealthRecord::from_definition(container.health_check.as_ref()),
                }
            })
            .collect()
    }

    /// Record the placement every task PARKED in `cluster_name` is now owed, because the
    /// capacity its capacity provider had to acquire has just registered into the cluster.
    ///
    /// The transition falls due [`TRANSITION_SPAN`] from `now` and
    /// [`Self::advance_clock`] materialises it, so nothing here moves a status that is not
    /// owed yet. A task that cannot be launched before the interface it was allocated is in
    /// place waits for that too: the attach of such an interface BEGINS with the placement
    /// — nothing was attaching one while the task had nowhere to run
    /// ([`AttachmentRecord::begin_attaching`]) — so the placement falls due at the later of
    /// the two instants. A task the caller has already asked to stop is not placed: its
    /// placement was cancelled, not merely overtaken.
    pub fn place_tasks_awaiting_capacity(&mut self, cluster_name: &str, now: SystemTime) {
        for task in self.tasks.values_mut() {
            if task.cluster_name == cluster_name
                && task.desired_status == STATUS_RUNNING
                && task.is_awaiting_placement()
            {
                let placed_at = now + TRANSITION_SPAN;
                let mut attached_at: Option<SystemTime> = None;
                for attachment in &mut task.attachments {
                    attachment.begin_attaching(now);
                    if let Some(at) = attachment.attached_due_at() {
                        attached_at = Some(attached_at.map_or(at, |seen: SystemTime| seen.max(at)));
                    }
                }
                task.pending_due_at = Some(attached_at.unwrap_or(placed_at).max(placed_at));
            }
        }
    }

    /// How many container instances `cluster_name` holds in a registered status.
    pub fn registered_container_instances(&self, cluster_name: &str) -> usize {
        self.container_instances
            .values()
            .filter(|instance| instance.cluster_name == cluster_name && instance.is_registered())
            .count()
    }

    /// The container instance of `cluster_name` the scheduler places a task of `launch_type`
    /// on, or `None` when that launch type is not hosted on the cluster's own capacity (a
    /// Fargate task runs on capacity AWS owns, so it has no container instance at all) or
    /// when the cluster has no instance that accepts a placement right now.
    ///
    /// "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
    /// cluster" ([`needs_registered_capacity`]) — so such a task HAS a host, and
    /// `Task$containerInstanceArn` is "the ARN of the container instance that hosts the
    /// task". Which instance is the scheduler's choice: an instance it may place on at all
    /// ([`ContainerInstanceRecord::accepts_task_placement`], which excludes a `DRAINING`
    /// one — "prevents new tasks from being scheduled for placement on the container
    /// instance" — and one whose agent is disconnected), and of those the one carrying the
    /// FEWEST of the cluster's tasks, which is the `spread` placement that is the service
    /// scheduler's default ("the default for the service scheduler is spread"). A tie is
    /// broken by the instance's own ARN, so one cluster's placements are deterministic.
    pub fn select_task_host(
        &self,
        cluster_name: &str,
        launch_type: Option<&str>,
    ) -> Option<String> {
        if !needs_registered_capacity(launch_type) {
            return None;
        }
        self.container_instances
            .values()
            .filter(|instance| {
                instance.cluster_name == cluster_name && instance.accepts_task_placement()
            })
            .min_by_key(|instance| {
                let tally = self.container_instance_tally(&instance.arn);
                (tally.running + tally.pending, instance.arn.clone())
            })
            .map(|instance| instance.arn.clone())
    }

    /// Stop the SERVICE-managed tasks hosted on the container instance `instance_arn`,
    /// because it is being taken out of service.
    ///
    /// `UpdateContainerInstancesState` on a `DRAINING` instance: "When you set a container
    /// instance to `DRAINING`, Amazon ECS prevents new tasks from being scheduled for
    /// placement on the container instance and replacement service tasks are started on
    /// other container instances in the cluster if the resources are available. ... Any
    /// tasks that you run manually (`RunTask`) aren't affected by the container instance
    /// transitioning to `DRAINING` status." So the stop is made HERE for the tasks a service
    /// owns, and the replacements are the scheduler's own steady-state work
    /// ([`Self::maintain_service_task_counts`], which will not choose this instance again
    /// because it no longer accepts a placement).
    ///
    /// A task already on its way out is left alone, which is what
    /// [`Self::stop_service_task`] decides.
    pub fn drain_container_instance_tasks(&mut self, instance_arn: &str, now: SystemTime) {
        let hosted: Vec<String> = self
            .tasks
            .values()
            .filter(|task| {
                task.container_instance_arn.as_deref() == Some(instance_arn)
                    && task.service_name.is_some()
                    && task.desired_status == STATUS_RUNNING
                    && task.is_active()
            })
            .map(|task| task.task_arn.clone())
            .collect();
        for task_arn in hosted {
            self.stop_service_task(&task_arn, now, DRAINING_INSTANCE_STOP_REASON);
        }
    }

    /// The stored key of the container instance a `containerInstance` reference names,
    /// if this store holds it in `cluster_name`.
    ///
    /// Every such member's doc: "The container instance ID or full ARN of the container
    /// instance" — so a reference is either form — and the `cluster` member scopes the
    /// request: "If you do not specify a cluster, the default cluster is assumed." An
    /// instance registered into another cluster is not one this request names.
    pub fn resolve_container_instance(
        &self,
        region: &str,
        cluster_name: &str,
        reference: &str,
    ) -> Option<&ContainerInstanceRecord> {
        let arn = container_instance_arn_from_reference(region, cluster_name, reference);
        self.container_instances
            .get(&arn)
            .filter(|instance| instance.cluster_name == cluster_name)
    }

    /// The `ContainerInstance` a response reports for the instance stored under `arn`, and
    /// `None` when this store holds none.
    ///
    /// The ONE projection of a container instance: the stored record plus the counters
    /// read off the tasks placed on it ([`Self::container_instance_tally`]), so every
    /// operation that reports an instance reports the same derived numbers.
    pub fn container_instance_view(
        &self,
        arn: &str,
        fields: ContainerInstanceFields,
    ) -> Option<ContainerInstance> {
        let instance = self.container_instances.get(arn)?;
        Some(instance.view(fields, self.container_instance_tally(arn)))
    }

    /// What the task map counts for ONE container instance: the tasks placed on it that
    /// are still wanted running, split by lifecycle status —
    /// `ContainerInstance$runningTasksCount` is "the number of tasks on the container
    /// instance that are in the RUNNING status" and `pendingTasksCount` the same for
    /// `PENDING`.
    pub fn container_instance_tally(&self, instance_arn: &str) -> ServiceTally {
        self.tally_wanted(|task| task.container_instance_arn.as_deref() == Some(instance_arn))
    }

    /// The latest `ACTIVE` revision registered for `family`, if any.
    pub fn latest_active_revision(&self, family: &str) -> Option<&TaskDefinitionRecord> {
        self.task_definitions
            .values()
            .filter(|record| record.family == family && record.status == TASK_DEFINITION_ACTIVE)
            .max_by_key(|record| record.revision)
    }

    /// The latest `ACTIVE` daemon revision registered for `family`, if any. The
    /// `DescribeDaemonTaskDefinition` doc: "you can simply specify the family to find the
    /// latest ACTIVE revision in that family".
    pub fn latest_active_daemon_revision(
        &self,
        family: &str,
    ) -> Option<&DaemonTaskDefinitionRecord> {
        self.daemon_task_definitions
            .values()
            .filter(|record| {
                record.family == family && record.status == DAEMON_TASK_DEFINITION_ACTIVE
            })
            .max_by_key(|record| record.revision)
    }

    /// The revision a `taskDefinition` reference names for a call that would put it to
    /// WORK — run a task of it, or point a service at it — or the `ClientException`
    /// message that refuses the reference.
    ///
    /// Two conditions, one per [`TaskDefinitionRefusal`]. A reference that resolves to no
    /// stored revision is [`TaskDefinitionRefusal::NotFound`]; one that resolves to a
    /// revision whose status is not [`TASK_DEFINITION_ACTIVE`] is
    /// [`TaskDefinitionRefusal::Inactive`], because "you can't use an `INACTIVE` task
    /// definition to run new tasks or create new services, and you can't update an
    /// existing service to reference an `INACTIVE` task definition" — a deregistered
    /// revision stays readable (`DescribeTaskDefinition` keeps describing it) but is no
    /// longer usable. A reference that names no revision takes "the latest `ACTIVE`
    /// revision", so it can only ever resolve to a usable one.
    ///
    /// A service ALREADY pointed at a revision is untouched by this: it keeps the key it
    /// stored ("Existing services that reference an `INACTIVE` task definition ... can
    /// still scale up or down by modifying the service's desired count"), which is why
    /// this answers only a reference the REQUEST carries.
    pub fn usable_revision(
        &self,
        family: &str,
        revision: Option<i32>,
    ) -> Result<&TaskDefinitionRecord, TaskDefinitionRefusal> {
        match revision {
            None => self
                .latest_active_revision(family)
                .ok_or(TaskDefinitionRefusal::NotFound),
            Some(revision) => match self.task_definitions.get(&(family.to_string(), revision)) {
                None => Err(TaskDefinitionRefusal::NotFound),
                Some(record) if record.status != TASK_DEFINITION_ACTIVE => {
                    Err(TaskDefinitionRefusal::Inactive)
                }
                Some(record) => Ok(record),
            },
        }
    }

    /// What the task map counts for ONE service: the tasks its scheduler placed and
    /// still wants, split by lifecycle status. `Service$runningCount` is "the number of
    /// tasks in the cluster that are in the RUNNING state" and `pendingCount` the same
    /// for `PENDING`; a task that has been asked to stop is no longer one of them.
    pub fn service_tally(&self, cluster_name: &str, service_name: &str) -> ServiceTally {
        self.tally_wanted(|task| {
            task.cluster_name == cluster_name && task.service_name.as_deref() == Some(service_name)
        })
    }

    /// The RUNNING/PENDING split of every task `wanted` selects that is still wanted
    /// RUNNING — defined once, so the service's counts and a task set's cannot disagree
    /// about what each status counts.
    pub(super) fn tally_wanted(&self, wanted: impl Fn(&TaskRecord) -> bool) -> ServiceTally {
        let mut tally = ServiceTally::default();
        for task in self
            .tasks
            .values()
            .filter(|task| task.desired_status == STATUS_RUNNING && wanted(task))
        {
            if task.last_status == STATUS_RUNNING {
                tally.running += 1;
            } else if task.is_pending() {
                tally.pending += 1;
            }
        }
        tally
    }

    /// What the task map counts for ONE deployment of a service: the tasks that deployment
    /// placed and that have not stopped yet.
    ///
    /// A service's task carries the id of the deployment that placed it as its `startedBy`
    /// ([`ServiceRecord::deployment_id`]), which is what tells a superseded deployment's
    /// remaining tasks from the current one's. Tasks being replaced are counted here even
    /// though they are on their way out: they are what makes that deployment one that
    /// "still has running tasks".
    pub(super) fn deployment_tally(&self, cluster_name: &str, deployment_id: &str) -> ServiceTally {
        let mut tally = ServiceTally::default();
        for task in self.tasks.values().filter(|task| {
            task.cluster_name == cluster_name && task.started_by.as_deref() == Some(deployment_id)
        }) {
            if task.last_status == STATUS_RUNNING {
                tally.running += 1;
            } else if task.last_status != STATUS_STOPPED {
                tally.pending += 1;
            }
        }
        tally
    }

    /// The operating-system family the tasks of `service` run on, as an `OSFamily` value.
    ///
    /// DERIVED from the revision the service runs: `RuntimePlatform$operatingSystemFamily`
    /// is what a task definition declares its operating system as, and a revision that
    /// declares no `runtimePlatform` runs on [`DEFAULT_OS_FAMILY`] — which is also what a
    /// service that runs no revision of its own (its tasks come from a task set) reports.
    pub(super) fn service_os_family(&self, service: &ServiceRecord) -> String {
        service
            .task_definition
            .as_ref()
            .and_then(|key| self.task_definitions.get(key))
            .and_then(|definition| definition.runtime_platform.as_ref())
            .and_then(|platform| platform.operating_system_family.clone())
            .unwrap_or_else(|| DEFAULT_OS_FAMILY.to_string())
    }

    /// The ARN of the IAM role `Service$roleArn` reports — "The ARN of the IAM role
    /// that's associated with the service" — or `None` for a service that has none.
    ///
    /// The role the create named, when it named one. Otherwise the ECS SERVICE-LINKED
    /// role, for exactly the services `CreateService$role` says need one: "The
    /// service-linked role is required if your task definition uses the `awsvpc` network
    /// mode or if the service is configured to use service discovery, an external
    /// deployment controller, multiple target groups, or Elastic Inference accelerators in
    /// which case you don't specify a role here." A service outside that list that named
    /// no role has no role associated with it, so it reports none.
    pub(super) fn service_role_arn(&self, service: &ServiceRecord) -> Option<String> {
        if let Some(requested) = service.role_arn.clone() {
            return Some(requested);
        }
        let awsvpc = service
            .task_definition
            .as_ref()
            .and_then(|key| self.task_definitions.get(key))
            .is_some_and(|definition| definition.network_mode == NETWORK_MODE_AWSVPC);
        let requires_service_linked_role = awsvpc
            || !service.places_tasks()
            || !service.service_registries.is_empty()
            || service.load_balancers.len() > 1;
        requires_service_linked_role.then(service_linked_role_arn)
    }

    /// The `Service` every service-returning operation reports, built from what is
    /// stored.
    ///
    /// `runningCount` and `pendingCount` are COUNTED off the task map, so they always
    /// agree with the tasks the scheduler actually placed, and the deployment's
    /// `rolloutState` follows from the steady state the CLOCK settled rather than being
    /// asserted.
    ///
    /// Which members a service carries follows its DEPLOYMENT CONTROLLER, not the
    /// operation that reports it: only the ECS controller's own scheduler owns a
    /// deployment and a service revision (`Deployment$rolloutState` is "only returned for
    /// services that use the rolling update (ECS) deployment type"), while the other
    /// controllers' tasks come from a task set (`CreateService$service`: "if the service
    /// uses the CODE_DEPLOY deployment controller, the deploymentController, taskSets and
    /// deployments parameters will be returned, however the deployments parameter will be
    /// an empty list"). `deploymentController` itself is reported for EVERY service: the
    /// recorded responses of the real service carry it for an ECS-controller service too,
    /// which is what the same doc's "will not be returned" sentence no longer matches.
    pub fn service_view(&self, service: &ServiceRecord, fields: ServiceFields) -> Service {
        let tally = self.service_tally(&service.cluster_name, &service.name);
        let scheduler_owned = service.places_tasks();
        let settled = service.deployment_steady_due_at.is_none();
        // "A service will return either a capacityProviderStrategy or launchType
        // parameter, but not both, depending where one was specified when it was
        // created."
        let (launch_type, capacity_provider_strategy) = match &service.launch_type {
            Some(launch_type) => (Some(launch_type.clone()), None),
            None => (None, Some(service.capacity_provider_strategy.clone())),
        };
        // The Fargate-only members: "A platform family is specified only for tasks using
        // the Fargate launch type", and so is a platform version. A service that does not
        // run on Fargate carries neither, whatever its request happened to say.
        let fargate = service.runs_on_fargate();
        let os_family = self.service_os_family(service);
        // "The operating system that your tasks in the service run on", derived from the
        // revision's `runtimePlatform` and reported in its display spelling.
        let platform_family = fargate.then(|| os_family_display(&os_family));
        // "If one isn't specified, the LATEST platform version is used": the SERVICE
        // reports the version it asks for, which is that alias when it named none, while
        // its DEPLOYMENT reports the version that alias resolves to for its operating
        // system — the version the tasks of the rollout actually run.
        let requested_platform_version = fargate.then(|| {
            service
                .platform_version
                .clone()
                .unwrap_or_else(|| PLATFORM_VERSION_LATEST.to_string())
        });
        let deployment_platform_version = requested_platform_version
            .as_deref()
            .map(|requested| resolved_platform_version(requested, &os_family));
        let service_tasks = self.service_tasks(service);
        // When each of the service's deployments started, newest first: the primary one
        // and the ones it is still replacing.
        let mut generations: Vec<(&str, DateTime)> =
            vec![(service.deployment_id.as_str(), service.deployment_created_at)];
        generations.extend(
            service
                .superseded_deployments
                .iter()
                .map(|superseded| (superseded.id.as_str(), superseded.created_at)),
        );
        let counters = Self::deployment_counters(
            &service_tasks,
            &service.deployment_id,
            service.deployment_created_at,
            &generations,
        );
        // What the deployment circuit breaker has counted for this rollout, whether or not
        // one is watching it: `Deployment$failedTasks` is "the number of consecutively
        // failed tasks in the deployment", which is the same count
        // ([`Self::deployment_launch_failures`]).
        let failures = Self::launch_failures_of(
            &service_tasks,
            &service.deployment_id,
            circuit_breaker_resets_on_healthy_task(&service.deployment_configuration),
        );
        let deployment = Deployment {
            id: Some(service.deployment_id.clone()),
            status: Some(DEPLOYMENT_PRIMARY.to_string()),
            task_definition: service.task_definition_arn.clone(),
            desired_count: service.desired_count,
            pending_count: tally.pending,
            running_count: tally.running,
            // "The number of consecutively failed tasks in the deployment. A task is
            // considered a failure if the service scheduler can't launch the task, the task
            // doesn't transition to a RUNNING state" — counted off the launches this rollout
            // lost rather than asserted.
            failed_tasks: failures.count,
            healthy_task_count: counters.healthy,
            failed_launch_task_count: counters.failed_launch,
            replaced_task_count: counters.replaced,
            total_failed_tasks: counters.total_failed,
            // The share of the service's desired count this deployment is scaled to: the
            // whole of it once the rollout has taken the service over, and none of it
            // while the rollout is still under way.
            scale: Some(deployment_scale(service.deployment_rollout_completed)),
            // Stable when the scheduler's confirmation is in AND the deployment holds the
            // tasks the service wants, which is what the `stabilityStatus` of a task set
            // reads ("The task `runningCount` is equal to the `computedDesiredCount`",
            // "The `pendingCount` is 0"); anything else is still stabilizing.
            stability_status: Some(
                if settled
                    && service.deployment_stable_due_at.is_none()
                    && tally.running == service.desired_count
                    && tally.pending == 0
                {
                    STABILITY_STATUS_STEADY_STATE
                } else {
                    STABILITY_STATUS_STABILIZING
                }
                .to_string(),
            ),
            created_at: Some(service.deployment_created_at),
            updated_at: Some(service.deployment_updated_at),
            capacity_provider_strategy: capacity_provider_strategy.clone(),
            launch_type: launch_type.clone(),
            platform_version: deployment_platform_version,
            platform_family: platform_family.clone(),
            network_configuration: service.network_configuration.clone(),
            // "When a service deployment is started, it begins in an IN_PROGRESS state.
            // When the service reaches a steady state, the deployment transitions to a
            // COMPLETED state. If the service fails to reach a steady state and circuit
            // breaker is turned on, the deployment transitions to a FAILED state" — the
            // steady state is the clock-owed transition the record holds the due instant
            // of, so a rollout the scheduler has not settled yet is IN_PROGRESS whatever
            // the counts already say, and one the breaker failed carries the terminal
            // state and the reason it failed for.
            rollout_state: Some(
                match &service.deployment_rollout_failure {
                    Some(_) => ROLLOUT_FAILED,
                    None if settled => ROLLOUT_COMPLETED,
                    None => ROLLOUT_IN_PROGRESS,
                }
                .to_string(),
            ),
            rollout_state_reason: Some(
                service
                    .deployment_rollout_failure
                    .clone()
                    .unwrap_or_else(|| rollout_state_reason(&service.deployment_id, settled)),
            ),
            service_connect_configuration: service.service_connect_configuration.clone(),
            // A Service Connect resource is registered in Cloud Map, which this
            // emulator does not provision, so a deployment has none.
            service_connect_resources: Some(Vec::new()),
            volume_configurations: Some(service.volume_configurations.clone()),
            fargate_ephemeral_storage: None,
            vpc_lattice_configurations: Some(service.vpc_lattice_configurations.clone()),
        };
        Service {
            service_arn: Some(service.arn.clone()),
            service_name: Some(service.name.clone()),
            cluster_arn: Some(service.cluster_arn.clone()),
            load_balancers: Some(service.load_balancers.clone()),
            service_registries: Some(service.service_registries.clone()),
            status: Some(service.status.clone()),
            desired_count: service.desired_count,
            running_count: tally.running,
            pending_count: tally.pending,
            launch_type,
            capacity_provider_strategy,
            platform_version: requested_platform_version,
            platform_family,
            task_definition: service.task_definition_arn.clone(),
            deployment_configuration: Some(service.deployment_configuration.clone()),
            task_sets: if scheduler_owned {
                None
            } else {
                // "If the service uses the CODE_DEPLOY deployment controller, the
                // deploymentController, taskSets and deployments parameters will be
                // returned": the task sets the service actually holds, which for a
                // controller that is not the ECS scheduler is where its tasks come from.
                Some(
                    self.service_task_sets(&service.cluster_name, &service.name)
                        .into_iter()
                        .map(|task_set| self.task_set_view(task_set, fields.into()))
                        .collect(),
                )
            },
            deployments: Some(if scheduler_owned {
                // The current rollout, then the deployments it is replacing — which are
                // reported for as long as they still hold a task of their own
                // (`Deployment$status`: "ACTIVE: A service deployment that still has
                // running tasks, but are in the process of being replaced").
                let mut deployments = vec![deployment];
                deployments.extend(service.superseded_deployments.iter().map(|superseded| {
                    self.superseded_deployment_view(
                        service,
                        superseded,
                        &service_tasks,
                        &generations,
                    )
                }));
                deployments
            } else {
                Vec::new()
            }),
            role_arn: self.service_role_arn(service),
            // "The event stream for your service. A maximum of 100 of the latest events
            // are displayed" — newest first, which is the order the service's own event
            // stream reads in.
            events: Some(
                service
                    .events
                    .iter()
                    .rev()
                    .take(MAX_SERVICE_EVENTS)
                    .map(ServiceEventRecord::view)
                    .collect(),
            ),
            created_at: Some(service.created_at),
            // "The ARN of the service deployment" the service is on now: the record the
            // call that started this rollout wrote, which is the same deployment the
            // `deployments` list above reports and the one `ListServiceDeployments`
            // answers with.
            current_service_deployment: self
                .current_service_deployment(service)
                .map(|deployment| deployment.arn.clone()),
            current_service_revisions: if scheduler_owned {
                Some(vec![ServiceCurrentRevisionSummary {
                    arn: Some(service.service_revision_arn.clone()),
                    requested_task_count: service.desired_count,
                    running_task_count: tally.running,
                    pending_task_count: tally.pending,
                }])
            } else {
                None
            },
            placement_constraints: Some(service.placement_constraints.clone()),
            placement_strategy: Some(service.placement_strategy.clone()),
            network_configuration: service.network_configuration.clone(),
            health_check_grace_period_seconds: service.health_check_grace_period_seconds,
            scheduling_strategy: Some(service.scheduling_strategy.clone()),
            deployment_controller: Some(service.deployment_controller.clone()),
            // `DescribeServices$include`: "Determines whether you want to see the resource
            // tags for the service" — the member is reported only when the request asked
            // for the tags AND the service carries some, as the recorded responses of the
            // real service do: an untagged service omits the key rather than reporting an
            // empty list.
            tags: if fields.tags && !service.tags.is_empty() {
                Some(service.tags.clone())
            } else {
                None
            },
            // "The principal that created the service": the principal of the request
            // that created it, as stored — the same principal
            // `TaskDefinition$registeredBy` names.
            created_by: Some(service.created_by.clone()),
            enable_ecs_managed_tags: service.enable_ecs_managed_tags,
            propagate_tags: Some(service.propagate_tags.clone()),
            enable_execute_command: service.enable_execute_command,
            availability_zone_rebalancing: Some(service.availability_zone_rebalancing.clone()),
            resource_management_type: Some(service.resource_management_type().to_string()),
            // The version of the configuration the service runs, as stored: it advances
            // with every new service revision the service rolls out.
            version: service.version,
            // What the scheduler still owes the service, for a service it has run at all.
            run_status: self.service_run_status(service),
            // The extra time period the service's current deployment has been granted.
            // Nothing grants one: no operation of this API extends a deployment's time
            // period, so every service reports none — a true zero rather than an invented
            // number.
            extend_deployment_time_period: 0,
        }
    }

    /// `Service$runStatus`: what the scheduler still owes this service, or `None` for a
    /// service it has not run at all.
    ///
    /// The member is not in the Smithy model, so its readings come from the recorded
    /// responses of the real service: the tombstone a finished deletion leaves reports
    /// [`RUN_STATUS_TOMBSTONE`], a service whose scheduler still has a task or a task set
    /// to place or let go of reports [`RUN_STATUS_MORE_WORK`], and one with nothing
    /// outstanding reports [`RUN_STATUS_STEADY_STATE`]. A service the scheduler has not
    /// worked on yet — freshly created, neither settled nor being deleted — has no run to
    /// report and carries no run status at all.
    pub(super) fn service_run_status(&self, service: &ServiceRecord) -> Option<String> {
        if service.status == SERVICE_INACTIVE {
            return Some(RUN_STATUS_TOMBSTONE.to_string());
        }
        let deleting = service.status != SERVICE_ACTIVE;
        if !deleting && !service.reached_steady_state {
            return None;
        }
        Some(
            if self.service_has_outstanding_work(service) {
                RUN_STATUS_MORE_WORK
            } else {
                RUN_STATUS_STEADY_STATE
            }
            .to_string(),
        )
    }

    /// Whether the scheduler still has work to do for `service`.
    ///
    /// A service whose deletion has started still has to let go of the TASKS it holds ("if
    /// there are still running tasks that require cleanup, the service status moves from
    /// ACTIVE to DRAINING") — and a service that DID deploy through a task set and holds
    /// none any more has lost the deployment that served its traffic and still owes what
    /// that deployment held; while one of them is still draining the service holds it yet,
    /// and reports nothing outstanding beyond it. Recorded behavior, as `Service$runStatus`
    /// itself is: the recorded teardowns report work outstanding where the service's task
    /// sets have gone and the terminal steady state where one is still draining away.
    ///
    /// A live service's work is the difference between the task set it wants and the one it
    /// has: a task still coming up, or a count that is not yet the one it was asked for.
    pub(super) fn service_has_outstanding_work(&self, service: &ServiceRecord) -> bool {
        if service.status != SERVICE_ACTIVE {
            // The second disjunct reads the opposite way round from the first, and is
            // meant to: the recorded teardown of a service that still holds a draining
            // task set reports the terminal steady state, while the recorded teardowns
            // whose task sets have gone report work outstanding (the replay clusters
            // `ecs|DeleteService|2xx->2xx|$.service.runStatus` and
            // `ecs|DeleteService|2xx->2xx|$.service.taskSets[*]` are what fix the pair).
            return self.service_has_active_tasks(&service.cluster_name, &service.name)
                || (service.held_task_set
                    && self
                        .service_task_sets(&service.cluster_name, &service.name)
                        .is_empty());
        }
        let tally = self.service_tally(&service.cluster_name, &service.name);
        tally.pending > 0 || (service.places_tasks() && tally.running != service.desired_count)
    }

    /// Every task of ONE service, collected once so the counts of its deployments cost one
    /// pass over the task map rather than one per deployment.
    pub(super) fn service_tasks(&self, service: &ServiceRecord) -> Vec<&TaskRecord> {
        self.tasks
            .values()
            .filter(|task| {
                task.cluster_name == service.cluster_name
                    && task.service_name.as_deref() == Some(service.name.as_str())
            })
            .collect()
    }

    /// What the task map counts for ONE deployment of a service beyond its RUNNING/PENDING
    /// split ([`Self::deployment_tally`]): the tasks it holds that count as healthy, the
    /// ones it lost, and the ones of an earlier deployment it replaced.
    ///
    /// Every count is read off the task records, so none of them can drift from the tasks
    /// that exist. `tasks` is the service's own tasks, collected ONCE per reported service
    /// ([`Self::service_tasks`]) and shared by every deployment of it. `created_at` is the
    /// instant the deployment started, and `generations` is when each of the service's
    /// deployments started: a task counts as REPLACED by this deployment only when it
    /// stopped since this deployment started and the deployment that placed it is not one
    /// that started later, so a deployment is never credited with replacing the tasks of
    /// the rollout that replaced IT.
    pub(super) fn deployment_counters(
        tasks: &[&TaskRecord],
        deployment_id: &str,
        created_at: DateTime,
        generations: &[(&str, DateTime)],
    ) -> DeploymentCounters {
        let instant = |at: DateTime| (at.secs(), at.subsec_nanos());
        let started = instant(created_at);
        let mut counters = DeploymentCounters::default();
        for task in tasks {
            if task.started_by.as_deref() == Some(deployment_id) {
                // "Tasks for services that don't use a load balancer are considered
                // healthy if they're in the RUNNING state" — a load-balanced service's
                // tasks additionally have to be reported healthy by the load balancer,
                // which is the load balancer's report and not something the service
                // holds, so a task's own status is what this counts either way.
                if task.last_status == STATUS_RUNNING {
                    counters.healthy += 1;
                }
                // A task the deployment lost while it still wanted it running is one that
                // failed; one the scheduler asked to stop (scaled in, replaced, drained
                // away with its service) is not. A task Amazon ECS could not launch at all
                // is marked for the stop it earns, so its desired status is the stop's and
                // the stop CODE is what tells the two apart
                // ([`TaskRecord::failed_to_launch`]).
                if task.failed_to_launch()
                    || (task.last_status == STATUS_STOPPED
                        && task.desired_status == STATUS_RUNNING)
                {
                    counters.total_failed += 1;
                    // And one that never reached RUNNING failed to LAUNCH.
                    if task.started_at.is_none() {
                        counters.failed_launch += 1;
                    }
                }
            } else if task.last_status == STATUS_STOPPED
                && task.stopped_at.is_some_and(|at| instant(at) >= started)
                && !generations.iter().any(|(id, at)| {
                    task.started_by.as_deref() == Some(*id) && instant(*at) >= started
                })
            {
                counters.replaced += 1;
            }
        }
        counters
    }

    /// The `Deployment` a SUPERSEDED deployment is reported as: the configuration it was
    /// rolling out, with the tasks it still holds counted off the task map.
    ///
    /// `Deployment$status` [`DEPLOYMENT_ACTIVE`] — it "still has running tasks, but are in
    /// the process of being replaced". Its `rolloutState` is the one that rollout had
    /// reached when it was superseded, not one asserted here.
    pub(super) fn superseded_deployment_view(
        &self,
        service: &ServiceRecord,
        superseded: &SupersededDeployment,
        service_tasks: &[&TaskRecord],
        generations: &[(&str, DateTime)],
    ) -> Deployment {
        let tally = self.deployment_tally(&service.cluster_name, &superseded.id);
        let fargate = places_on_fargate(
            superseded.launch_type.as_deref(),
            &superseded.capacity_provider_strategy,
        );
        let os_family = self.service_os_family(service);
        let platform_version = fargate.then(|| {
            resolved_platform_version(
                superseded
                    .platform_version
                    .as_deref()
                    .unwrap_or(PLATFORM_VERSION_LATEST),
                &os_family,
            )
        });
        let counters =
            Self::deployment_counters(service_tasks, &superseded.id, superseded.created_at, generations);
        let failures = Self::launch_failures_of(
            service_tasks,
            &superseded.id,
            circuit_breaker_resets_on_healthy_task(&service.deployment_configuration),
        );
        Deployment {
            id: Some(superseded.id.clone()),
            status: Some(DEPLOYMENT_ACTIVE.to_string()),
            task_definition: superseded.task_definition_arn.clone(),
            desired_count: superseded.desired_count,
            pending_count: tally.pending,
            running_count: tally.running,
            failed_tasks: failures.count,
            healthy_task_count: counters.healthy,
            failed_launch_task_count: counters.failed_launch,
            replaced_task_count: counters.replaced,
            total_failed_tasks: counters.total_failed,
            // A newer deployment has taken the service over, so none of the service's
            // desired count is this one's share any more.
            scale: Some(deployment_scale(false)),
            // Its task set "is in the process of being replaced", which is work still
            // under way, so it has not reached a steady state.
            stability_status: Some(STABILITY_STATUS_STABILIZING.to_string()),
            created_at: Some(superseded.created_at),
            updated_at: Some(superseded.updated_at),
            capacity_provider_strategy: superseded
                .launch_type
                .is_none()
                .then(|| superseded.capacity_provider_strategy.clone()),
            launch_type: superseded.launch_type.clone(),
            platform_version,
            platform_family: fargate.then(|| os_family_display(&os_family)),
            network_configuration: superseded.network_configuration.clone(),
            rollout_state: Some(
                match &superseded.rollout_failure {
                    Some(_) => ROLLOUT_FAILED,
                    None if superseded.rollout_completed => ROLLOUT_COMPLETED,
                    None => ROLLOUT_IN_PROGRESS,
                }
                .to_string(),
            ),
            rollout_state_reason: Some(superseded.rollout_failure.clone().unwrap_or_else(|| {
                rollout_state_reason(&superseded.id, superseded.rollout_completed)
            })),
            service_connect_configuration: superseded.service_connect_configuration.clone(),
            service_connect_resources: Some(Vec::new()),
            volume_configurations: Some(superseded.volume_configurations.clone()),
            fargate_ephemeral_storage: None,
            vpc_lattice_configurations: Some(superseded.vpc_lattice_configurations.clone()),
        }
    }

    /// The capacity provider `name` names, or `None` when nothing holds that name.
    ///
    /// Either one a caller created or one of the providers Amazon ECS manages for every
    /// account ([`managed_capacity_provider`]), so every operation that resolves a
    /// provider by name sees the same set — and `region` is the request's own, which is
    /// the Region the ARN of a managed provider reports.
    pub fn capacity_provider(&self, name: &str, region: &str) -> Option<CapacityProviderRecord> {
        self.capacity_providers
            .get(name)
            .cloned()
            .or_else(|| managed_capacity_provider(name, region))
    }

    /// Every capacity provider this account holds: the ones a caller created, plus the
    /// ones Amazon ECS manages. A created provider that somehow holds a managed name
    /// shadows it, so a name is reported once.
    pub fn all_capacity_providers(&self, region: &str) -> Vec<CapacityProviderRecord> {
        let mut providers: Vec<CapacityProviderRecord> =
            self.capacity_providers.values().cloned().collect();
        providers.extend(
            MANAGED_CAPACITY_PROVIDERS
                .iter()
                .filter(|name| !self.capacity_providers.contains_key(**name))
                .filter_map(|name| managed_capacity_provider(name, region)),
        );
        providers
    }

    /// The refusal a `capacityProviderStrategy` a request NAMES earns, or `None` when every
    /// entry of it names a capacity provider the placement's own cluster may use.
    ///
    /// `CapacityProviderStrategyItem$capacityProvider` is "The short name or full Amazon
    /// Resource Name (ARN) of the capacity provider", and the item's own doc states what has
    /// to be true of it: "If specifying a capacity provider that uses an Auto Scaling group,
    /// the capacity provider must already be created" — a provider nothing created, and that
    /// is not one Amazon ECS manages ([`Self::capacity_provider`]), places no task, so the
    /// request is refused and nothing is stored. The sentence names the FIRST such entry,
    /// the one entry the caller has to change first. A provider that WAS created still
    /// places nothing in a cluster it is not associated with, which is
    /// [`Self::unassociated_capacity_provider_refusal`].
    ///
    /// The strategy a CLUSTER holds as its default is not checked here: it was resolved
    /// against the same set when it was set on the cluster.
    pub fn capacity_provider_strategy_refusal(
        &self,
        strategy: &[CapacityProviderStrategyItem],
        cluster_name: &str,
        region: &str,
    ) -> Option<String> {
        strategy
            .iter()
            .find_map(|item| {
                let name = capacity_provider_name_from_reference(&item.capacity_provider);
                self.capacity_provider(&name, region)
                    .is_none()
                    .then(|| capacity_provider_not_found_message(&item.capacity_provider))
            })
            .or_else(|| self.unassociated_capacity_provider_refusal(strategy, cluster_name))
    }

    /// The refusal a `capacityProviderStrategy` earns when an entry names a capacity
    /// provider that is not one of the cluster's own, or `None` when every entry is.
    ///
    /// A capacity provider places tasks in a cluster only once it is ASSOCIATED with that
    /// cluster: `PutClusterCapacityProviders` takes "the capacity providers to associate
    /// with the cluster", and `Cluster$capacityProviders` is the list it leaves behind — so
    /// a placement whose strategy names anything outside that list has no capacity to place
    /// on, whether the name is one the account holds or one nothing created
    /// ([`CAPACITY_PROVIDER_NOT_ASSOCIATED_MESSAGE`]). The providers Amazon ECS manages for
    /// every account ([`MANAGED_CAPACITY_PROVIDERS`]) are associated the same way as any
    /// other, so a cluster that was never given them cannot place on them either.
    pub fn unassociated_capacity_provider_refusal(
        &self,
        strategy: &[CapacityProviderStrategyItem],
        cluster_name: &str,
    ) -> Option<String> {
        let held: Vec<String> = self
            .clusters
            .get(cluster_name)
            .map(|cluster| {
                cluster
                    .capacity_providers
                    .iter()
                    .map(|reference| capacity_provider_name_from_reference(reference))
                    .collect()
            })
            .unwrap_or_default();
        strategy
            .iter()
            .any(|item| {
                let name = capacity_provider_name_from_reference(&item.capacity_provider);
                !held.contains(&name)
            })
            .then(|| CAPACITY_PROVIDER_NOT_ASSOCIATED_MESSAGE.to_string())
    }

    /// The cluster an operation may act IN: one that exists and has not been deleted.
    ///
    /// A tombstone (`status` [`CLUSTER_INACTIVE`]) is not one — the `Cluster$status` doc:
    /// "The cluster has been deleted" — so every operation that resolves a `cluster` to
    /// work in refuses it with the same error code it answers for a name that never
    /// existed, and with the sentence that condition carries
    /// ([`Self::cluster_not_found_message`]). The three operations that own the tombstone
    /// read `clusters` directly instead: `DescribeClusters` reports it, `DeleteCluster`
    /// answers idempotently with it, and `CreateCluster` creates a new cluster over it —
    /// and a describe that reports its answers per NAME resolves it through
    /// [`Self::holds_cluster`], since a name it cannot resolve is a failure it reports
    /// rather than a call it refuses.
    pub fn active_cluster(&self, cluster_name: &str) -> Option<&ClusterRecord> {
        self.clusters
            .get(cluster_name)
            .filter(|cluster| cluster.status != CLUSTER_INACTIVE)
    }

    /// Whether [`Self::active_cluster`] names one — the existence check an operation that
    /// only needs to know the cluster is there makes.
    pub fn has_active_cluster(&self, cluster_name: &str) -> bool {
        self.active_cluster(cluster_name).is_some()
    }

    /// Whether this store HOLDS a cluster record under `cluster_name` at all, whatever
    /// status it is in — the existence check a describe that reports its answers per NAME
    /// makes.
    ///
    /// The tombstone a delete leaves is still a record this account holds
    /// (`Cluster$status`: "Clusters with an INACTIVE status may remain discoverable in
    /// your account for a period of time"), and such a describe has a channel for every
    /// name it cannot resolve: the resources the cluster held went with the delete, so each
    /// name the request carries comes back as a `MISSING` failure and the call itself
    /// answers. Only a reference that resolves to NO record leaves the describe nothing to
    /// answer about, which is the condition `ClusterNotFoundException` reports
    /// ([`CLUSTER_NOT_FOUND_MESSAGE`]).
    pub fn holds_cluster(&self, cluster_name: &str) -> bool {
        self.clusters.contains_key(cluster_name)
    }

    /// The Cloud Map namespace `service`'s tasks are registered in through Service
    /// Connect, or `None` for a service that is not in one.
    ///
    /// `ServiceConnectConfiguration$enabled`: "Specifies whether to use Service Connect
    /// with this service" — a service that does not use it joins no namespace. The
    /// namespace is the one the service's own configuration names, and otherwise the
    /// cluster's Service Connect default ("the namespace name ... for use with Service
    /// Connect"), which is what a cluster holds the default FOR.
    pub fn service_namespace(&self, service: &ServiceRecord) -> Option<String> {
        let configuration = service.service_connect_configuration.as_ref()?;
        if !configuration.enabled {
            return None;
        }
        let named = configuration
            .namespace
            .as_deref()
            .filter(|namespace| !namespace.is_empty());
        match named {
            Some(namespace) => Some(namespace.to_string()),
            None => self
                .active_cluster(&service.cluster_name)
                .and_then(|cluster| cluster.service_connect_defaults.as_ref())
                .and_then(|defaults| defaults.namespace.clone()),
        }
    }

    /// Whether this store holds a record that names the namespace `reference` refers to.
    ///
    /// A Cloud Map namespace is not an Amazon ECS resource: what this service knows of one
    /// is the Service Connect configuration that names it — a service's own, or a
    /// cluster's default. A reference no such record names is a namespace this account has
    /// nothing in.
    pub fn knows_namespace(&self, reference: &str) -> bool {
        let key = namespace_key(reference);
        let on_a_cluster = self.clusters.values().any(|cluster| {
            cluster
                .service_connect_defaults
                .as_ref()
                .and_then(|defaults| defaults.namespace.as_deref())
                .is_some_and(|namespace| namespace_key(namespace) == key)
        });
        on_a_cluster
            || self.services.values().any(|service| {
                service
                    .service_connect_configuration
                    .as_ref()
                    .and_then(|configuration| configuration.namespace.as_deref())
                    .is_some_and(|namespace| namespace_key(namespace) == key)
            })
    }

    /// Whether `cluster_name` still holds an `ACTIVE` service — one the caller has NOT
    /// deleted yet, and so one of the cluster's `activeServicesCount`.
    ///
    /// `DeleteCluster`'s `ClusterContainsServicesException`: "You can't delete a cluster
    /// that contains services. First, update the service to reduce its desired task count
    /// to 0, and then delete the service" — so a service already being drained away
    /// (`DRAINING`) or deleted (`INACTIVE`) is not what that refusal is about.
    pub fn cluster_has_active_services(&self, cluster_name: &str) -> bool {
        self.services
            .values()
            .any(|service| service.cluster_name == cluster_name && service.is_active())
    }

    /// Whether `cluster_name` still carries a CLUSTER-SCOPED capacity provider: one
    /// created for this cluster and not deleted.
    ///
    /// `CapacityProvider$cluster`: "Managed instances capacity providers are
    /// cluster-scoped, meaning they can only be used within their associated cluster" — so
    /// such a provider is a resource OF the cluster, and the cluster cannot go while it is
    /// still there ([`CLUSTER_CONTAINS_CLUSTER_SCOPED_PROVIDER_MESSAGE`]). An Auto Scaling
    /// group provider is held by the ACCOUNT and merely associated with clusters, so it
    /// never holds one open; neither does a provider the caller already deleted.
    pub fn cluster_has_cluster_scoped_capacity_providers(&self, cluster_name: &str) -> bool {
        self.capacity_providers.values().any(|provider| {
            provider.status != CAPACITY_PROVIDER_INACTIVE
                && provider.cluster.as_deref() == Some(cluster_name)
        })
    }

    /// Whether the service holds a `PRIMARY` task set — the one
    /// `UpdateServicePrimaryTaskSet` promoted, as `TaskSet$status` records it ("The task
    /// set is serving production traffic", the `PRIMARY` status).
    ///
    /// The precondition `UpdateService` holds a deployment-controller change to: the
    /// controller taking the service over deploys through a task set, so there has to be a
    /// primary one for it to take over.
    pub fn service_has_primary_task_set(&self, cluster_name: &str, service_name: &str) -> bool {
        self.service_task_sets(cluster_name, service_name)
            .iter()
            .any(|task_set| task_set.status == TASK_SET_PRIMARY)
    }

    /// Drain a DELETED service: it keeps nothing running, and the task sets it holds are
    /// drained away with it.
    ///
    /// `DeleteService`: "When you delete a service, if there are still running tasks that
    /// require cleanup, the service status moves from ACTIVE to DRAINING ... After all
    /// tasks have transitioned to either STOPPING or STOPPED status, the service status
    /// moves from DRAINING to INACTIVE." The deletion is what starts that drain, so the
    /// call leaves the service `DRAINING` and the CLOCK finishes it ([`Self::settle_services`]):
    /// the cleanup the second sentence waits for is never done in the same instant the
    /// first one begins. The record is KEPT either way: "Services in the DRAINING or
    /// INACTIVE status can still be viewed with the DescribeServices API operation."
    ///
    /// Deleting a service whose drain already finished changes nothing: the delete is
    /// `@idempotent` and an `INACTIVE` service has nothing left to drain.
    pub fn drain_service(&mut self, key: &(String, String), now: SystemTime) {
        let Some(service) = self.services.get(key) else {
            return;
        };
        if service.status == SERVICE_INACTIVE {
            return;
        }
        let mut task_arns: Vec<String> = self
            .tasks
            .values()
            .filter(|task| {
                task.cluster_name == service.cluster_name
                    && task.service_name.as_deref() == Some(service.name.as_str())
                    && task.is_active()
            })
            .map(|task| task.task_arn.clone())
            .collect();
        task_arns.sort();
        let name = service.name.clone();
        let service_arn = service.arn.clone();
        for arn in &task_arns {
            self.stop_service_task(arn, now, &format!("Service {name} was deleted"));
        }
        let Some(service) = self.services.get_mut(key) else {
            return;
        };
        service.desired_count = 0;
        // The drain the delete starts: the service is DRAINING, and the move to the
        // terminal INACTIVE is owed by the clock. An already-draining service keeps the
        // instant its own delete put it on, so repeating the delete does not push the
        // drain further out.
        service.status = SERVICE_DRAINING.to_string();
        service.drain_due_at = Some(service.drain_due_at.unwrap_or(now + TRANSITION_SPAN));
        // The delete is a write that touches the deployment, so its stability is
        // re-confirmed from here (see `ServiceRecord::deployment_stable_due_at`).
        service.deployment_stable_due_at = Some(now + TRANSITION_SPAN);
        // A deleted service has no rollout left to settle.
        service.deployment_steady_due_at = None;
        // And the rollout that was still under way ends here with it, rather than being
        // left open for a steady state the deleted service will never reach. The history
        // itself is KEPT, exactly as the service record is.
        self.finish_open_deployments(
            &service_arn,
            DateTime::from(now),
            SERVICE_DEPLOYMENT_STOPPED,
            &format!("Service {name} was deleted"),
        );
        // A task set belongs to the service it was created in, so the task sets of a
        // service being drained away are being drained away with it — their tasks were
        // stopped above with every other task of the service, which is exactly what
        // `TaskSet$status` DRAINING describes. They stop being the service's task sets
        // where the drain finishes ([`Self::settle_services`]).
        for task_set in self.task_sets.values_mut().filter(|task_set| {
            (&task_set.cluster_name, &task_set.service_name) == (&key.0, &key.1)
        }) {
            if task_set.status != TASK_SET_DRAINING {
                task_set.status = TASK_SET_DRAINING.to_string();
                task_set.updated_at = DateTime::from(now);
            }
        }
    }

    /// The task sets of ONE service, oldest first.
    ///
    /// Task set ids are minted from a fixed-width monotonic counter, so sorting the ids
    /// orders the task sets by the call that created them.
    pub fn service_task_sets(
        &self,
        cluster_name: &str,
        service_name: &str,
    ) -> Vec<&TaskSetRecord> {
        let mut found: Vec<&TaskSetRecord> = self
            .task_sets
            .values()
            .filter(|task_set| {
                task_set.cluster_name == cluster_name && task_set.service_name == service_name
            })
            .collect();
        found.sort_by(|left, right| left.id.cmp(&right.id));
        found
    }

    /// The task set a `taskSet` reference names in one service of one cluster, if this
    /// store holds it. The member's doc: "The ID or full Amazon Resource Name (ARN) of
    /// task sets", so a reference is either form.
    pub fn resolve_task_set(
        &self,
        cluster_name: &str,
        service_name: &str,
        reference: &str,
    ) -> Option<&TaskSetRecord> {
        self.task_sets.values().find(|task_set| {
            task_set.cluster_name == cluster_name
                && task_set.service_name == service_name
                && task_set.is_named_by(reference)
        })
    }

    /// Hold the task set `key` names at the scale `scale` asks for, and put the scheduler
    /// to work on it, answering with the record as the update left it.
    ///
    /// `UpdateTaskSet$scale` is "A floating-point percentage of the desired number of
    /// tasks to place and keep running in the task set", so changing it changes the
    /// `TaskSet$computedDesiredCount` the task set is held at — and the call that changes
    /// what the task set wants is what places or stops its tasks
    /// ([`Self::scale_task_set`]), never a later read.
    ///
    /// `Err` is [`Self::scale_task_set`]'s: the placement would push this emulator past
    /// [`MAX_TASKS`], so the task set is left at the scale it was already held at.
    pub fn rescale_task_set(
        &mut self,
        key: &(String, String, String),
        scale: &Scale,
        region: &str,
        now: SystemTime,
    ) -> Result<Option<TaskSetRecord>, TaskCapacityExceeded> {
        let Some(previous) = self.task_sets.get(key).cloned() else {
            return Ok(None);
        };
        if let Some(task_set) = self.task_sets.get_mut(key) {
            task_set.scale = stored_scale(Some(scale));
            task_set.updated_at = DateTime::from(now);
        }
        // The revision the task set places is the one it was created with, as stored: an
        // update of the scale changes how MANY tasks the task set holds, never which
        // task definition they run.
        let definition = self
            .task_definitions
            .get(&previous.task_definition)
            .cloned();
        if let Some(definition) = definition {
            if let Err(exceeded) = self.scale_task_set(key, &definition, region, now) {
                // Nothing was placed, so NOTHING was written: the task set is left exactly
                // as it stood, down to the instant its steady state was confirmed at.
                if let Some(task_set) = self.task_sets.get_mut(key) {
                    task_set.scale = previous.scale.clone();
                    task_set.updated_at = previous.updated_at;
                }
                return Err(exceeded);
            }
        }
        if let Some(task_set) = self.task_sets.get_mut(key) {
            // The write touched what the task set wants, so its steady state is
            // re-confirmed from here.
            task_set.stable_due_at = Some(now + TRANSITION_SPAN);
        }
        Ok(self.task_sets.get(key).cloned())
    }

    /// Delete the task set `key` names, answering with the record as the delete left it.
    ///
    /// `DeleteTaskSet` "Deletes a specified task set within a service": the tasks the task
    /// set placed are asked to stop and the task set is left in the status that says so —
    /// `TaskSet$status` DRAINING, "the tasks in the task set are being stopped, and their
    /// corresponding targets are being deregistered from their target group". That drain
    /// is the work the delete STARTS, so the task set is still one the service holds while
    /// it is under way, and the CLOCK is what finishes it: once the drain falls due the
    /// task set is gone ([`Self::advance_clock`]), so a read of it reports it MISSING and
    /// the service no longer carries it.
    ///
    /// Deleting a task set whose drain is already under way changes nothing: the delete is
    /// `@idempotent` and the task set keeps the instant its own delete put it on.
    pub fn delete_task_set(
        &mut self,
        key: &(String, String, String),
        now: SystemTime,
    ) -> Option<TaskSetRecord> {
        let task_set = self.task_sets.get(key).cloned()?;
        let mut task_arns: Vec<String> = self
            .tasks
            .values()
            .filter(|task| {
                task.cluster_name == task_set.cluster_name
                    && task.started_by.as_deref() == Some(task_set.id.as_str())
                    && task.is_active()
            })
            .map(|task| task.task_arn.clone())
            .collect();
        task_arns.sort();
        let reason = task_set_deleted_reason(&task_set.id);
        for arn in &task_arns {
            self.stop_service_task(arn, now, &reason);
        }
        let deleted = self.task_sets.get_mut(key)?;
        deleted.status = TASK_SET_DRAINING.to_string();
        // A task set being drained away keeps no tasks, so it wants none either: the
        // delete takes its scale to zero, the way a deleted service's desired count goes
        // to zero ([`Self::drain_service`]).
        deleted.scale = Scale {
            value: MIN_SCALE_PERCENT,
            unit: Some(SCALE_UNIT_PERCENT.to_string()),
        };
        deleted.updated_at = DateTime::from(now);
        deleted.removal_due_at = Some(deleted.removal_due_at.unwrap_or(now + TRANSITION_SPAN));
        Some(deleted.clone())
    }

    /// Make the task set `key` names the one serving production traffic, and hand the
    /// service the parameters it deploys with.
    ///
    /// `TaskSet$status`: "PRIMARY: The task set is serving production traffic. ACTIVE: The
    /// task set isn't serving production traffic" — exactly ONE task set of a service is
    /// the primary one, so whichever held that status loses it here. The
    /// `UpdateServicePrimaryTaskSet` doc adds "Any parameters that are updated on the
    /// primary task set in a service will transition to the service": the service deploys
    /// through its primary task set, so the task definition, the capacity the tasks run
    /// on, and the network, load-balancer and service-registry bindings the service
    /// reports are the primary task set's.
    ///
    /// Promoting the task set that is already primary changes nothing, so the instants the
    /// records carry are left where they are and a repeat of the call reads the same.
    ///
    /// A task set whose tasks "are being stopped, and their corresponding targets are
    /// being deregistered from their target group" (the `DRAINING` status) is one the
    /// delete of its service drained away with it, and such a service is not one to update
    /// at all — the caller's request is refused with `ServiceNotActiveException` before it
    /// reaches here.
    ///
    /// Answers the task set as it now STANDS in the store, so the operation reporting it
    /// projects the stored record rather than reading the map a second time, and `None`
    /// for a key this store does not hold.
    pub fn promote_primary_task_set(
        &mut self,
        key: &(String, String, String),
        now: SystemTime,
    ) -> Option<TaskSetRecord> {
        let promoted = self.task_sets.get(key).cloned()?;
        let at = DateTime::from(now);
        for task_set in self.task_sets.values_mut().filter(|task_set| {
            task_set.cluster_name == promoted.cluster_name
                && task_set.service_name == promoted.service_name
                && task_set.id != promoted.id
                && task_set.status == TASK_SET_PRIMARY
        }) {
            task_set.status = TASK_SET_ACTIVE.to_string();
            task_set.updated_at = at;
        }
        if let Some(task_set) = self.task_sets.get_mut(key) {
            if task_set.status != TASK_SET_PRIMARY {
                task_set.status = TASK_SET_PRIMARY.to_string();
                task_set.updated_at = at;
                // The promotion is a write that touches the task set, so the scheduler
                // re-confirms its steady state from here.
                task_set.stable_due_at = Some(now + TRANSITION_SPAN);
            }
        }
        let service_key = (
            promoted.cluster_name.clone(),
            promoted.service_name.clone(),
        );
        if let Some(service) = self.services.get_mut(&service_key) {
            service.task_definition_arn = Some(promoted.task_definition_arn.clone());
            service.task_definition = Some(promoted.task_definition.clone());
            // "A service will return either a capacityProviderStrategy or launchType
            // parameter, but not both": the task set carries exactly one of the two, and
            // the service takes the pair as it stands rather than mixing the two sources.
            service.launch_type = promoted.launch_type.clone();
            service.capacity_provider_strategy = promoted.capacity_provider_strategy.clone();
            service.platform_version = promoted.platform_version.clone();
            service.network_configuration = promoted.network_configuration.clone();
            service.load_balancers = promoted.load_balancers.clone();
            service.service_registries = promoted.service_registries.clone();
        }
        self.task_sets.get(key).cloned()
    }

    /// How many tasks `task_set` wants. `TaskSet$computedDesiredCount`: "This is
    /// calculated by multiplying the service's `desiredCount` by the task set's `scale`
    /// percentage. The result is always rounded up."
    pub fn computed_desired_count(&self, task_set: &TaskSetRecord) -> i32 {
        let Some(service) = self
            .services
            .get(&(task_set.cluster_name.clone(), task_set.service_name.clone()))
        else {
            return 0;
        };
        let desired = f64::from(service.desired_count.max(0));
        let computed = (desired * task_set.scale.value / MAX_SCALE_PERCENT).ceil();
        if computed <= 0.0 {
            return 0;
        }
        // The product of a count and a percentage cannot leave the `Integer` the member
        // targets, but the cast is still bounded rather than wrapped.
        computed.min(f64::from(i32::MAX)) as i32
    }

    /// What the task map counts for ONE task set: the tasks it placed and still wants,
    /// split by lifecycle status — the same split [`Self::service_tally`] makes, keyed on
    /// the task set that placed the task rather than on the service.
    pub fn task_set_tally(&self, task_set: &TaskSetRecord) -> ServiceTally {
        self.tally_wanted(|task| {
            task.cluster_name == task_set.cluster_name
                && task.started_by.as_deref() == Some(task_set.id.as_str())
        })
    }

    /// The `TaskSet` every task-set-returning operation reports, built from what is
    /// stored.
    ///
    /// `computedDesiredCount` is DERIVED from the service's desired count and this task
    /// set's scale, and `runningCount`/`pendingCount` are COUNTED off the task map, so
    /// the `stabilityStatus` that follows from them is read rather than asserted.
    pub fn task_set_view(&self, task_set: &TaskSetRecord, fields: TaskSetFields) -> TaskSet {
        let tally = self.task_set_tally(task_set);
        let computed_desired_count = self.computed_desired_count(task_set);
        let service = self
            .services
            .get(&(task_set.cluster_name.clone(), task_set.service_name.clone()));
        // "A platform family is specified only for tasks that use the Fargate launch
        // type", and so is a platform version — the same rule a service's projection
        // follows, read off the capacity THIS task set runs on.
        let fargate = places_on_fargate(
            task_set.launch_type.as_deref(),
            &task_set.capacity_provider_strategy,
        );
        let os_family = self
            .task_definitions
            .get(&task_set.task_definition)
            .and_then(|definition| definition.runtime_platform.as_ref())
            .and_then(|platform| platform.operating_system_family.clone())
            .unwrap_or_else(|| DEFAULT_OS_FAMILY.to_string());
        let requested_platform_version = fargate.then(|| {
            task_set
                .platform_version
                .clone()
                .unwrap_or_else(|| PLATFORM_VERSION_LATEST.to_string())
        });
        TaskSet {
            id: Some(task_set.id.clone()),
            task_set_arn: Some(task_set.arn.clone()),
            service_arn: Some(task_set.service_arn.clone()),
            cluster_arn: Some(task_set.cluster_arn.clone()),
            // "If an CodeDeploy deployment created the task set, the `startedBy`
            // parameter is `CODE_DEPLOY`. If an external deployment created the task set,
            // the `startedBy` field isn't used."
            started_by: service
                .filter(|service| service.controller() == DEPLOYMENT_CONTROLLER_CODE_DEPLOY)
                .map(|_| TASK_SET_STARTED_BY_CODE_DEPLOY.to_string()),
            external_id: task_set.external_id.clone(),
            status: Some(task_set.status.clone()),
            task_definition: Some(task_set.task_definition_arn.clone()),
            computed_desired_count,
            pending_count: tally.pending,
            running_count: tally.running,
            created_at: Some(task_set.created_at),
            updated_at: Some(task_set.updated_at),
            launch_type: task_set.launch_type.clone(),
            capacity_provider_strategy: Some(task_set.capacity_provider_strategy.clone()),
            platform_version: requested_platform_version
                .as_deref()
                .map(|requested| resolved_platform_version(requested, &os_family)),
            platform_family: fargate.then(|| os_family_display(&os_family)),
            network_configuration: task_set.network_configuration.clone(),
            load_balancers: Some(task_set.load_balancers.clone()),
            service_registries: Some(task_set.service_registries.clone()),
            scale: Some(task_set.scale.clone()),
            // "The task `runningCount` is equal to the `computedDesiredCount`" and "The
            // `pendingCount` is 0"; otherwise the task set is still stabilizing — and so
            // is one whose steady state the scheduler has not confirmed yet, however its
            // counts already read ([`TaskSetRecord::stable_due_at`]). A task set being
            // drained away meets neither reading of a steady state: its tasks "are being
            // stopped, and their corresponding targets are being deregistered from their
            // target group", which is work still under way.
            stability_status: Some(
                if task_set.stable_due_at.is_none()
                    && task_set.status != TASK_SET_DRAINING
                    && tally.running == computed_desired_count
                    && tally.pending == 0
                {
                    STABILITY_STATUS_STEADY_STATE
                } else {
                    STABILITY_STATUS_STABILIZING
                }
                .to_string(),
            ),
            // "The Unix timestamp for the time when the task set stability status was
            // retrieved": the stability status is computed from the tasks as they stand
            // when the task set was last written, which is the instant it carries.
            stability_status_at: Some(task_set.updated_at),
            // `DescribeTaskSets$include`: "If `TAGS` is specified, the tags are included
            // in the response. If this field is omitted, tags aren't included" — so the
            // tags a task set carries are reported only to a caller that asked for them.
            // The MEMBER itself is always serialized: the recorded task sets of the real
            // service carry it as an empty list where there are none to report.
            tags: Some(if fields.tags {
                task_set.tags.clone()
            } else {
                Vec::new()
            }),
            // A Fargate ephemeral-storage setting is encrypted with a customer key the
            // caller supplies, and nothing here supplies one.
            fargate_ephemeral_storage: None,
        }
    }

    /// Set a service's `desiredCount` to `capacity` and put the scheduler to work on it,
    /// answering whether the service was there to move at all.
    ///
    /// The bookkeeping a count change owes, in ONE place: `Service$desiredCount` is "The
    /// desired number of instantiations of the task definition to keep running on the
    /// service", so changing it gives the scheduler work to do — the service leaves its
    /// steady state and its deployment is STABILIZING again until the clock confirms it
    /// ([`TRANSITION_SPAN`]) — and then [`Self::scale_service`] brings the task set in line.
    /// A count that is already the one the service holds, and a service that is not active,
    /// move nothing.
    ///
    /// `Err` is [`Self::scale_service`]'s: the placement would push this emulator past
    /// [`MAX_TASKS`], so the task set is unchanged. The desired count is still the one that
    /// was asked for, and it is the CALLER that decides whether to keep it — `UpdateService`
    /// answers its requester with a refusal and restores the service it snapshotted.
    pub fn scale_service_to(
        &mut self,
        key: &(String, String),
        capacity: i32,
        region: &str,
        now: SystemTime,
    ) -> Result<bool, TaskCapacityExceeded> {
        let Some(service) = self.services.get(key) else {
            return Ok(false);
        };
        if !service.is_active() || service.desired_count == capacity {
            return Ok(false);
        }
        let definition = service
            .task_definition
            .clone()
            .and_then(|revision| self.task_definitions.get(&revision).cloned());
        let updated_at = DateTime::from(now);
        let Some(service) = self.services.get_mut(key) else {
            return Ok(false);
        };
        service.desired_count = capacity;
        service.deployment_updated_at = updated_at;
        service.deployment_steady_due_at = Some(now + TRANSITION_SPAN);
        service.deployment_stable_due_at = Some(now + TRANSITION_SPAN);
        if let Some(definition) = definition {
            self.scale_service(key, &definition, region, now)?;
        }
        Ok(true)
    }

    /// Place or stop tasks so the service holds exactly the `desiredCount` tasks it
    /// wants, from `definition`.
    ///
    /// This is the ECS service scheduler's job — the `CreateService` doc: "Runs and
    /// maintains your desired number of tasks from a specified task definition" — and it
    /// runs both on the call that CHANGES what the service wants and on the clock pass that
    /// keeps the count the caller asked for
    /// ([`Self::maintain_service_task_counts`]): maintaining it is continuous work, so a
    /// task that stopped is replaced without a further call. A task of a superseded
    /// deployment is no longer wanted, so a new deployment replaces the task set it
    /// inherited.
    ///
    /// A placement the cluster cannot satisfy places NOTHING and records WHY on the
    /// service's event stream instead (see [`Self::record_placement_failure`]): the
    /// desired count is what the caller asked for and does not move because the scheduler
    /// could not meet it.
    ///
    /// `Err` means the placement would push this emulator past [`MAX_TASKS`]; nothing is
    /// changed in that case.
    pub fn scale_service(
        &mut self,
        key: &(String, String),
        definition: &TaskDefinitionRecord,
        region: &str,
        now: SystemTime,
    ) -> Result<(), TaskCapacityExceeded> {
        let Some(service) = self.services.get(key).cloned() else {
            return Ok(());
        };
        if !service.places_tasks() {
            return Ok(());
        }
        let placement = service_placement(&service);

        // Task ids are minted from a fixed-width monotonic counter and the ARN ends with
        // the id, so sorting the ARNs orders the task set oldest first.
        let mut wanted: Vec<String> = self
            .tasks
            .values()
            .filter(|task| {
                task.cluster_name == service.cluster_name
                    && task.service_name.as_deref() == Some(service.name.as_str())
                    && task.desired_status == STATUS_RUNNING
                    && task.is_active()
            })
            .map(|task| task.task_arn.clone())
            .collect();
        wanted.sort();

        let (superseded, live): (Vec<String>, Vec<String>) = wanted.into_iter().partition(|arn| {
            self.tasks
                .get(arn)
                .and_then(|task| task.started_by.clone())
                .as_deref()
                != Some(service.deployment_id.as_str())
        });
        let desired = usize::try_from(service.desired_count).unwrap_or(0);
        let to_place = desired.saturating_sub(live.len());
        // Nothing is changed when the placement would not fit: the cap is checked before
        // the first task is touched.
        if self.tasks.len() + to_place > MAX_TASKS {
            return Err(TaskCapacityExceeded);
        }
        // A task of a superseded deployment is one the service no longer wants, so the
        // rollout replaces it — unless its caller has taken it out of the scheduler's reach
        // ([`Self::stop_unprotected_service_tasks`]).
        self.stop_unprotected_service_tasks(
            superseded.iter().rev().map(String::as_str),
            superseded.len(),
            now,
            &format!("Task stopped by deployment {}", service.deployment_id),
        );
        if live.len() > desired {
            // Scaling in stops the newest tasks of the deployment first: nothing here
            // runs on a container instance or in an Availability Zone, so there is no
            // placement balance to break the tie, and newest-first is deterministic.
            self.stop_unprotected_service_tasks(
                live.iter().rev().map(String::as_str),
                live.len() - desired,
                now,
                &format!(
                    "Scaling activity initiated by deployment {}",
                    service.deployment_id
                ),
            );
            return Ok(());
        }

        if to_place > 0 {
            if let Some(reason) = self.placement_capacity_reason(&service, &placement) {
                self.record_placement_failure(key, &reason, now);
                return Ok(());
            }
        }
        for _ in 0..to_place {
            self.place_service_task(&service, definition, &placement, region, now);
        }
        Ok(())
    }

    /// Place or stop tasks so `task_set` holds exactly the `computedDesiredCount` tasks
    /// it wants, from `definition`.
    ///
    /// The same scheduler work [`Self::scale_service`] does, for the task set of a
    /// service whose deployment controller is not the ECS one: the `Service$taskSets` doc
    /// describes those as "a set of Amazon ECS tasks in either an CodeDeploy or an
    /// `EXTERNAL` deployment", and how many tasks the set wants is the
    /// `TaskSet$computedDesiredCount` the service's desired count and this scale come to.
    /// It runs on the call that CHANGES what the task set wants, never on a read.
    pub fn scale_task_set(
        &mut self,
        key: &(String, String, String),
        definition: &TaskDefinitionRecord,
        region: &str,
        now: SystemTime,
    ) -> Result<(), TaskCapacityExceeded> {
        let Some(task_set) = self.task_sets.get(key).cloned() else {
            return Ok(());
        };
        let Some(service) = self
            .services
            .get(&(task_set.cluster_name.clone(), task_set.service_name.clone()))
            .cloned()
        else {
            return Ok(());
        };
        let placement = task_set_placement(&task_set);
        let desired = usize::try_from(self.computed_desired_count(&task_set)).unwrap_or(0);

        let mut live: Vec<String> = self
            .tasks
            .values()
            .filter(|task| {
                task.cluster_name == task_set.cluster_name
                    && task.started_by.as_deref() == Some(task_set.id.as_str())
                    && task.desired_status == STATUS_RUNNING
                    && task.is_active()
            })
            .map(|task| task.task_arn.clone())
            .collect();
        live.sort();

        let to_place = desired.saturating_sub(live.len());
        if self.tasks.len() + to_place > MAX_TASKS {
            return Err(TaskCapacityExceeded);
        }
        if live.len() > desired {
            // The same selection rule the service's own scale-in follows: a task its caller
            // has protected is passed over ([`Self::stop_unprotected_service_tasks`]).
            self.stop_unprotected_service_tasks(
                live.iter().rev().map(String::as_str),
                live.len() - desired,
                now,
                &format!("Scaling activity initiated by task set {}", task_set.id),
            );
            return Ok(());
        }
        if to_place > 0 {
            if let Some(reason) = self.placement_capacity_reason(&service, &placement) {
                let service_key = (task_set.cluster_name.clone(), task_set.service_name.clone());
                self.record_placement_failure(&service_key, &reason, now);
                return Ok(());
            }
        }
        for _ in 0..to_place {
            self.place_service_task(&service, definition, &placement, region, now);
        }
        Ok(())
    }

    /// Why the scheduler cannot place a task of `service` under `placement` right now, or
    /// `None` when it can.
    ///
    /// The capacity a placement runs on has to EXIST before Amazon ECS has anywhere to
    /// put the task: "The EC2 launch type runs your tasks on Amazon EC2 instances
    /// registered to your cluster" (and `EXTERNAL` on the registered on-premises
    /// capacity), and an Auto Scaling group capacity provider's capacity is the container
    /// instances its group registers — unless the provider scales that group itself, in
    /// which case the capacity is coming and the task is parked instead
    /// ([`awaits_provisioned_capacity`]). The sentence this answers is the one `RunTask`
    /// refuses the same condition with, so a client reads one explanation whichever call
    /// hit it.
    pub fn placement_capacity_reason(
        &self,
        service: &ServiceRecord,
        placement: &Placement,
    ) -> Option<String> {
        if self.registered_container_instances(&service.cluster_name) > 0 {
            return None;
        }
        if needs_registered_capacity(placement.launch_type.as_deref()) {
            return Some(NO_CONTAINER_INSTANCES_MESSAGE.to_string());
        }
        let provider = placement
            .capacity_provider_name
            .as_deref()
            .and_then(|name| self.capacity_providers.get(name))?;
        needs_registered_capacity_now(provider)
            .then(|| empty_capacity_provider_message(&provider.name))
    }

    /// Append the scheduler's own account of a placement it could not make to the
    /// service's event stream.
    ///
    /// `Service$events` is "The event stream for your service", and this is the entry
    /// Amazon ECS writes there when it wants to place a task and finds nowhere to put it
    /// — the documented place a client reads why `runningCount` is below `desiredCount`.
    /// A consecutive repeat of the same sentence is not appended: the event stream holds
    /// "the latest events" of the service, and re-stating the same unchanged condition
    /// would push every other event out of it.
    pub(super) fn record_placement_failure(
        &mut self,
        key: &(String, String),
        reason: &str,
        now: SystemTime,
    ) {
        let Some(service) = self.services.get(key) else {
            return;
        };
        let message = unable_to_place_message(&service.name, reason);
        if service
            .events
            .last()
            .is_some_and(|event| event.message == message)
        {
            return;
        }
        let event = self.mint_service_event(now, message);
        let Some(service) = self.services.get_mut(key) else {
            return;
        };
        service.record_events([event]);
    }

    /// Place ONE task of `service`, `PENDING` with its start owed by the clock
    /// [`TRANSITION_SPAN`] from now, exactly as `RunTask` leaves a task it placed — or
    /// parked in [`STATUS_PROVISIONING`] when the strategy's provider still has to acquire
    /// the capacity to place it on ([`awaits_provisioned_capacity`]), or when the interface
    /// it was allocated still has to be attached ([`placement_schedule`]).
    pub(super) fn place_service_task(
        &mut self,
        service: &ServiceRecord,
        definition: &TaskDefinitionRecord,
        placement: &Placement,
        region: &str,
        now: SystemTime,
    ) {
        let awaiting_capacity = awaits_provisioned_capacity(
            placement
                .capacity_provider_name
                .as_deref()
                .and_then(|name| self.capacity_providers.get(name)),
            self.registered_container_instances(&service.cluster_name),
        );
        let task_id = self.next_task_id();
        let task_arn = task_arn(region, &service.cluster_name, &task_id);
        // The interface an `awsvpc` task is allocated is allocated to EVERY task of it,
        // whoever places it, out of the network configuration the placement runs under —
        // for a service's task, the service's own.
        let attachments = self.mint_task_attachments(
            &definition.network_mode,
            placement.network_configuration.as_ref(),
            now,
        );
        let schedule = placement_schedule(awaiting_capacity, &attachments, now);
        let containers = self.mint_containers(
            region,
            &service.cluster_name,
            &task_id,
            &task_arn,
            definition,
            schedule.last_status,
        );
        // "Determines whether to propagate the tags from the task definition or the
        // service to the task. If no value is specified, the tags aren't propagated."
        let tags = match service.propagate_tags.as_str() {
            PROPAGATE_TAGS_SERVICE => service.tags.clone(),
            PROPAGATE_TAGS_TASK_DEFINITION => definition.tags.clone(),
            _ => Vec::new(),
        };
        let record = TaskRecord {
            task_arn: task_arn.clone(),
            task_id,
            cluster_name: service.cluster_name.clone(),
            cluster_arn: service.cluster_arn.clone(),
            // `Task$containerInstanceArn`: "The ARN of the container instance that hosts the
            // task" — the instance the scheduler chose for a launch type that runs on the
            // cluster's own registered capacity, and `None` for one that does not
            // ([`Self::select_task_host`]).
            container_instance_arn: self
                .select_task_host(&service.cluster_name, placement.launch_type.as_deref()),
            task_definition_arn: definition.arn.clone(),
            family: definition.family.clone(),
            last_status: schedule.last_status.to_string(),
            desired_status: STATUS_RUNNING.to_string(),
            containers,
            cpu: definition.cpu.clone(),
            memory: definition.memory.clone(),
            overrides: None,
            launch_type: placement.launch_type.clone(),
            capacity_provider_name: placement.capacity_provider_name.clone(),
            platform_version: placement.platform_version.clone(),
            // `Task$startedBy`: for a task the service scheduler placed, the id of the
            // deployment that placed it; for a task set's, the id of that task set.
            started_by: Some(placement.started_by.clone()),
            // `Task$group`: "The name of the task group associated with the task", which
            // for a service's task is `service:<service name>`.
            group: format!("service:{}", service.name),
            service_name: Some(service.name.clone()),
            tags,
            enable_execute_command: service.enable_execute_command,
            created_at: DateTime::from(now),
            started_at: None,
            stopping_at: None,
            stopped_at: None,
            execution_stopped_at: None,
            // The image pull is attempted by the call that placed the task, with this
            // store's guard dropped, and the instants it took are recorded there
            // ([`evaluate_image_pulls`]).
            pull_started_at: None,
            pull_stopped_at: None,
            stopped_reason: None,
            stop_code: None,
            version: 1,
            // A task that is not placed yet has no start owed: the clock owes it only
            // once the capacity it waits for exists — and a task whose interface has to
            // be attached first is placed at the instant it is in place.
            pending_due_at: schedule.pending_due_at,
            running_due_at: schedule.running_due_at,
            stopped_due_at: None,
            // Nobody has protected a task the scheduler has just placed.
            protection_expires_at: None,
            attachments,
            // Nothing is on the network before the networking it runs with is in place.
            connectivity: None,
            connectivity_at: None,
            // The networking the placement runs under, which for a service's task is the
            // service's own — the placement the image fetch is decided against.
            awsvpc_configuration: placement
                .network_configuration
                .as_ref()
                .and_then(|configuration| configuration.awsvpc_configuration.clone()),
            // Whether the images this task's containers run can be fetched from that
            // placement is the launching call's to decide, with this store's guard
            // dropped ([`crate::state::evaluate_image_pulls`]).
            launch_failure: None,
            // Nothing has put the roles this placement resolved to the account's IAM
            // records yet: that is the launching pass's to do, with this store's guard
            // dropped ([`crate::state::evaluate_launch_role_assumptions`]).
            launch_roles_assumed: false,
            // The task execution role the revision this placement runs declares, which is
            // the principal its image pull is made as. A service's placement carries no
            // overrides, so the revision's own value is the resolved one.
            execution_role_arn: definition.execution_role_arn.clone(),
            // Nothing is owed on the work of a task that has not started, and a service's
            // task is kept in place by its scheduler either way
            // ([`CONTAINER_WORK_SPAN`]).
            work_done_due_at: None,
        };
        self.tasks.insert(task_arn, record);
    }

    /// Ask at most `wanted` of the service tasks `candidates` offers — in the order they are
    /// offered — to stop, PASSING OVER every one whose caller has protected it.
    ///
    /// The scheduler's own selection rule, in the one place both of its stops are made from
    /// ([`Self::scale_service`]): whenever a service holds tasks it no longer wants, Amazon
    /// ECS picks which of them to terminate — and scale-in protection is what takes a task
    /// out of that choice. `UpdateTaskProtection`: "Protects an Amazon ECS task from being
    /// terminated by scale-in events from Service Auto Scaling or deployments", and
    /// `ProtectedTask$protectionEnabled`: "If scale-in protection is turned on for a task,
    /// the value is true." So a protected task is NOT selected; an unprotected one is
    /// stopped in its place, and when the candidates run out with fewer stops made than were
    /// wanted the reduction simply waits — the protection expires on its own
    /// ([`Self::advance_clock`]), and the next scheduler pass makes the stop it could not
    /// make now.
    pub(super) fn stop_unprotected_service_tasks<'a>(
        &mut self,
        candidates: impl IntoIterator<Item = &'a str>,
        wanted: usize,
        now: SystemTime,
        reason: &str,
    ) {
        let mut left = wanted;
        for task_arn in candidates {
            if left == 0 {
                return;
            }
            if self
                .tasks
                .get(task_arn)
                .is_none_or(TaskRecord::is_protected)
            {
                continue;
            }
            self.stop_service_task(task_arn, now, reason);
            left -= 1;
        }
    }

    /// Ask one of a service's tasks to stop, with `STOPPED` owed by the clock
    /// [`TRANSITION_SPAN`] from now — the same two-step stop `StopTask` records.
    pub(super) fn stop_service_task(&mut self, task_arn: &str, now: SystemTime, reason: &str) {
        let Some(task) = self.tasks.get_mut(task_arn) else {
            return;
        };
        if task.desired_status == STATUS_STOPPED {
            return;
        }
        task.desired_status = STATUS_STOPPED.to_string();
        task.last_status = STATUS_DEACTIVATING.to_string();
        task.stopping_at = Some(DateTime::from(now));
        task.stopped_reason = Some(reason.to_string());
        task.stop_code = Some(STOP_CODE_SERVICE_SCHEDULER.to_string());
        task.stopped_due_at = Some(now + TRANSITION_SPAN);
        // A task asked to stop never reaches RUNNING: the start it was still waiting for,
        // and the placement it was parked for, are cancelled rather than merely overtaken.
        task.running_due_at = None;
        task.pending_due_at = None;
        task.version += 1;
        for container in &mut task.containers {
            container.advance_to(STATUS_DEACTIVATING);
        }
    }

    /// Count every number a described cluster reports in ONE pass over the task map and
    /// one over the service map, so describing a cluster costs one scan each rather than
    /// one scan per reported number.
    pub(super) fn tally(&self, cluster_name: &str) -> TaskTally {
        let mut tally = TaskTally::default();
        for task in self
            .tasks
            .values()
            .filter(|task| task.cluster_name == cluster_name)
        {
            let launch_type = statistics_launch_type_index(task.launch_type.as_deref());
            if task.last_status == STATUS_RUNNING {
                tally.running += 1;
                if let Some(index) = launch_type {
                    tally.by_launch_type[index].running_tasks += 1;
                }
            } else if task.is_pending() {
                tally.pending += 1;
                if let Some(index) = launch_type {
                    tally.by_launch_type[index].pending_tasks += 1;
                }
            }
        }
        for service in self
            .services
            .values()
            .filter(|service| service.cluster_name == cluster_name)
        {
            let launch_type = statistics_launch_type_index(service.launch_type.as_deref());
            if service.is_active() {
                tally.active_services += 1;
                if let Some(index) = launch_type {
                    tally.by_launch_type[index].active_services += 1;
                }
            } else if service.status == SERVICE_DRAINING {
                if let Some(index) = launch_type {
                    tally.by_launch_type[index].draining_services += 1;
                }
            }
        }
        tally
    }

    /// Whether `cluster_name` still holds a task the caller has NOT released: one that is
    /// neither stopped nor stopping, i.e. whose `desiredStatus` is still `RUNNING`.
    ///
    /// `DeleteCluster`'s `ClusterContainsTasksException`: "You can't delete a cluster that
    /// has active tasks." A task that has been asked to stop is on its way out and is
    /// already excluded from everything else that counts a cluster's tasks
    /// (`Service$runningCount`, `ListTasks`' default filter), so it does not hold the
    /// cluster open either.
    pub fn cluster_has_active_tasks(&self, cluster_name: &str) -> bool {
        self.tasks.values().any(|task| {
            task.cluster_name == cluster_name
                && task.desired_status == STATUS_RUNNING
                && task.is_active()
        })
    }

    /// The capacity providers of `cluster_name` a WORKLOAD of it still depends on, collected
    /// in one pass over the task map and one over the service and task-set maps.
    ///
    /// `PutClusterCapacityProviders`: "You can only disassociate an existing capacity
    /// provider from a cluster if it's not being used by any existing tasks." A provider is
    /// being used by a task two ways, and both keep it:
    ///
    /// * a task of the cluster is RUNNING on the capacity it stands up, which is a provider
    ///   the ACCOUNT created over an Auto Scaling group or Amazon ECS Managed Instances. The
    ///   two providers Amazon ECS publishes for every account ([`MANAGED_CAPACITY_PROVIDERS`])
    ///   stand nothing up for the cluster — "The Fargate capacity providers are available to
    ///   all accounts and only need to be associated with a cluster to be used" — so their
    ///   association is what a future placement is resolved against and not capacity a
    ///   running task sits on, and dropping one is served while its tasks run on;
    /// * a live SERVICE of the cluster, or a live task set of one, takes its tasks FROM that
    ///   provider (`Service$capacityProviderStrategy`, `TaskSet$capacityProviderStrategy`):
    ///   the scheduler keeps placing the service's tasks on it for as long as the service
    ///   exists, so dropping it would strand a workload Amazon ECS is still maintaining —
    ///   the next placement would have no capacity to resolve against
    ///   ([`Self::unassociated_capacity_provider_refusal`]). That holds however many of the
    ///   service's tasks happen to be running at this instant, which is why it is read off
    ///   the service's own strategy rather than off its task count.
    ///
    /// Each name is read as the provider it REFERS to
    /// ([`capacity_provider_name_from_reference`]), so naming one provider by ARN on one side
    /// and by short name on the other is still one provider.
    pub fn capacity_providers_in_use(&self, cluster_name: &str) -> HashSet<String> {
        let placed_on = self
            .tasks
            .values()
            .filter(|task| task.cluster_name == cluster_name && task.is_active())
            .filter_map(|task| task.capacity_provider_name.clone())
            .filter(|provider| !MANAGED_CAPACITY_PROVIDERS.contains(&provider.as_str()));
        let wanted_by_services = self
            .services
            .values()
            .filter(|service| service.cluster_name == cluster_name && service.is_active())
            .flat_map(|service| service.capacity_provider_strategy.iter())
            .map(|item| item.capacity_provider.clone());
        let wanted_by_task_sets = self
            .task_sets
            .values()
            .filter(|task_set| task_set.cluster_name == cluster_name)
            .flat_map(|task_set| task_set.capacity_provider_strategy.iter())
            .map(|item| item.capacity_provider.clone());
        placed_on
            .chain(wanted_by_services)
            .chain(wanted_by_task_sets)
            .map(|reference| capacity_provider_name_from_reference(&reference))
            .collect()
    }

    /// The value stored for `name` for ONE principal, or `None` when that principal has
    /// no setting of that name.
    ///
    /// The ROOT user's own settings ARE the account-wide defaults — `PutAccountSetting`
    /// `principalArn`: "If you specify the root user, it modifies the account setting
    /// for all users ... unless an individual account setting was specified" — so a
    /// lookup for the root principal reads the defaults map.
    pub fn explicit_account_setting(&self, principal_arn: &str, name: &str) -> Option<String> {
        if principal_arn == root_principal_arn() {
            return self.account_setting_defaults.get(name).cloned();
        }
        self.principal_account_settings
            .get(&(principal_arn.to_string(), name.to_string()))
            .cloned()
    }

    /// The ACCOUNT-WIDE value of `name`: what a write of the default (or of the root
    /// user's setting) stored, falling back to the [`ACCOUNT_SETTINGS`] default for a
    /// name the account has never written.
    pub fn default_account_setting(&self, name: &str) -> String {
        self.account_setting_defaults
            .get(name)
            .cloned()
            .unwrap_or_else(|| {
                account_setting_spec(name)
                    // Unreachable for a `name` off the closed `SettingName` enum; a name
                    // outside the catalog is reported off rather than invented.
                    .map_or(SETTING_OFF, |setting| setting.default_value)
                    .to_string()
            })
    }

    /// The EFFECTIVE account setting of `name` for `principal_arn`, and the principal it
    /// comes from: the principal's own value when it has one, otherwise the account-wide
    /// default, which belongs to the root user.
    ///
    /// `ListAccountSettings$effectiveSettings`: "If `true`, the account settings for the
    /// root user or the default setting for the `principalArn` are returned."
    pub fn effective_account_setting(&self, principal_arn: &str, name: &str) -> ResolvedSetting {
        match self.explicit_account_setting(principal_arn, name) {
            Some(value) => ResolvedSetting {
                value,
                principal_arn: principal_arn.to_string(),
            },
            None => ResolvedSetting {
                value: self.default_account_setting(name),
                principal_arn: root_principal_arn(),
            },
        }
    }

    /// Store the account setting `name` for ONE principal, as `PutAccountSetting` does,
    /// and answer the setting it reports.
    ///
    /// A write against the ROOT user is a write of the account-wide default, because the
    /// root user's settings are those defaults (see [`Self::explicit_account_setting`]).
    ///
    /// REFUSED with [`AccountSettingCapacityExceeded`], storing nothing, when the
    /// principal is one this emulator holds no setting for yet and
    /// [`MAX_ACCOUNT_SETTING_PRINCIPALS`] are already stored: the principal comes from
    /// the request, so the number of them a caller can leave behind is bounded here the
    /// way every other caller-driven map in this store is.
    pub fn put_account_setting(
        &mut self,
        principal_arn: &str,
        name: &str,
        value: &str,
    ) -> Result<Setting, AccountSettingCapacityExceeded> {
        if principal_arn == root_principal_arn() {
            return Ok(self.put_account_setting_default(name, value));
        }
        let principals: HashSet<&str> = self
            .principal_account_settings
            .keys()
            .map(|(principal, _)| principal.as_str())
            .collect();
        if !principals.contains(principal_arn)
            && principals.len() >= MAX_ACCOUNT_SETTING_PRINCIPALS
        {
            return Err(AccountSettingCapacityExceeded);
        }
        self.principal_account_settings.insert(
            (principal_arn.to_string(), name.to_string()),
            value.to_string(),
        );
        Ok(setting_view(name, value, principal_arn))
    }

    /// Store the ACCOUNT-WIDE default of `name`, as `PutAccountSettingDefault` does, and
    /// answer the setting it reports: one that belongs to the root user, since the
    /// default applies "for all users on an account for whom no individual account
    /// setting has been specified".
    pub fn put_account_setting_default(&mut self, name: &str, value: &str) -> Setting {
        self.account_setting_defaults
            .insert(name.to_string(), value.to_string());
        setting_view(name, value, &root_principal_arn())
    }

    /// Clear the account setting `name` for ONE principal, as `DeleteAccountSetting`
    /// does ("Disables an account setting for a specified user, role, or the root user
    /// for an account"), and answer the value that was stored, or `None` when that
    /// principal had none of that name.
    ///
    /// Clearing a principal's entry is what makes the account-wide default apply to it
    /// again ([`Self::effective_account_setting`]); a clear against the ROOT user clears
    /// the account-wide default itself, for the same reason a write against it sets one
    /// (see [`Self::explicit_account_setting`]), so the name falls back to its
    /// [`ACCOUNT_SETTINGS`] default.
    pub fn delete_account_setting(&mut self, principal_arn: &str, name: &str) -> Option<String> {
        if principal_arn == root_principal_arn() {
            return self.account_setting_defaults.remove(name);
        }
        self.principal_account_settings
            .remove(&(principal_arn.to_string(), name.to_string()))
    }

    /// The cluster's EFFECTIVE settings: one entry for every setting name the model
    /// defines (`ClusterSettingName`), carrying the value stored for this cluster when
    /// one was set and the account setting of that name otherwise.
    ///
    /// `ClusterSetting$value`: "If a cluster value is specified, it will override the
    /// `containerInsights` value set with PutAccountSetting or PutAccountSettingDefault"
    /// — so what a cluster that set nothing reports is exactly the account setting in
    /// effect for the caller this emulator authenticates every request as
    /// ([`caller_identity_arn`]), which falls back to the account-wide default and then
    /// to the setting's own default. `Cluster$settings` "indicates whether CloudWatch
    /// Container Insights is on or off for a cluster", so a cluster that set nothing
    /// still has an answer to that question and reports it.
    pub fn cluster_effective_settings(&self, cluster: &ClusterRecord) -> Vec<ClusterSetting> {
        let principal_arn = caller_identity_arn();
        CLUSTER_SETTING_NAME_VALUES
            .iter()
            .map(|name| ClusterSetting {
                name: Some((*name).to_string()),
                value: Some(cluster.setting_value(name).unwrap_or_else(|| {
                    self.effective_account_setting(&principal_arn, name).value
                })),
            })
            .collect()
    }

    /// The `Cluster` a cluster-returning operation reports.
    ///
    /// The counters are COMPUTED from the task, container-instance and service maps, so
    /// they always agree with the records that exist: `runningTasksCount` is "the number
    /// of tasks in the cluster that are in the RUNNING state", `pendingTasksCount` the
    /// same for `PENDING`, and `activeServicesCount` the services the cluster holds in
    /// an `ACTIVE` status.
    ///
    /// `registeredContainerInstancesCount` is counted off the container-instance map the
    /// same way: "the number of container instances registered into the cluster. This
    /// includes container instances in both ACTIVE and DRAINING status."
    ///
    /// `fields` follows `DescribeClusters$include`: "Determines whether to include
    /// additional information about the clusters in the response. If this field is
    /// omitted, this information isn't included."
    pub fn cluster_view(&self, cluster: &ClusterRecord, fields: ClusterFields) -> Cluster {
        let tally = self.tally(&cluster.name);
        Cluster {
            cluster_arn: Some(cluster.arn.clone()),
            cluster_name: Some(cluster.name.clone()),
            status: Some(cluster.status.clone()),
            registered_container_instances_count: i32::try_from(
                self.registered_container_instances(&cluster.name),
            )
            .unwrap_or(i32::MAX),
            running_tasks_count: i32::try_from(tally.running).unwrap_or(i32::MAX),
            pending_tasks_count: i32::try_from(tally.pending).unwrap_or(i32::MAX),
            active_services_count: i32::try_from(tally.active_services).unwrap_or(i32::MAX),
            capacity_providers: Some(cluster.capacity_providers.clone()),
            default_capacity_provider_strategy: Some(
                cluster.default_capacity_provider_strategy.clone(),
            ),
            configuration: if fields.configurations {
                cluster.configuration.clone()
            } else {
                None
            },
            // `settings`, `tags` and `statistics` are LISTS a described cluster always
            // carries: `include` decides whether they are materialised, not whether the
            // member is there, so a cluster read without `include` reports them empty
            // rather than absent and a client cannot mistake "not asked for" for "none
            // stored".
            settings: Some(if fields.settings {
                self.cluster_effective_settings(cluster)
            } else {
                Vec::new()
            }),
            tags: Some(if fields.tags {
                cluster.tags.clone()
            } else {
                Vec::new()
            }),
            statistics: Some(if fields.statistics {
                cluster_statistics(&tally)
            } else {
                Vec::new()
            }),
            // `Cluster$attachments` and `Cluster$attachmentsStatus` are what
            // `DescribeClusters$include` `ATTACHMENTS` asks for, so they are reported
            // only when it does. The list itself stays empty: attachments are the
            // resources a capacity provider provisions, and this emulator provisions
            // none (see [`ClusterRecord::attachments`]).
            attachments: if fields.attachments {
                Some(cluster.attachments())
            } else {
                None
            },
            attachments_status: if fields.attachments {
                cluster.attachments_status.clone()
            } else {
                None
            },
            service_connect_defaults: cluster.service_connect_defaults.clone(),
        }
    }

    /// The `Cluster` an operation that CREATES, UPDATES or DELETES a cluster reports.
    ///
    /// Such an operation carries no `include` member, and the halves of a `Cluster`
    /// answer that differently.
    ///
    /// What the cluster is CONFIGURED to be is always materialised: `settings` (the
    /// effective value of every setting name the model defines, see
    /// [`Self::cluster_effective_settings`] — `Cluster$settings` "indicates whether
    /// CloudWatch Container Insights is on or off for a cluster", which is true of a
    /// cluster whoever reads it) and `configuration` ("the execute command and managed
    /// storage configuration for the cluster"), so a client can read back the cluster it
    /// just changed.
    ///
    /// `statistics` — "additional information about your clusters that are separated by
    /// launch type" — is COMPUTED only for a read that asks for it
    /// (`DescribeClusters$include` `STATISTICS`), so here it is an EMPTY list: emitting
    /// launch-type metrics nobody asked for hands metric-name-keyed client code values
    /// the service never reported.
    ///
    /// `tags` is materialised only where the ANSWERING OPERATION'S INPUT SHAPE mentions
    /// tags — the `tags` member a create is handed (`report_tags`), or the `TAGS` entry of
    /// a read's `include`. An operation whose input mentions tags nowhere, a delete,
    /// reports an empty list rather than the resource's tag set, so "no tags on this
    /// cluster" and "tags not requested" stay the same answer they are on AWS.
    pub fn cluster_mutation_view(&self, cluster: &ClusterRecord, report_tags: bool) -> Cluster {
        let mut view = self.cluster_view(
            cluster,
            ClusterFields {
                attachments: true,
                configurations: true,
                settings: true,
                statistics: false,
                tags: report_tags,
            },
        );
        view.statistics = Some(Vec::new());
        view.tags = Some(view.tags.unwrap_or_default());
        view
    }

    /// How many clusters the account holds under `cluster_name` — the top-level
    /// `clusterCount` a cluster-mutating response carries, read AFTER the call has been
    /// applied. See [`ClusterCountEnvelope`] for why it is not a modelled member.
    ///
    /// A cluster NAME identifies one cluster ("you create a cluster that's named
    /// default" / "the short name ... of the cluster"), and a DELETED cluster is not one
    /// the account holds: the record stays as a tombstone so a reader can still describe
    /// it ("The cluster has been deleted"), but the account no longer holds that cluster,
    /// so it is not counted. A create therefore answers `1` and a delete `0`, whatever
    /// else the account holds beside the cluster the call named — which is what every
    /// recorded `clusterCount` carries, including the ones recorded while a second,
    /// untouched cluster was live.
    pub fn cluster_count_for(&self, cluster_name: &str) -> i32 {
        i32::from(
            self.clusters
                .get(cluster_name)
                .is_some_and(|cluster| cluster.status != CLUSTER_INACTIVE),
        )
    }

    /// The tag set stored on the resource `resource_arn` names, or `None` when this store
    /// holds no resource of that kind with that ARN.
    ///
    /// The resource's OWN `tags` — the list its create was given and its describe reports
    /// — is the one tag set this service holds, so whoever writes it (this service's own
    /// create, or a cross-service tagging call reaching into these records) and whoever
    /// reads it see the same list, with no second copy to drift from it.
    ///
    /// Each kind is keyed by something other than its ARN, so the scan is over the ONE
    /// map that kind lives in rather than over every map.
    pub fn resource_tags(&self, kind: TaggedResourceKind, resource_arn: &str) -> Option<&[Tag]> {
        match kind {
            TaggedResourceKind::CapacityProvider => self
                .capacity_providers
                .values()
                .find(|provider| provider.arn == resource_arn)
                .map(|provider| provider.tags.as_slice()),
            TaggedResourceKind::Cluster => self
                .clusters
                .values()
                .find(|cluster| cluster.arn == resource_arn)
                .map(|cluster| cluster.tags.as_slice()),
            TaggedResourceKind::ContainerInstance => self
                .container_instances
                .values()
                .find(|instance| instance.arn == resource_arn)
                .map(|instance| instance.tags.as_slice()),
            TaggedResourceKind::Service => self
                .services
                .values()
                .find(|service| service.arn == resource_arn)
                .map(|service| service.tags.as_slice()),
            TaggedResourceKind::Task => self
                .tasks
                .values()
                .find(|task| task.task_arn == resource_arn)
                .map(|task| task.tags.as_slice()),
            TaggedResourceKind::TaskDefinition => self
                .task_definitions
                .values()
                .find(|definition| definition.arn == resource_arn)
                .map(|definition| definition.tags.as_slice()),
        }
    }

    /// The tag set stored on the resource `resource_arn` names, for a caller that is
    /// about to CHANGE it — the same resolution as [`Self::resource_tags`], so a write
    /// and a read of one resource's tags reach the same list.
    pub fn resource_tags_mut(
        &mut self,
        kind: TaggedResourceKind,
        resource_arn: &str,
    ) -> Option<&mut Vec<Tag>> {
        match kind {
            TaggedResourceKind::CapacityProvider => self
                .capacity_providers
                .values_mut()
                .find(|provider| provider.arn == resource_arn)
                .map(|provider| &mut provider.tags),
            TaggedResourceKind::Cluster => self
                .clusters
                .values_mut()
                .find(|cluster| cluster.arn == resource_arn)
                .map(|cluster| &mut cluster.tags),
            TaggedResourceKind::ContainerInstance => self
                .container_instances
                .values_mut()
                .find(|instance| instance.arn == resource_arn)
                .map(|instance| &mut instance.tags),
            TaggedResourceKind::Service => self
                .services
                .values_mut()
                .find(|service| service.arn == resource_arn)
                .map(|service| &mut service.tags),
            TaggedResourceKind::Task => self
                .tasks
                .values_mut()
                .find(|task| task.task_arn == resource_arn)
                .map(|task| &mut task.tags),
            TaggedResourceKind::TaskDefinition => self
                .task_definitions
                .values_mut()
                .find(|definition| definition.arn == resource_arn)
                .map(|definition| &mut definition.tags),
        }
    }
}
