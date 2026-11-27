// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this service publishes as a TASK moves through its statuses.
//!
//! # The rule this module implements
//!
//! Amazon ECS is an event SOURCE, not only a control plane a client polls: every status a
//! task MOVES INTO is published onto the account's default event bus
//! ([`DEFAULT_EVENT_BUS`]) under the `source` this service owns ([`EVENT_SOURCE`]) and the
//! `detail-type` of a task's state change ([`TASK_STATE_CHANGE_DETAIL_TYPE`]), carrying the
//! task Amazon ECS holds at that instant in the event's `detail` — the same projection
//! `DescribeTasks` answers with, so the event and the API agree about the task — and the
//! task's own ARN in the envelope's `resources`. A rule on that bus whose `EventPattern`
//! matches then delivers it to the rule's targets, so a client that reacts to a task
//! through EventBridge observes the run without calling Amazon ECS again.
//!
//! WHAT counts as a move: the pair a task reports itself by, its `lastStatus` and its
//! `desiredStatus`. A task is published the first time it is seen — a placement IS the
//! first status it reports — and again every time either half of that pair changes,
//! however it changed: the clock materialising a transition, the caller asking for a stop,
//! the scheduler stopping one of a service's tasks, or the agent acknowledging a status
//! through `SubmitTaskStateChange`. A change nothing has moved publishes nothing, so two
//! calls at the same instant publish the same single event.
//!
//! # How a transition becomes an event
//!
//! Nothing here decides WHEN a task moves: the stored due instants do, through
//! [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]. This module
//! only COMPARES what the store now holds against what was last published
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::record_task_state_changes`]),
//! so a transition is published exactly once however it was materialised, and no writer
//! has to remember to announce itself. The queue it feeds is bounded
//! ([`MAX_PENDING_EVENTS`], oldest dropped first) because every transition appends to it.
//!
//! The same comparison is where a SERVICE learns that a launch it asked for was lost: a task
//! Amazon ECS could not launch the containers of at all is an action of the scheduler that
//! placed it, published as an `ECS Service Action`
//! ([`super::service_action_events::SERVICE_TASK_CONFIGURATION_FAILURE`]) beside the task's
//! own move, from the one place that already sees every move exactly once — carrying the
//! `stoppedReason` that stop earned as the action's `reason`, so the event says what the
//! launch failed on and not merely that one did.
//!
//! A queued change is published with no lock held ([`publish_pending`]): EventBridge is
//! reached the way every emulated service reaches another, through the framework's
//! in-process callback, so this crate names no other service's crate. Publication is best
//! effort — an emulator with no EventBridge mounted has nowhere to publish — so a failure
//! is logged and the operation's own answer stands.
//!
//! WHICH call publishes: every door that WRITES a task's status records what its own write
//! produced ([`publish_task_state_changes`]) once it has released the store's lock, and the
//! clock pump ([`drive_task_clock`]) is what puts those events on the bus and then keeps
//! going until the store owes nothing — because a client watching a task through
//! EventBridge makes no further Amazon ECS call, so a transition published only by the next
//! reader would never be published at all. The pump changes WHEN nothing: it materialises
//! only what the stored due instant has already made due, exactly as a reading door does.
//!
//! The CONTAINER DATA PLANE is a writer too, and the one no due instant can stand in for:
//! when a container's program ends is the program's own business, so the run that ends is
//! what records the move it produced and starts the pump again
//! ([`publish_data_plane_task_state_changes`]) — a standalone task whose essential container
//! has exited is stopped at that instant, and the `STOPPED` event that stop earns is the one
//! a client watching the task is waiting for.

use std::time::UNIX_EPOCH;

use super::*;

/// The `source` of every event Amazon ECS publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its ARNs carry ([`ARN_NAMESPACE`]).
pub const EVENT_SOURCE: &str = "aws.ecs";

/// The `detail-type` of the event published when a TASK changes state.
pub const TASK_STATE_CHANGE_DETAIL_TYPE: &str = "ECS Task State Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many task state changes the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every
/// transition appends to it, so without a bound a client placing tasks without end could
/// grow it without end. The oldest change is dropped first, so the transitions the live
/// tasks just made are the ones that survive. It is far above what a realistic burst
/// produces (a task contributes a handful of changes).
pub const MAX_PENDING_EVENTS: usize = 4_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a drain holding more changes than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many changes ONE drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one call draining all of it
/// would fan out to a rule's targets that many times: this caps one drain, and
/// [`publish_pending`] repeats a drain until the queue is empty, so nothing waits for a
/// later call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// The shortest one wake-up of the clock pump waits.
///
/// A due instant already in the past is materialised on the NEXT wake-up, not on this
/// one's instant, so the pump always hands the runtime back and a wake-up that materialises
/// nothing cannot turn the loop into a spin. It is far below [`TRANSITION_SPAN`], so it
/// never delays a transition noticeably.
const MIN_CLOCK_WAIT: Duration = Duration::from_millis(5);

/// The longest one wake-up of the clock pump waits before it looks at the store again.
///
/// Every transition a task owes falls due within a few spans of the one before it, so a
/// wait is never near this value; it is a backstop, so a due instant far in the future
/// cannot park the pump indefinitely.
const MAX_CLOCK_WAIT: Duration = Duration::from_secs(5);

/// How many wake-ups one pump makes before it stands down.
///
/// A backstop, not a budget: the transitions the store owes are already finite (a task
/// makes a handful, and the task map is bounded), and every wake-up either materialises a
/// transition or clears the due instant that woke it. The cap guarantees termination even
/// if a later change left a due instant that materialising does not clear, and the next
/// write starts a fresh pump.
const MAX_CLOCK_WAKE_UPS: usize = 10_000;

/// The pair a task reports its own position in the lifecycle by: `lastStatus` and
/// `desiredStatus`. A change to either is a change a state-change event reports.
pub type TaskStatusPair = (String, String);

/// One task state change waiting to be published, rendered against the task record as it
/// stood when the change was observed.
#[derive(Debug, Clone)]
pub struct TaskStateChange {
    /// The task that moved, named by the ARN the envelope's `resources` carries.
    pub task_arn: String,
    /// WHEN it moved — the instant the record itself stamps on the status it moved into,
    /// not the instant some later call materialised or published it.
    pub at: SystemTime,
    /// The rendered `detail`: the task, as `DescribeTasks` reports it.
    pub detail: ::serde_json::Value,
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Queue a state-change event for every task whose reported status pair differs from
    /// the one last published for it, as of `now`.
    ///
    /// This is the ONE place a transition becomes an event, so no writer can forget to
    /// announce one and none can be published twice: the comparison is against what was
    /// published, not against who wrote it. A task this store no longer holds cannot move
    /// again, so its entry goes with it.
    ///
    /// The changes are queued oldest transition first, so events about several tasks leave
    /// in the order the tasks moved rather than in the order the map happened to be walked.
    ///
    /// The same comparison is what tells the SERVICE SCHEDULER which of its tasks have just
    /// STARTED, so the entry naming them on the service's own event stream is written from
    /// here too ([`super::service_scheduler_task_reports`]): a task that moved into `RUNNING`
    /// in this comparison is one the scheduler started at that instant, and one that moved
    /// into it once is never announced twice.
    pub fn record_task_state_changes(&mut self, now: SystemTime) {
        let tasks = &self.tasks;
        self.published_task_status
            .retain(|task_arn, _| tasks.contains_key(task_arn));
        let mut observed: Vec<(String, TaskStatusPair, TaskStateChange)> = Vec::new();
        // The launches a SERVICE lost in this comparison, as the action the scheduler took by
        // losing them names them: `(serviceArn, clusterArn, why the launch was lost, the
        // instant it was lost at)`.
        let mut lost_launches: Vec<(String, String, Option<String>, SystemTime)> = Vec::new();
        // And the tasks a service's scheduler STARTED in it, as the entries that account for
        // them ([`super::service_scheduler_task_reports::ServiceTaskReport`]).
        let mut started_tasks: Vec<ServiceTaskReport> = Vec::new();
        for task in self.tasks.values() {
            let reported = (task.last_status.clone(), task.desired_status.clone());
            let published = self.published_task_status.get(&task.task_arn);
            if published == Some(&reported) {
                continue;
            }
            let at = status_instant(task).unwrap_or(now);
            // A task Amazon ECS could not launch the containers of AT ALL is a launch the
            // scheduler that asked for it LOST ([`TaskRecord::failed_to_launch`]), and losing
            // one is an action of that scheduler's: it is published beside the task's own move
            // as an `ECS Service Action`
            // ([`super::service_action_events::SERVICE_TASK_CONFIGURATION_FAILURE`]), so a
            // client watching the service learns that what it asked for cannot be launched
            // without reading every task the scheduler keeps replacing, and WHY: the action
            // carries the stop's own `stoppedReason` as its `reason`. Only on the move that
            // TURNED the task away from running — the published pair still wanted it running —
            // so the steps it then takes on its way out publish the action once rather than
            // once per status.
            let turned_away = published.is_none_or(|(_, desired)| desired == STATUS_RUNNING);
            if task.failed_to_launch() && turned_away {
                if let Some(service) = task.service_name.as_deref().and_then(|name| {
                    self.services
                        .get(&(task.cluster_name.clone(), name.to_string()))
                }) {
                    lost_launches.push((
                        service.arn.clone(),
                        service.cluster_arn.clone(),
                        task.stopped_reason.clone(),
                        at,
                    ));
                }
            }
            // A task that has reached `RUNNING` since the last comparison is a task the
            // scheduler that placed it has STARTED, which is what the service's own event
            // stream names it in.
            let started = task.last_status == STATUS_RUNNING
                && published.is_none_or(|(last, _)| last != STATUS_RUNNING);
            if let Some(name) = task.service_name.as_deref().filter(|_| started) {
                started_tasks.push(ServiceTaskReport {
                    service: (task.cluster_name.clone(), name.to_string()),
                    task_id: task.task_id.clone(),
                    at,
                });
            }
            observed.push((
                task.task_arn.clone(),
                reported,
                TaskStateChange {
                    task_arn: task.task_arn.clone(),
                    at,
                    detail: task_state_change_detail(task, at),
                },
            ));
        }
        observed.sort_by_key(|(_, _, change)| change.at);
        lost_launches.sort_by_key(|(_, _, _, at)| *at);
        for (service_arn, cluster_arn, reason, at) in lost_launches {
            self.record_service_action(
                &service_arn,
                &cluster_arn,
                SERVICE_TASK_CONFIGURATION_FAILURE,
                reason.as_deref(),
                at,
            );
        }
        for (task_arn, reported, change) in observed {
            self.published_task_status.insert(task_arn, reported);
            while self.pending_task_events.len() >= MAX_PENDING_EVENTS {
                self.pending_task_events.pop_front();
                ::tracing::warn!(
                    "the queue of ECS task state changes waiting to be published to \
                     EventBridge is full at {MAX_PENDING_EVENTS}; the oldest change was \
                     dropped"
                );
            }
            self.pending_task_events.push_back(change);
        }
        self.record_service_task_reports(started_tasks, tasks_started_message);
    }

    /// Take at most `limit` of the changes not yet published, oldest first.
    ///
    /// Taking the changes out under the lock is what stops two concurrent calls from
    /// publishing one change twice.
    fn take_task_state_changes(&mut self, limit: usize) -> Vec<TaskStateChange> {
        let taking = limit.min(self.pending_task_events.len());
        self.pending_task_events.drain(..taking).collect()
    }

    /// How many changes are queued for publication — reporting only, for a reader that has
    /// to tell "nothing moved" from "nothing was published".
    pub fn pending_task_event_count(&self) -> usize {
        self.pending_task_events.len()
    }

    /// The earliest instant at which this store owes a TASK transition, if any — the
    /// instant the clock pump waits for.
    ///
    /// Every instant a task's own lifecycle is waiting on: its placement, its start, the
    /// end of the work it was placed to do, its stop, and each step of the interface Amazon
    /// ECS is attaching for it. An instant whose transition can no longer be materialised is
    /// not owed, however long ago it fell due — a placement waiting on an interface that is
    /// NOT in place ([`TaskRecord::interfaces_attached`]), and the end of the work of a task
    /// something else has already stopped — so the pump waits for what it can actually
    /// apply and stands down when nothing is left, instead of waking over and over on an
    /// instant nothing will act on.
    pub fn next_task_due_at(&self) -> Option<SystemTime> {
        self.tasks
            .values()
            .flat_map(|task| {
                [
                    task.pending_due_at.filter(|_| task.interfaces_attached()),
                    task.running_due_at,
                    // The confirmation that a started task's run is HELD, which the steady
                    // state of the rollout that placed it waits for
                    // ([`TaskRecord::kept_running_at`]) — owed whether or not anybody calls
                    // again, and only for a task that is still running.
                    task.kept_running_due_at
                        .filter(|_| task.last_status == STATUS_RUNNING),
                    task.work_done_due_at.filter(|_| task.work_is_running()),
                    task.stopped_due_at,
                ]
                .into_iter()
                .chain(
                    task.attachments
                        .iter()
                        .map(|attachment| attachment.next_status_due_at),
                )
            })
            .flatten()
            // The next health-check execution a running container is owed is a clock-owed
            // instant too: Amazon ECS probes a container every `interval` with no further
            // call from anybody ([`super::container_health`]).
            .chain(self.next_health_probe_due_at())
            // So is the stop a task whose essential container FAILED that check earns: the
            // scheduler replaces such a task with no further call from anybody
            // ([`super::unhealthy_task_replacement`]).
            .chain(self.next_unhealthy_task_stop_due_at())
            // So is the next step of a DAEMON rollout, and the delete of a daemon that was
            // asked for: each is owed whether or not anybody calls again
            // ([`super::daemon_scheduling`]).
            .chain(self.next_daemon_due_at())
            .min()
    }

    /// What the clock pump waits for next, or `None` — having RELEASED its claim — when
    /// this store owes nothing more.
    ///
    /// Observing "nothing is owed" and standing down happen in ONE critical section, which
    /// is what makes the hand-over race-free: a write stores its task (and so its due
    /// instant) under this same lock BEFORE it tries to claim the pump, so either this call
    /// sees that due instant and the running pump keeps it, or the write's claim finds the
    /// flag already cleared and starts a pump of its own. A change recorded but not yet
    /// published keeps the pump awake too: one drain publishes at most
    /// [`MAX_PUBLISHED_PER_DRAIN`] of them, so the rest are owed another wake-up rather than
    /// the next call.
    pub fn next_task_due_or_release_pump(
        &mut self,
        now: SystemTime,
        generation: u64,
    ) -> Option<SystemTime> {
        if !self.pending_task_events.is_empty()
            || self.has_pending_service_actions()
            || self.has_pending_deployment_state_changes()
            // A container instance's change waiting on the bus keeps the pump awake for the
            // same reason a task's does: the client watching that capacity makes no further
            // Amazon ECS call ([`super::container_instance_events`]).
            || self.has_pending_container_instance_state_changes()
        {
            return Some(now);
        }
        // The service scheduler's own next step is an instant the pump waits for too: a
        // rolling update takes one step per span ([`super::rolling_update`]), and the step
        // after this one is owed whether or not anybody calls again.
        let next = [
            self.next_task_due_at(),
            self.next_scheduler_step_due_at(now),
            // So is the SERVICE-level transition the clock owes — the steady state a
            // rollout settles into above all, which is an action a client watching the bus
            // is waiting on and which no reader may be there to materialise
            // ([`super::service_action_events`]).
            self.next_service_transition_due_at(now),
            // And so is the managed-scaling action a cluster holding unplaceable tasks is
            // owed: nothing calls again between the placement and the capacity appearing, so
            // the pump is what takes it ([`super::managed_capacity_scaling`]).
            self.next_managed_capacity_due_at(),
            // And so is the next datapoint of the `AWS/ECS` utilization series of a service
            // that is RUNNING: that series is published for as long as the service runs, and
            // nothing calls again between one datapoint and the next
            // ([`super::service_utilization`]).
            self.next_service_utilization_due_at(),
            // And so is the next datapoint of the `CapacityProviderReservation` series of a
            // capacity provider whose managed scaling Amazon ECS runs: that series is published
            // for as long as the provider's wiring stands, and nothing calls again between one
            // datapoint and the next ([`super::capacity_provider_reservation_series`]).
            self.next_capacity_provider_reservation_due_at(),
            // And so is the stop a task holding a container that GAVE UP on resolving a
            // dependency earns: that container never starts, so nothing else will ever move
            // the task ([`super::container_health`], `ContainerDefinition$startTimeout`).
            self.next_dependency_give_up_due_at(now),
        ]
        .into_iter()
        .flatten()
        .min();
        match next {
            Some(due) => Some(due),
            None => {
                self.release_task_clock_pump(generation);
                None
            }
        }
    }

    /// Claim the single clock pump for this store: the claim's GENERATION when the caller
    /// may start one, `None` when one is already running.
    ///
    /// The generation is what makes a release safe: a pump releases its own claim and never
    /// a later pump's (see [`Self::release_task_clock_pump`]).
    pub fn start_task_clock_pump(&mut self) -> Option<u64> {
        if self.task_clock_pump_running {
            return None;
        }
        self.task_clock_pump_running = true;
        self.task_clock_pump_generation = self.task_clock_pump_generation.wrapping_add(1);
        Some(self.task_clock_pump_generation)
    }

    /// Release the claim `generation` took — and ONLY that one.
    ///
    /// A pump that ends on the wake-up cap must not clear a claim a pump started since is
    /// holding, or two pumps would run against one store.
    pub fn release_task_clock_pump(&mut self, generation: u64) {
        if self.task_clock_pump_generation == generation {
            self.task_clock_pump_running = false;
        }
    }

    /// Keep the running pump's task handle, aborting the one it replaces.
    ///
    /// The replaced handle is a pump that has already ended — by standing down, or on its
    /// wake-up cap — so the abort is normally a no-op; it is what guarantees the invariant
    /// anyway, that no earlier pump task is still running beside the one that now holds the
    /// claim.
    pub fn set_task_clock_pump(&mut self, handle: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.task_clock_pump.replace(handle) {
            previous.abort();
        }
    }
}

