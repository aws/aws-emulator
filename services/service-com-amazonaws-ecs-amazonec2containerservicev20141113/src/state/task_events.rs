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
    pub fn record_task_state_changes(&mut self, now: SystemTime) {
        let tasks = &self.tasks;
        self.published_task_status
            .retain(|task_arn, _| tasks.contains_key(task_arn));
        let mut observed: Vec<(String, TaskStatusPair, TaskStateChange)> = Vec::new();
        for task in self.tasks.values() {
            let reported = (task.last_status.clone(), task.desired_status.clone());
            if self.published_task_status.get(&task.task_arn) == Some(&reported) {
                continue;
            }
            let at = status_instant(task).unwrap_or(now);
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
        if !self.pending_task_events.is_empty() {
            return Some(now);
        }
        match self.next_task_due_at() {
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
fn date_time_text(at: DateTime) -> Option<String> {
    at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok()
}

/// Add `name` carrying `value` when the record holds one: a member the task does not carry
/// is LEFT OUT rather than reported empty, exactly as the API's own projection leaves it
/// out.
fn insert_text(
    detail: &mut ::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
    value: Option<String>,
) {
    if let Some(value) = value {
        detail.insert(name.to_string(), ::serde_json::Value::String(value));
    }
}

/// Add `name` carrying the date-time text of `at`, when the record holds that instant.
fn insert_instant(
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
    insert_instant(&mut detail, "updatedAt", Some(DateTime::from(at)));
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
    if let Some(state) = service_state(&**engine) {
        state.lock().record_task_state_changes(SystemTime::now());
    }
    drive_task_clock(ctx, engine, region);
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
        let changes = {
            let mut store = state.lock();
            store.record_task_state_changes(SystemTime::now());
            store.take_task_state_changes(MAX_PUBLISHED_PER_DRAIN)
        };
        if changes.is_empty() {
            return;
        }
        publish_batch(&client, &changes).await;
    }
}

/// Publish one drain, in `PutEvents` requests of at most [`MAX_PUT_EVENTS_ENTRIES`]
/// entries.
async fn publish_batch(client: &::aws_sdk_eventbridge::Client, changes: &[TaskStateChange]) {
    for window in changes.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for change in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(TASK_STATE_CHANGE_DETAIL_TYPE)
                // The event's `time` is WHEN the task moved, not when the queue was
                // drained: `PutEventsRequestEntry$Time` is "the time stamp of the event,
                // per RFC3339. If no time stamp is provided, the time stamp of the
                // PutEvents call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(change.at),
                ))
                // The event names the resource it is about, which is what a rule filtering
                // on `resources` reads.
                .resources(&change.task_arn)
                .detail(change.detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an ECS task state change could not be published to EventBridge"
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
pub fn drive_task_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    // The container data plane is settled BEFORE the pump's claim is taken, so a door that
    // finds a pump already running still has the containers of the task it just wrote
    // started or signalled ([`super::settle_container_runtime`]).
    super::settle_container_runtime(ctx.callback(), engine, region);
    let Some(generation) = state.lock().start_task_clock_pump() else {
        return;
    };
    let callback = ctx.callback().clone();
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
            // A task the clock has just started or stopped is one the load balancers its
            // service binds have to be brought in line with — the same side effect the door
            // that wrote a status settles, owed here for the statuses NO door wrote
            // ([`super::settle_load_balancer_targets`]).
            super::settle_load_balancer_targets(&*engine);
            // A task the clock has just started is one whose containers are now running, and
            // one it has just stopped is one whose containers are signalled: the same side
            // effect the door that wrote a status settles, owed here for the statuses NO
            // door wrote ([`super::settle_container_runtime`]).
            super::settle_container_runtime(&callback, &engine, &region);
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