/// The instant the RECORD stamps on the status this task now reports, or `None` when it
/// stamps none.
///
/// Every status a task reaches has its own timestamp on the task (`Task$startedAt` is "the
/// time when the task started", `Task$stoppingAt` "the time when the task stops", and
/// `Task$createdAt` "the time when the task was created"), so the event's `time` is read off
/// the record rather than taken from whoever happened to drain the queue.
fn status_instant(task: &TaskRecord) -> Option<SystemTime> {
    let stamped = match task.last_status.as_str() {
        STATUS_STOPPED => task.stopped_at.or(task.stopping_at),
        STATUS_RUNNING => task.started_at,
        STATUS_DEACTIVATING => task.stopping_at,
        // `PROVISIONING` and `PENDING`: the task has been accepted and nothing has started
        // or stopped yet, so the instant it carries is its creation.
        _ => Some(task.created_at),
    };
    stamped
        .or(Some(task.created_at))
        .and_then(|at| SystemTime::try_from(at).ok())
}

/// `at` as the epoch milliseconds the event's `time` carries.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}

/// The date-time text a timestamp is reported as inside an event's `detail`: RFC 3339 in
/// UTC, which is the form every AWS event body carries an instant in.
pub(super) fn date_time_text(at: DateTime) -> Option<String> {
    at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok()
}

/// Add `name` carrying `value` when the record holds one: a member the task does not carry
/// is LEFT OUT rather than reported empty, exactly as the API's own projection leaves it
/// out.
pub(super) fn insert_text(
    detail: &mut ::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
    value: Option<String>,
) {
    if let Some(value) = value {
        detail.insert(name.to_string(), ::serde_json::Value::String(value));
    }
}

/// Add `name` carrying the date-time text of `at`, when the record holds that instant.
pub(super) fn insert_instant(
    detail: &mut ::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
    at: Option<DateTime>,
) {
    insert_text(detail, name, at.and_then(date_time_text));
}

/// The `detail` of an ECS task state change: the TASK, as the record holds it.
///
/// Every member is read off the stored task, so the event reports the task a
/// `DescribeTasks` of the same instant reports and never a second, invented copy of it.
/// `updatedAt` is the instant of THIS change, which is what tells two events about one task
/// apart.
fn task_state_change_detail(task: &TaskRecord, at: SystemTime) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    insert_text(&mut detail, "taskArn", Some(task.task_arn.clone()));
    insert_text(&mut detail, "clusterArn", Some(task.cluster_arn.clone()));
    insert_text(
        &mut detail,
        "taskDefinitionArn",
        Some(task.task_definition_arn.clone()),
    );
    insert_text(&mut detail, "lastStatus", Some(task.last_status.clone()));
    insert_text(
        &mut detail,
        "desiredStatus",
        Some(task.desired_status.clone()),
    );
    insert_text(&mut detail, "group", Some(task.group.clone()));
    insert_text(&mut detail, "launchType", task.launch_type.clone());
    insert_text(
        &mut detail,
        "capacityProviderName",
        task.capacity_provider_name.clone(),
    );
    insert_text(&mut detail, "platformVersion", task.platform_version.clone());
    insert_text(&mut detail, "cpu", task.cpu.clone());
    insert_text(&mut detail, "memory", task.memory.clone());
    insert_text(&mut detail, "startedBy", task.started_by.clone());
    insert_text(
        &mut detail,
        "containerInstanceArn",
        task.container_instance_arn.clone(),
    );
    insert_text(&mut detail, "connectivity", task.connectivity.clone());
    insert_text(&mut detail, "stopCode", task.stop_code.clone());
    insert_text(&mut detail, "stoppedReason", task.stopped_reason.clone());
    insert_instant(&mut detail, "createdAt", Some(task.created_at));
    insert_instant(&mut detail, "connectivityAt", task.connectivity_at);
    insert_instant(&mut detail, "pullStartedAt", task.pull_started_at);
    insert_instant(&mut detail, "pullStoppedAt", task.pull_stopped_at);
    insert_instant(&mut detail, "startedAt", task.started_at);
    insert_instant(&mut detail, "stoppingAt", task.stopping_at);
    insert_instant(&mut detail, "stoppedAt", task.stopped_at);
    insert_instant(
        &mut detail,
        "executionStoppedAt",
        task.execution_stopped_at,
    );
    insert_instant(&mut detail, "updatedAt", Some(reported_instant(at)));
    detail.insert(
        "version".to_string(),
        ::serde_json::Value::from(task.version),
    );
    detail.insert(
        "attachments".to_string(),
        ::serde_json::Value::Array(task.attachments.iter().map(attachment_detail).collect()),
    );
    detail.insert(
        "containers".to_string(),
        ::serde_json::Value::Array(
            task.containers
                .iter()
                .map(|container| container_detail(container, &task.attachments))
                .collect(),
        ),
    );
    ::serde_json::Value::Object(detail)
}

/// One attachment of the task, as the event's `detail` carries it — the same four members
/// the `Attachment` projection reports ([`AttachmentRecord::view`]).
fn attachment_detail(attachment: &AttachmentRecord) -> ::serde_json::Value {
    let mut rendered = ::serde_json::Map::new();
    insert_text(&mut rendered, "id", Some(attachment.id.clone()));
    insert_text(
        &mut rendered,
        "type",
        Some(attachment.attachment_type.clone()),
    );
    insert_text(&mut rendered, "status", Some(attachment.status.clone()));
    rendered.insert(
        "details".to_string(),
        ::serde_json::Value::Array(
            attachment
                .details
                .iter()
                .map(|detail| {
                    let mut pair = ::serde_json::Map::new();
                    insert_text(&mut pair, "name", detail.name.clone());
                    insert_text(&mut pair, "value", detail.value.clone());
                    ::serde_json::Value::Object(pair)
                })
                .collect(),
        ),
    );
    ::serde_json::Value::Object(rendered)
}

/// One container of the task, as the event's `detail` carries it: what the `Container`
/// projection reports about it, read off the same record
/// ([`ContainerRecord::view`]).
fn container_detail(
    container: &ContainerRecord,
    attachments: &[AttachmentRecord],
) -> ::serde_json::Value {
    let mut rendered = ::serde_json::Map::new();
    insert_text(
        &mut rendered,
        "containerArn",
        Some(container.container_arn.clone()),
    );
    insert_text(&mut rendered, "taskArn", Some(container.task_arn.clone()));
    insert_text(&mut rendered, "name", container.name.clone());
    insert_text(&mut rendered, "image", container.image.clone());
    insert_text(&mut rendered, "imageDigest", container.image_digest.clone());
    insert_text(&mut rendered, "runtimeId", container.runtime_id.clone());
    insert_text(
        &mut rendered,
        "lastStatus",
        Some(container.last_status.clone()),
    );
    insert_text(&mut rendered, "cpu", container.cpu.clone());
    insert_text(&mut rendered, "memory", container.memory.clone());
    insert_text(
        &mut rendered,
        "memoryReservation",
        container.memory_reservation.clone(),
    );
    insert_text(&mut rendered, "reason", container.reason.clone());
    if let Some(exit_code) = container.exit_code {
        rendered.insert(
            "exitCode".to_string(),
            ::serde_json::Value::from(exit_code),
        );
    }
    // `Container$networkInterfaces`: the interfaces the container is reached on, which for
    // an `awsvpc` task are the ones its TASK was allocated — the same resolution the API's
    // own projection makes.
    rendered.insert(
        "networkInterfaces".to_string(),
        ::serde_json::Value::Array(
            attachments
                .iter()
                .filter_map(AttachmentRecord::container_network_interface)
                .map(|interface| {
                    let mut pair = ::serde_json::Map::new();
                    insert_text(&mut pair, "attachmentId", interface.attachment_id);
                    insert_text(
                        &mut pair,
                        "privateIpv4Address",
                        interface.private_ipv4_address,
                    );
                    ::serde_json::Value::Object(pair)
                })
                .collect(),
        ),
    );
    ::serde_json::Value::Object(rendered)
}

/// Record what THIS call moved, and hand the publication — and the rest of the lifecycle —
/// to the clock pump.
///
/// Called by a door that WRITES a task's status, AFTER it has released the store's lock:
/// this takes that lock itself to compare the task map against what was last published
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::record_task_state_changes`]),
/// so a door still holding the guard must drop it first. A call that moved nothing records
/// nothing, so it costs one comparison of the task map.
///
/// The publication itself is the pump's ([`drive_task_clock`]), which is what lets a door
/// answer its caller without waiting on another service: the event leaves one pump tick
/// later, and each transition the clock still owes the task leaves as it falls due.
pub fn publish_task_state_changes<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    record_task_moves(engine);
    drive_task_clock(ctx, engine, region);
}

/// Compare this store's task map against what was last published, so every move since then
/// is queued as an event — the ONE comparison both announcements make
/// ([`publish_task_state_changes`] for a door's write,
/// [`publish_data_plane_task_state_changes`] for the end of a container's run).
///
/// Called with NO guard of this store held: it takes that lock itself.
fn record_task_moves<Eng: Send + Sync + 'static>(engine: &::std::sync::Arc<Eng>) {
    if let Some(state) = service_state(&**engine) {
        let mut store = state.lock();
        let now = SystemTime::now();
        store.record_task_state_changes(now);
        // The change the same write made to a CONTAINER INSTANCE travels the same hop: a
        // drain, a deregistration or a registration is announced by the door that made it,
        // exactly as a task's move is ([`super::container_instance_events`]).
        store.record_container_instance_state_changes(now);
    }
}

/// Publish every task state change this store has to publish, and leave the queue empty.
///
/// Called with NO lock held: each drain records and takes its changes out in one critical
/// section, so nothing is published twice, and the in-process `PutEvents` runs outside it.
/// The queue is drained until it is EMPTY — one drain is capped
/// ([`MAX_PUBLISHED_PER_DRAIN`]) so no single pass is unbounded, and the loop is what
/// guarantees the tail of a burst goes out with it. Nothing about the answer an operation
/// gives changes either way.
async fn publish_pending<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
    loop {
        let (changes, actions, deployments, instances) = {
            let mut store = state.lock();
            store.record_task_state_changes(SystemTime::now());
            store.record_container_instance_state_changes(SystemTime::now());
            (
                store.take_task_state_changes(MAX_PUBLISHED_PER_DRAIN),
                // The actions this store's SERVICE scheduler has taken travel the same hop
                // as a task's moves, and are drained in the same critical section, so one
                // pump publishes everything this service owes the bus
                // ([`super::service_action_events`]).
                store.take_service_action_events(MAX_PUBLISHED_PER_DRAIN),
                // And so do the rollout states this store's service DEPLOYMENTS have moved
                // into ([`super::deployment_events`]).
                store.take_deployment_state_changes(MAX_PUBLISHED_PER_DRAIN),
                // And so do the changes this store's CONTAINER INSTANCES have experienced
                // ([`super::container_instance_events`]).
                store.take_container_instance_state_changes(MAX_PUBLISHED_PER_DRAIN),
            )
        };
        if changes.is_empty()
            && actions.is_empty()
            && deployments.is_empty()
            && instances.is_empty()
        {
            return;
        }
        publish_batch(&client, &changes).await;
        super::service_action_events::publish_service_actions(&client, &actions).await;
        super::deployment_events::publish_deployment_state_changes(&client, &deployments).await;
        super::container_instance_events::publish_container_instance_state_changes(
            &client, &instances,
        )
        .await;
    }
}

/// Publish one drain of task state changes.
async fn publish_batch(client: &::aws_sdk_eventbridge::Client, changes: &[TaskStateChange]) {
    let entries: Vec<(&str, SystemTime, &::serde_json::Value)> = changes
        .iter()
        // The event names the resource it is about, which is what a rule filtering on
        // `resources` reads.
        .map(|change| (change.task_arn.as_str(), change.at, &change.detail))
        .collect();
    publish_events(client, TASK_STATE_CHANGE_DETAIL_TYPE, &entries).await;
}

/// Publish `entries` under ONE `detail-type`, in `PutEvents` requests of at most
/// [`MAX_PUT_EVENTS_ENTRIES`] entries.
///
/// The one place this service builds an event envelope: whatever moved — a task or the
/// service scheduler — the event is published onto the account's default event bus
/// ([`DEFAULT_EVENT_BUS`]) under this service's own `source` ([`EVENT_SOURCE`]), naming the
/// resource it is about in `resources` and the instant it happened at in `time`.
pub(super) async fn publish_events(
    client: &::aws_sdk_eventbridge::Client,
    detail_type: &str,
    entries: &[(&str, SystemTime, &::serde_json::Value)],
) {
    for window in entries.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for (resource_arn, at, detail) in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(detail_type)
                // The event's `time` is WHEN it happened, not when the queue was drained:
                // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339.
                // If no time stamp is provided, the time stamp of the PutEvents call is
                // used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(*at),
                ))
                .resources(*resource_arn)
                .detail(detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an ECS {detail_type} event could not be published to EventBridge"
            );
        }
    }
}

/// Publish the task state changes the CLOCK owes, as they fall due, with no further call
/// from anybody.
///
/// Amazon ECS notifies EventBridge as a task moves, which is the whole point of the event
/// path: a client that reacts to a task through EventBridge makes no further Amazon ECS
/// call, so a transition published only by the next reader would never be published at all.
/// This starts ONE in-process task that waits for the earliest due instant the store holds,
/// materialises what is then due
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`], the same call
/// every operation makes) and publishes what that produced, until nothing is owed.
///
/// It does not change WHEN a transition happens: `advance_clock` applies only what the
/// stored due instant has already made due, so a caller reading before that instant still
/// sees the transient status, and two reads at the same instant still answer identically.
/// The task waits for the clock so that no request ever has to.
///
/// At most one pump runs per store (the store carries the claim), it is bounded by
/// [`MAX_CLOCK_WAKE_UPS`] wake-ups, and it exits as soon as the store owes nothing. With no
/// async runtime to spawn onto there is no pump, and the transitions are materialised and
/// published by the next operation instead.
/// Hand the CAPACITY a reporting operation has just materialised to the clock pump, so the
/// events its changes earned reach the bus with no further call.
///
/// The registration an Amazon EC2 instance's own agent makes is owed by the CLOCK and by no
/// call ([`super::ec2_capacity`]): a client watching a cluster's capacity through EventBridge
/// polls the bus, not Amazon ECS, so a change announced only by the next WRITING door would
/// never be announced at all. A reporting operation that settled that capacity therefore hands
/// what it materialised to the same single pump every door shares.
///
/// Only the pump is started: a read decides nothing about a launch, so none of the steps
/// [`drive_task_clock`] takes for a door that has just WRITTEN a task belongs here. The pump
/// changes no answer this call gives and waits for nothing — it materialises only what the
/// stored due instant has already made due, exactly as the reading door itself does.
///
/// Called with NO guard of this store held.
pub fn drive_capacity_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    spawn_task_clock_pump(ctx.callback(), engine, region);
}

pub fn drive_task_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    // An engine that carries no store of this service, and a thread with no async runtime to
    // spawn a pump onto, leave this call nothing to do — the same two conditions
    // [`spawn_task_clock_pump`] stands down on, answered here so the steps below keep running
    // only where the pump they lead up to can.
    if service_state(&**engine).is_none() || ::tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    // The execution resources of every launch that is waiting for them are retrieved BEFORE
    // the pump's claim is taken, so a door that has just placed a task has the values its
    // containers take read before the clock could start it, whether or not a pump was already
    // running ([`super::settle_secret_retrievals`]).
    super::settle_secret_retrievals(ctx.callback(), engine);
    // And so are the task-role CREDENTIALS of every task that has not been vended them: the
    // containers of a task that resolves a task role wait for the step that mints one, and
    // minting it needs the credential store this request's own context carries
    // ([`super::settle_task_role_credentials`]), which the pump has none of.
    super::settle_task_role_credentials(ctx.credential_store(), &**engine);
    // And the agent-connection decision every exec-enabled task waits for: whether its task
    // role authorizes the Systems Manager channel its execute-command agent connects over is
    // an IAM question, which the pump cannot ask either
    // ([`super::settle_execute_command_agent_connections`]).
    super::settle_execute_command_agent_connections(ctx.callback(), engine);
    // And the Session Manager records of the execute-command sessions that have ended: the
    // session is this store's and the record is that service's, so what a clock pass left
    // owed is carried out here ([`super::settle_exec_session_records`]).
    super::settle_exec_session_records(&**engine, SystemTime::now());
    // The container data plane is settled BEFORE the pump's claim is taken, so a door that
    // finds a pump already running still has the containers of the task it just wrote
    // started or signalled ([`super::settle_container_runtime`]).
    super::settle_container_runtime(ctx.callback(), engine, region);
    // And the rollouts whose CloudWatch alarms have fired: an alarm moves with no further
    // Amazon ECS call, so a door that has just written a service reads what those alarms have
    // made of its rollout before the pump is claimed ([`super::settle_deployment_alarms`]).
    super::settle_deployment_alarms(&**engine);
    spawn_task_clock_pump(ctx.callback(), engine, region);
}

/// Record the task state change the CONTAINER DATA PLANE has just produced, and drive the
/// clock so the transitions it earns are materialised and published.
///
/// The end of a container's PROGRAM is a task transition no stored due instant can predict:
/// the program itself decides when it ends, and a standalone task whose essential container
/// has exited is stopped at that instant
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::record_container_outcome`]) — so
/// the run that has just ended is the only component that knows the task moved, and it
/// announces that move exactly as a door announces the moves its own write produced
/// ([`publish_task_state_changes`]). Without it the stop a container's exit earns waits for
/// the next caller to materialise it, and a client that watches the task through EventBridge
/// makes none: the `STOPPED` event it is waiting for would never be published at all.
///
/// Called with NO guard of this store held, from the run's own in-process task: it takes that
/// lock itself to compare the task map against what was last published, and hands the
/// publication to the same single pump every door shares.
pub fn publish_data_plane_task_state_changes<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    record_task_moves(engine);
    spawn_task_clock_pump(callback, engine, region);
}

/// Drive the clock for the MANAGED-SCALING wiring a capacity-provider call has just changed.
///
/// The `CapacityProviderReservation` series Amazon ECS publishes for a provider whose scaling it
/// manages is owed by the clock for as long as that wiring stands, and nothing calls again
/// between one datapoint and the next — so the door that created, re-targeted or associated the
/// provider OPENS that series ([`super::settle_capacity_provider_reservation`]) and hands the
/// datapoints after it to the same single pump every door shares. It changes no answer this call
/// gives and waits for nothing.
///
/// Called with NO guard of this store held.
pub fn drive_managed_scaling_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    super::settle_capacity_provider_reservation(&**engine, SystemTime::now());
    spawn_task_clock_pump(ctx.callback(), engine, region);
}

/// Start the ONE clock pump of this store, unless a pump is already driving it.
///
/// The half of [`drive_task_clock`] that needs no request context, so the container data
/// plane reaches it too ([`publish_data_plane_task_state_changes`]).
fn spawn_task_clock_pump<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = state.lock().start_task_clock_pump() else {
        return;
    };
    let callback = callback.clone();
    let engine = ::std::sync::Arc::clone(engine);
    let region = region.to_string();
    let pumped = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap below is what ended the loop; a pump that ran out
        // of work has already released its claim inside the one critical section that
        // observed it (`next_task_due_or_release_pump`).
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let now = SystemTime::now();
            let Some(due) = state.lock().next_task_due_or_release_pump(now, generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps the
            // backstop above from ever costing a CPU-bound spin, and it hands the runtime
            // back between wake-ups.
            let wait = due
                .duration_since(now)
                .unwrap_or_default()
                .clamp(MIN_CLOCK_WAIT, MAX_CLOCK_WAIT);
            ::tokio::time::sleep(wait).await;
            // The launch steps of a task the CLOCK placed — the scheduler's replacement for a
            // task its service lost ([`super::service_steady_state`]) — which no call is
            // there to take for it: the execution role is assumed and the images are fetched.
            // Made with no guard of this store held, since those reads take the IAM, Amazon
            // EC2 and Amazon ECR stores' locks ([`super::settle_task_launches`]): each is a
            // lookup in that emulated service's own in-memory store through the shared engine
            // handle, so nothing is dialled, resolved or opened. Done BEFORE the clock
            // materialises what is due, because both steps happen where the task would
            // otherwise start — and again after it, for the tasks that pass placed
            // ([`super::advance_task_clock`], the same prologue every operation runs).
            super::advance_task_clock(&*engine, SystemTime::now());
            // The execution resources of a launch the CLOCK placed, which no door is there
            // to retrieve for it: the values its containers take are read as the execution
            // role before any of them starts ([`super::settle_secret_retrievals`]).
            super::settle_secret_retrievals(&callback, &engine);
            // A task the clock has just started or stopped is one the load balancers its
            // service binds have to be brought in line with — the same side effect the door
            // that wrote a status settles, owed here for the statuses NO door wrote
            // ([`super::settle_load_balancer_targets`]).
            super::settle_load_balancer_targets(&*engine);
            // The Container Insights series each cluster the setting is ON for owes, and the
            // performance log events beside them, which the collection interval makes due
            // whether or not anybody called ([`super::settle_container_insights`]) — taken off
            // the very records the pass above has just moved, with no guard of this store held.
            super::settle_container_insights(&callback, &*engine, &region).await;
            // The `AWS/ECS` utilization series each service holding a running task owes, which
            // the publish cadence makes due whether or not anybody called
            // ([`super::settle_service_utilization`]) — read off the same records the pass
            // above has just moved, with no guard of this store held.
            super::settle_service_utilization(&*engine);
            // A task the clock has just started is one whose containers are now running, and
            // one it has just stopped is one whose containers are signalled: the same side
            // effect the door that wrote a status settles, owed here for the statuses NO
            // door wrote ([`super::settle_container_runtime`]).
            super::settle_container_runtime(&callback, &engine, &region);
            // A rollout whose CloudWatch alarms have moved into their `ALARM` state is one
            // Amazon ECS has failed, whether or not anybody called to look: the alarm is
            // moved by the metric it watches, and a client that drives that metric and then
            // polls the service makes no call that would otherwise notice
            // ([`super::settle_deployment_alarms`]). Read with no guard of this store held.
            super::settle_deployment_alarms(&*engine);
            // The Session Manager records of the execute-command sessions a pass of the clock
            // has ended — a session whose task it has just stopped carries no stream any
            // longer — and the sessions that service has been asked to end, which is the same
            // fact read from its side ([`super::settle_exec_session_records`]). Read with no
            // guard of this store held.
            super::settle_exec_session_records(&*engine, SystemTime::now());
            publish_pending(&callback, &*engine, &region).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its own.
            state.lock().release_task_clock_pump(generation);
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely forgotten:
    // that is what guarantees no earlier pump task runs beside the one holding the claim.
    pumped.lock().set_task_clock_pump(handle);
}
