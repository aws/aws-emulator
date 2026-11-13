// SPDX-License-Identifier: CC0-1.0

//! The container HEALTH CHECK: the probe a container definition declares, and what running
//! it makes of the container's — and the task's — `healthStatus`.
//!
//! # The rule this module implements
//!
//! `ContainerDefinition$healthCheck` is "an object representing a container health check",
//! and Amazon ECS RUNS it: "The Amazon ECS container agent only monitors and reports on the
//! health checks specified in the task definition." `HealthCheck$command` is "the command
//! that the container runs to determine if it is healthy ... An exit code of 0 indicates
//! success, and non-zero exit code indicates failure", `interval` "the time period in
//! seconds between each health check execution", `timeout` "the time period in seconds to
//! wait for a health check to succeed before it is considered a failure", `retries` "the
//! number of times to retry a failed health check before the container is considered
//! unhealthy", and `startPeriod` "the optional grace period to provide containers time to
//! bootstrap before failed health checks count towards the maximum number of retries" —
//! with the note that "if a health check succeeds within the `startPeriod`, then the
//! container is considered healthy and any subsequent failures count toward the maximum
//! number of retries".
//!
//! So, for a container whose definition declares a command:
//!
//! * the probe is EXECUTED — in this process, through the same session a container's own
//!   `command` runs in ([`ContainerShell`]) — once the container is running, and again one
//!   `interval` after each execution;
//! * an execution that exits zero makes the container `HEALTHY` and clears the failures
//!   counted against it; one that exits non-zero, or that the `timeout` ran out on, counts
//!   as a failure;
//! * `retries` consecutive failures make it `UNHEALTHY`, except while the start period is
//!   still running and nothing has succeeded yet — a failure there counts toward nothing,
//!   which is what the grace period IS, and the suppression ends with the period rather
//!   than lasting forever;
//! * until an execution has produced a result the container is `UNKNOWN`, which is also
//!   what a container with NO health check reports for ever ("`UNKNOWN` — The container
//!   health check is being evaluated, there's no container health check defined, or Amazon
//!   ECS doesn't have the health status of the container").
//!
//! `Task$healthStatus` is the task-level reading of the same results: "determined by the
//! health of the essential containers in the task. If all essential containers in the task
//! are reporting as `HEALTHY`, the task status also reports as `HEALTHY`. If any essential
//! containers in the task are reporting as `UNHEALTHY` or `UNKNOWN`, the task status also
//! reports as `UNHEALTHY` or `UNKNOWN`" — in the priority order the `HealthCheck` doc gives
//! (`UNHEALTHY`, then `UNKNOWN`, then `HEALTHY`).
//!
//! # The containers that WAIT
//!
//! `ContainerDependency` is the other reader of those results:
//! `ContainerDependency$condition`'s `HEALTHY` "validates that the dependent container
//! passes its Docker health check before permitting other containers to start", `START`
//! "validates that a dependent container is started before permitting other containers to
//! start", `COMPLETE` "validates that a dependent container runs to completion (exits)
//! before permitting other containers to start" and `SUCCESS` "is the same as `COMPLETE`,
//! but it also requires that the container exits with a `zero` status". A container whose
//! dependencies are not met yet therefore does NOT start with its task: it stays `PENDING`
//! until they are ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::start_due_containers`]),
//! and nothing runs its program before then.
//!
//! # Who runs a probe
//!
//! Probes are RECONCILED from the task map, never run by whoever happened to call
//! ([`settle_container_health`]), exactly as a container's own program is
//! ([`super::container_runtime`]): the pass takes the probes whose instant has come, runs
//! each outside the store's lock, and records what it exited with. The probe instants are
//! part of what the clock pump waits for, so a container's health moves with the clock and
//! not with the next reader; a read only materialises what was already due.
//!
//! # Bounds
//!
//! At most [`MAX_HEALTH_PROBES_IN_FLIGHT`] probes run at once, and each is given its own
//! `timeout` of wall clock — past which it is the failure the member documents. The rest of
//! the bounds are the session's ([`ContainerShell`]).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use super::*;

/// `Container$healthStatus` of a container whose probe passed, from the `HealthStatus` enum
/// (`smithy_query(query="shape",shape="HealthStatus")` -> HEALTHY, UNHEALTHY, UNKNOWN):
/// "`HEALTHY` — The container health check has passed successfully."
pub const HEALTH_HEALTHY: &str = "HEALTHY";

/// `Container$healthStatus` of a container whose probe has failed `retries` times over, from
/// the same enum: "`UNHEALTHY` — The container health check has failed."
pub const HEALTH_UNHEALTHY: &str = "UNHEALTHY";

/// `HealthCheck$interval` when the member is absent: "The default value is 30 seconds."
pub const DEFAULT_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// `HealthCheck$timeout` when the member is absent: "The default value is 5."
pub const DEFAULT_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// `HealthCheck$retries` when the member is absent: "The default value is 3."
pub const DEFAULT_HEALTH_CHECK_RETRIES: i32 = 3;

/// The range `HealthCheck$interval` publishes: "You may specify between 5 and 300 seconds."
///
/// A value a request names is CLAMPED into it before anything waits on it: the interval is
/// this emulator's own wall clock between two in-process executions, so no request may
/// choose it outside the span AWS publishes for the member.
pub const MIN_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// See [`MIN_HEALTH_CHECK_INTERVAL`].
pub const MAX_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(300);

/// The range `HealthCheck$timeout` publishes: "You may specify between 2 and 60 seconds."
///
/// Clamped for the same reason, and it is the bound that matters most: the timeout is how
/// long ONE in-process probe may hold a slot of [`MAX_HEALTH_PROBES_IN_FLIGHT`], so a
/// request cannot ask this emulator to wait longer than the member allows.
pub const MIN_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
/// See [`MIN_HEALTH_CHECK_TIMEOUT`].
pub const MAX_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest `HealthCheck$startPeriod` the member publishes: "You can specify between 0
/// and 300 seconds." Clamped like the two above.
pub const MAX_HEALTH_CHECK_START_PERIOD: Duration = Duration::from_secs(300);

/// The range `HealthCheck$retries` publishes: "You may specify between 1 and 10 retries."
/// Clamped, so the number of executions a container's status waits on is bounded too.
pub const MIN_HEALTH_CHECK_RETRIES: i32 = 1;
/// See [`MIN_HEALTH_CHECK_RETRIES`].
pub const MAX_HEALTH_CHECK_RETRIES: i32 = 10;

/// The first word a `HealthCheck$command` takes to run its arguments directly: "The string
/// array must start with `CMD` to run the command arguments directly."
pub const HEALTH_CHECK_EXEC_FORM: &str = "CMD";

/// The first word a `HealthCheck$command` takes to run its argument through a shell: "...or
/// `CMD-SHELL` to run the command with the container's default shell."
pub const HEALTH_CHECK_SHELL_FORM: &str = "CMD-SHELL";

/// How many health-check probes this store runs at once. THIS NUMBER IS THE EMULATOR'S, not
/// an AWS quota: every probe is an in-process task of this emulator, so without a ceiling a
/// caller running tasks without end would start work without end. Sized like the container
/// runs' own ceiling ([`MAX_CONTAINER_RUNS`]).
pub const MAX_HEALTH_PROBES_IN_FLIGHT: usize = 64;

/// The health check ONE container declares, and where its evaluation has got to.
///
/// The configuration is COPIED off the container definition when the task's containers are
/// minted, so a probe runs what the revision the task runs declared — a later revision of
/// the family changes the next task's probes, not this one's.
#[derive(Debug, Clone)]
pub struct ContainerHealthRecord {
    /// `HealthCheck$command`, as the definition declared it.
    pub command: Vec<String>,
    /// `HealthCheck$interval`, defaulted ([`DEFAULT_HEALTH_CHECK_INTERVAL`]).
    pub interval: Duration,
    /// `HealthCheck$timeout`, defaulted ([`DEFAULT_HEALTH_CHECK_TIMEOUT`]).
    pub timeout: Duration,
    /// `HealthCheck$retries`, defaulted ([`DEFAULT_HEALTH_CHECK_RETRIES`]) and never below
    /// one: the first failure after the last success is already one try.
    pub retries: i32,
    /// `HealthCheck$startPeriod`, and `None` for the default the member documents ("By
    /// default, the `startPeriod` is off").
    pub start_period: Option<Duration>,
    /// The `healthStatus` this container reports now.
    pub status: String,
    /// How many executions have failed since the last one that succeeded.
    pub failures: i32,
    /// Whether any execution has succeeded yet, which is what ends the start period's
    /// suppression early ("If a health check succeeds within the `startPeriod`, then the
    /// container is considered healthy and any subsequent failures count toward the maximum
    /// number of retries").
    pub succeeded: bool,
    /// When the start period ENDS, for a container that declared one — set from the instant
    /// the container started running.
    pub start_period_ends_at: Option<SystemTime>,
    /// When the next execution falls due, and `None` before the container has started
    /// running. An execution in flight does NOT clear it, and the instant it holds is always
    /// in the future: while a probe runs it stands past that probe's own deadline
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::settle_container_health_probes`]),
    /// and the probe's outcome re-arms it one `interval` on ([`Self::record`]) — so the clock
    /// always has something to WAIT for and is never woken by an instant that has elapsed.
    pub next_probe_due_at: Option<SystemTime>,
    /// Whether an execution is in flight, so one container is probed once at a time.
    pub probe_in_flight: bool,
}

impl ContainerHealthRecord {
    /// The record for a container definition's `healthCheck`, or `None` when the definition
    /// declares none — "there's no container health check defined", the container that
    /// reports `UNKNOWN` for ever — or when the check names no PROGRAM to run
    /// ([`health_probe_program`]), which is a check nothing can execute and so no check at
    /// all.
    ///
    /// Every member but the command is optional and each has a documented default, so an
    /// absent member is the default rather than a reason not to probe. Every one of them is
    /// also CLAMPED into the range its own member publishes before anything waits on it: the
    /// interval, the timeout and the start period are spans this emulator holds an
    /// in-process probe for, so no request chooses them
    /// ([`MIN_HEALTH_CHECK_INTERVAL`], [`MIN_HEALTH_CHECK_TIMEOUT`],
    /// [`MAX_HEALTH_CHECK_START_PERIOD`], [`MAX_HEALTH_CHECK_RETRIES`]).
    pub fn from_definition(check: Option<&HealthCheck>) -> Option<Self> {
        let check = check?;
        health_probe_program(&check.command)?;
        Some(Self {
            command: check.command.clone(),
            interval: seconds_or(check.interval, DEFAULT_HEALTH_CHECK_INTERVAL)
                .clamp(MIN_HEALTH_CHECK_INTERVAL, MAX_HEALTH_CHECK_INTERVAL),
            timeout: seconds_or(check.timeout, DEFAULT_HEALTH_CHECK_TIMEOUT)
                .clamp(MIN_HEALTH_CHECK_TIMEOUT, MAX_HEALTH_CHECK_TIMEOUT),
            retries: check
                .retries
                .unwrap_or(DEFAULT_HEALTH_CHECK_RETRIES)
                .clamp(MIN_HEALTH_CHECK_RETRIES, MAX_HEALTH_CHECK_RETRIES),
            start_period: check
                .start_period
                .filter(|seconds| *seconds > 0)
                .and_then(|seconds| u64::try_from(seconds).ok())
                .map(Duration::from_secs)
                .map(|period| period.min(MAX_HEALTH_CHECK_START_PERIOD)),
            // "UNKNOWN - The container health check is being evaluated": nothing has run
            // yet, so no result is reported.
            status: HEALTH_UNKNOWN.to_string(),
            failures: 0,
            succeeded: false,
            start_period_ends_at: None,
            next_probe_due_at: None,
            probe_in_flight: false,
        })
    }

    /// Start this check's schedule from `now` — the instant the container started running,
    /// which is when the agent begins probing it and when the grace period the definition
    /// asked for begins.
    ///
    /// The first execution falls due one `interval` on, which is the only spacing the member
    /// documents ("the time period in seconds between each health check execution"), so a
    /// container reports `UNKNOWN` until its first probe has run. Idempotent: a container
    /// whose schedule has already started keeps it.
    pub fn begin(&mut self, now: SystemTime) {
        if self.next_probe_due_at.is_some() || self.probe_in_flight {
            return;
        }
        self.start_period_ends_at = self.start_period.map(|period| now + period);
        self.next_probe_due_at = Some(now + self.interval);
    }

    /// Whether an execution is due as of `now`.
    pub fn probe_due(&self, now: SystemTime) -> bool {
        !self.probe_in_flight && self.next_probe_due_at.is_some_and(|due| due <= now)
    }

    /// Record what ONE execution ended as — answering whether the reported `healthStatus`
    /// changed — and schedule the next one, one `interval` on.
    ///
    /// `passed` is the exit status read as the member documents it: "An exit code of 0
    /// indicates success, and non-zero exit code indicates failure", with an execution the
    /// `timeout` ran out on counted as the failure that member documents.
    ///
    /// `interval` is "the time period in seconds between each health check execution", so
    /// the next execution is owed that long after this one is over — two executions of one
    /// container's check never overlap, and the instant it falls due is always in the FUTURE,
    /// which is what keeps the clock waiting rather than waking on an elapsed instant.
    pub fn record(&mut self, passed: bool, now: SystemTime) -> bool {
        let before = self.status.clone();
        if passed {
            self.failures = 0;
            self.succeeded = true;
            self.status = HEALTH_HEALTHY.to_string();
        } else if self.suppresses_failure(now) {
            // The grace period: a failure here "count[s] towards" nothing, so the container
            // is still being evaluated rather than unhealthy.
            self.status = HEALTH_UNKNOWN.to_string();
        } else {
            self.failures = self.failures.saturating_add(1);
            if self.failures >= self.retries {
                self.status = HEALTH_UNHEALTHY.to_string();
            }
        }
        self.probe_in_flight = false;
        self.next_probe_due_at = Some(now + self.interval);
        self.status != before
    }

    /// Whether a FAILED execution at `now` counts toward the retries, or is suppressed by
    /// the start period: it is suppressed only while that period is still running AND
    /// nothing has succeeded yet, since a success inside it ends the grace ("any subsequent
    /// failures count toward the maximum number of retries").
    fn suppresses_failure(&self, now: SystemTime) -> bool {
        !self.succeeded
            && self
                .start_period_ends_at
                .is_some_and(|ends_at| now < ends_at)
    }
}

/// `seconds` as a duration, or `fallback` when the member carried none or a value that is
/// not a duration at all.
fn seconds_or(seconds: Option<i32>, fallback: Duration) -> Duration {
    seconds
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .map(Duration::from_secs)
        .unwrap_or(fallback)
}

/// The program a `HealthCheck$command` runs, read as the member's two forms state it, or
/// `None` when the array names nothing to run.
///
/// "The string array must start with `CMD` to run the command arguments directly, or
/// `CMD-SHELL` to run the command with the container's default shell." An array that starts
/// with neither names no form, so it is run the way a container's own `command` array is —
/// as the argument vector it is, which is what `CMD` means.
pub fn health_probe_program(command: &[String]) -> Option<ContainerProgram> {
    let (first, rest) = command.split_first()?;
    match first.as_str() {
        HEALTH_CHECK_SHELL_FORM => {
            let text = rest.join(" ");
            (!text.trim().is_empty()).then_some(ContainerProgram::Script(text))
        }
        HEALTH_CHECK_EXEC_FORM => (!rest.is_empty()).then(|| ContainerProgram::Argv(rest.to_vec())),
        _ => Some(ContainerProgram::Argv(command.to_vec())),
    }
}

/// The `healthStatus` a set of readings comes to, in the priority order the `HealthCheck`
/// doc publishes for a task: `UNHEALTHY` when any reading is unhealthy, `UNKNOWN` when any
/// is unknown, and `HEALTHY` only when every one of them is healthy.
///
/// A set with NOTHING in it is `UNKNOWN`: nothing reported health, which is the condition
/// that value states.
pub fn aggregate_health_status<'a>(readings: impl IntoIterator<Item = &'a str>) -> &'static str {
    let mut seen = false;
    let mut unknown = false;
    for reading in readings {
        seen = true;
        match reading {
            HEALTH_UNHEALTHY => return HEALTH_UNHEALTHY,
            HEALTH_HEALTHY => {}
            _ => unknown = true,
        }
    }
    if !seen || unknown {
        return HEALTH_UNKNOWN;
    }
    HEALTH_HEALTHY
}

/// What ONE health-check probe needs, taken out of the store under its lock.
///
/// Everything the probe uses is COPIED here, so the run itself holds no guard of this store
/// and takes it only to record the outcome it produced.
#[derive(Debug)]
pub struct HealthProbeLaunch {
    /// The container this probe is of.
    container_arn: String,
    /// The program it runs ([`health_probe_program`]).
    program: ContainerProgram,
    /// The environment it runs with: the container's own, because the probe runs inside the
    /// container.
    environment: BTreeMap<String, String>,
    /// How long it may take before it is the failure `HealthCheck$timeout` documents.
    timeout: Duration,
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Start every container whose dependencies are now met, as of `now`.
    ///
    /// A container of a `RUNNING` task starts when the conditions its `dependsOn` names hold
    /// ([`container_dependencies_met`]): the task reaching `RUNNING` starts the ones that
    /// wait for nothing, and each later pass starts the ones whose dependencies have since
    /// been met. Starting a container is also what starts its health-check schedule, because
    /// the agent probes a container that is running.
    pub(super) fn start_due_containers(&mut self, now: SystemTime) {
        for task in self.tasks.values_mut() {
            if task.last_status != STATUS_RUNNING || task.desired_status != STATUS_RUNNING {
                continue;
            }
            // The readings every dependency is weighed against are the ones the containers
            // hold at the START of this pass, so one pass starts one level of the dependency
            // graph and the next starts the level above it — a container never counts as
            // started by the same pass that started the container it waits on.
            let reached: Vec<ContainerReading> =
                task.containers.iter().map(ContainerReading::of).collect();
            let mut changed = false;
            for container in &mut task.containers {
                if container.last_status == STATUS_PENDING
                    && container_dependencies_met(&container.depends_on, &reached)
                {
                    container.advance_to(STATUS_RUNNING);
                    changed = true;
                }
                // The agent probes a container that is RUNNING, however it got there — the
                // gate above, or the agent's own report of it
                // (`SubmitContainerStateChange`) — so the schedule is started off the
                // container's STATUS rather than off the transition that wrote it.
                // Idempotent: a container already being probed keeps its schedule.
                if container.last_status == STATUS_RUNNING {
                    if let Some(health) = container.health.as_mut() {
                        health.begin(now);
                    }
                }
            }
            if changed {
                // "Every time a task experiences a change ... the version counter is
                // incremented."
                task.version += 1;
            }
        }
    }

    /// The health-check probes whose instant has come, as of `now`, each marked in flight so
    /// one container is probed once at a time.
    ///
    /// Only a RUNNING container of a RUNNING task is probed: the agent probes the container
    /// it is running, and a container that has not started — or whose task is on its way out
    /// — has nothing to answer a probe. A probe this pass does not run — because this
    /// emulator is already running as many as it holds at once — has its instant moved on
    /// anyway, so no container is ever left with an elapsed instant the clock would be woken
    /// by over and over.
    ///
    /// The pass also FORGETS the probes of containers that are no longer being probed,
    /// aborting each: a probe whose container has stopped reports on nothing, and leaving it
    /// to finish would hold one of [`MAX_HEALTH_PROBES_IN_FLIGHT`] slots for its whole span.
    pub(super) fn settle_container_health_probes(
        &mut self,
        now: SystemTime,
    ) -> Vec<HealthProbeLaunch> {
        self.forget_abandoned_health_probes();
        let mut in_flight = self.health_probes_in_flight();
        let mut due: Vec<(String, ContainerProgram, BTreeMap<String, String>, Duration)> =
            Vec::new();
        // Every container whose instant this pass CONSUMED, launched or not: each has its
        // next execution scheduled below, which is what keeps the clock's wait in the future.
        let mut consumed: Vec<String> = Vec::new();
        for task in self.tasks.values() {
            if task.last_status != STATUS_RUNNING || task.desired_status != STATUS_RUNNING {
                continue;
            }
            let definition = parse_task_definition_reference(&task.task_definition_arn)
                .and_then(|(family, revision)| {
                    revision.and_then(|revision| self.task_definitions.get(&(family, revision)))
                });
            for container in &task.containers {
                if container.last_status != STATUS_RUNNING {
                    continue;
                }
                let Some(health) = container.health.as_ref() else {
                    continue;
                };
                if !health.probe_due(now) {
                    continue;
                }
                consumed.push(container.container_arn.clone());
                if in_flight >= MAX_HEALTH_PROBES_IN_FLIGHT {
                    // The ceiling is this emulator's, so it is TRACED rather than passed
                    // over in silence: a probe that is not run produces no result, and the
                    // reader of that container cannot otherwise tell that from a probe
                    // that has not come due. It is owed again one interval on, like any
                    // other execution.
                    ::tracing::warn!(
                        container_arn = %container.container_arn,
                        limit = MAX_HEALTH_PROBES_IN_FLIGHT,
                        "this emulator is already running as many container health checks \
                         as it holds at once, so this one is not run now"
                    );
                    continue;
                }
                // A check that names no program is not stored at all
                // ([`ContainerHealthRecord::from_definition`]), so a stored one always
                // resolves; a record that somehow does not is rescheduled like the rest
                // rather than left with an elapsed instant.
                let Some(program) = health_probe_program(&health.command) else {
                    continue;
                };
                // The probe runs INSIDE the container, so it sees the container's own
                // environment (`KeyValuePair` entries of the definition it was minted from).
                let environment = definition
                    .and_then(|definition| {
                        definition.container_definitions.iter().find(|candidate| {
                            candidate.name.is_some() && candidate.name == container.name
                        })
                    })
                    .map(|stored| health_probe_environment(stored.environment.as_deref()))
                    .unwrap_or_default();
                in_flight += 1;
                due.push((
                    container.container_arn.clone(),
                    program,
                    environment,
                    health.timeout,
                ));
            }
        }
        let launched: ::std::collections::HashSet<String> =
            due.iter().map(|(arn, ..)| arn.clone()).collect();
        for container_arn in consumed {
            let taken = launched.contains(&container_arn);
            if let Some(health) = self.container_health_mut(&container_arn) {
                health.probe_in_flight = taken;
                health.next_probe_due_at = Some(if taken {
                    // A probe that is RUNNING holds an instant past its own deadline: the
                    // execution ends by then ([`run_health_probe`] is bounded by `timeout`)
                    // and re-arms the schedule one `interval` from where it ended
                    // ([`ContainerHealthRecord::record`]), so the clock has a future instant
                    // to wait for throughout the probe rather than an elapsed one to wake on.
                    now + health.interval.max(health.timeout)
                } else {
                    // One this pass did not run is owed again one interval on, like any
                    // other execution.
                    now + health.interval
                });
            }
        }
        due.into_iter()
            .map(
                |(container_arn, program, environment, timeout)| HealthProbeLaunch {
                    container_arn,
                    program,
                    environment,
                    timeout,
                },
            )
            .collect()
    }

    /// Abort and forget the probe of every container that is no longer being probed: one
    /// whose task is gone, whose task is no longer running, or that has stopped.
    ///
    /// The same reconcile the container RUNS get ([`super::container_runtime`]): the probe
    /// reports on a container, so one whose container is gone has nothing to report and is
    /// stopped rather than left to finish. The record's in-flight flag goes with it, so the
    /// slot it held is released.
    fn forget_abandoned_health_probes(&mut self) {
        let probed: Vec<String> = self.health_probes.keys().cloned().collect();
        for container_arn in probed {
            let still_probed = self.tasks.values().any(|task| {
                task.last_status == STATUS_RUNNING
                    && task.desired_status == STATUS_RUNNING
                    && task.containers.iter().any(|container| {
                        container.container_arn == container_arn
                            && container.last_status == STATUS_RUNNING
                    })
            });
            if still_probed {
                continue;
            }
            if let Some(handle) = self.health_probes.remove(&container_arn) {
                handle.abort();
            }
            if let Some(health) = self.container_health_mut(&container_arn) {
                health.probe_in_flight = false;
            }
        }
    }

    /// Keep the in-process task performing one container's probe, so a probe whose container
    /// is gone is ABORTED rather than merely forgotten — a handle dropped on the floor
    /// cannot be stopped. A probe that has already ended by the time its handle arrives
    /// needs none.
    pub(super) fn attach_health_probe(
        &mut self,
        container_arn: &str,
        handle: ::tokio::task::JoinHandle<()>,
    ) {
        let in_flight = self
            .container_health_mut(container_arn)
            .is_some_and(|health| health.probe_in_flight);
        if in_flight {
            if let Some(previous) = self
                .health_probes
                .insert(container_arn.to_string(), handle)
            {
                previous.abort();
            }
        } else {
            handle.abort();
        }
    }

    /// Forget the probe of `container_arn`, which has just recorded its outcome: the
    /// in-process task is over, so there is nothing left to abort.
    fn finish_health_probe(&mut self, container_arn: &str) {
        self.health_probes.remove(container_arn);
    }

    /// How many health-check probes are in flight across this store, which is what
    /// [`MAX_HEALTH_PROBES_IN_FLIGHT`] bounds.
    fn health_probes_in_flight(&self) -> usize {
        self.tasks
            .values()
            .flat_map(|task| task.containers.iter())
            .filter(|container| {
                container
                    .health
                    .as_ref()
                    .is_some_and(|health| health.probe_in_flight)
            })
            .count()
    }

    /// The health record of ONE container of ONE task, for the writes that record a probe.
    fn container_health_mut(&mut self, container_arn: &str) -> Option<&mut ContainerHealthRecord> {
        self.tasks
            .values_mut()
            .flat_map(|task| task.containers.iter_mut())
            .find(|container| container.container_arn == container_arn)
            .and_then(|container| container.health.as_mut())
    }

    /// Record what ONE health-check execution exited as, and schedule the next one.
    ///
    /// The container's own `healthStatus` is what this writes — and the TASK's reading
    /// follows from it ([`TaskRecord::health_status`]) — so nothing here moves a task's
    /// status or its containers' lifecycle: a probe reports on the container it ran in.
    pub(super) fn record_health_probe_outcome(
        &mut self,
        container_arn: &str,
        passed: bool,
        now: SystemTime,
    ) {
        self.finish_health_probe(container_arn);
        for task in self.tasks.values_mut() {
            let Some(container) = task
                .containers
                .iter_mut()
                .find(|container| container.container_arn == container_arn)
            else {
                continue;
            };
            let Some(health) = container.health.as_mut() else {
                return;
            };
            if health.record(passed, now) {
                // "Every time a task experiences a change ... the version counter is
                // incremented" — a reading that did not move is not a change.
                task.version += 1;
            }
            return;
        }
    }

    /// The earliest instant at which this store owes a health-check EXECUTION, if any — one
    /// of the instants the clock pump waits for.
    pub(super) fn next_health_probe_due_at(&self) -> Option<SystemTime> {
        self.tasks
            .values()
            .filter(|task| {
                task.last_status == STATUS_RUNNING && task.desired_status == STATUS_RUNNING
            })
            .flat_map(|task| task.containers.iter())
            .filter(|container| container.last_status == STATUS_RUNNING)
            .filter_map(|container| container.health.as_ref()?.next_probe_due_at)
            .min()
    }
}

/// The environment a container's `KeyValuePair` list describes, as the variables the probe's
/// session holds. A pair carrying no name names no variable, so it sets none.
fn health_probe_environment(pairs: Option<&[KeyValuePair]>) -> BTreeMap<String, String> {
    pairs
        .unwrap_or_default()
        .iter()
        .filter_map(|pair| {
            let name = pair.name.clone()?;
            Some((name, pair.value.clone().unwrap_or_default()))
        })
        .collect()
}

/// What one container of a task has REACHED, as a dependency of another container reads it.
#[derive(Debug, Clone)]
pub struct ContainerReading {
    /// The container's name, which is what a `ContainerDependency$containerName` names.
    pub name: Option<String>,
    /// Its `lastStatus`.
    pub last_status: String,
    /// Its `exitCode`, and `None` for a container that has not exited.
    pub exit_code: Option<i32>,
    /// Its `healthStatus`.
    pub health_status: String,
}

impl ContainerReading {
    /// The reading a stored container offers.
    pub fn of(container: &ContainerRecord) -> Self {
        Self {
            name: container.name.clone(),
            last_status: container.last_status.clone(),
            exit_code: container.exit_code,
            health_status: container.health_status().to_string(),
        }
    }

    /// Whether this container has STARTED: the `START` condition "validates that a dependent
    /// container is started", which is any stage of the container lifecycle at or past
    /// `RUNNING`.
    fn has_started(&self) -> bool {
        container_stage_rank(&self.last_status)
            .zip(container_stage_rank(STATUS_RUNNING))
            .is_some_and(|(reached, running)| reached >= running)
    }

    /// Whether this container has EXITED: the `COMPLETE` condition "validates that a
    /// dependent container runs to completion (exits)".
    fn has_exited(&self) -> bool {
        self.last_status == STATUS_STOPPED
    }

    /// Whether one dependency condition on this container holds.
    fn satisfies(&self, condition: &str) -> bool {
        match condition {
            CONTAINER_CONDITION_START => self.has_started(),
            CONTAINER_CONDITION_COMPLETE => self.has_exited(),
            // "the same as COMPLETE, but it also requires that the container exits with a
            // zero status".
            CONTAINER_CONDITION_SUCCESS => self.has_exited() && self.exit_code == Some(0),
            CONTAINER_CONDITION_HEALTHY => self.health_status == HEALTH_HEALTHY,
            // A condition outside the modelled enum cannot reach a handler, so it names no
            // condition this gate can hold open.
            _ => false,
        }
    }
}

/// `ContainerDependency$condition` `START`, from the `ContainerCondition` enum
/// (`smithy_query(query="shape",shape="ContainerCondition")` -> START, COMPLETE, SUCCESS,
/// HEALTHY).
pub const CONTAINER_CONDITION_START: &str = "START";
/// See [`CONTAINER_CONDITION_START`]: the dependent container has run to completion.
pub const CONTAINER_CONDITION_COMPLETE: &str = "COMPLETE";
/// See [`CONTAINER_CONDITION_START`]: it completed with a zero status.
pub const CONTAINER_CONDITION_SUCCESS: &str = "SUCCESS";
/// See [`CONTAINER_CONDITION_START`]: it passed its health check.
pub const CONTAINER_CONDITION_HEALTHY: &str = "HEALTHY";

/// Whether every dependency in `depends_on` is met by the containers `reached` describes.
///
/// A dependency naming a container the task does not hold is NOT met: there is nothing to
/// wait on, and permitting the start would be permitting it on no evidence. A container that
/// declares no dependency at all waits for nothing.
pub fn container_dependencies_met(
    depends_on: &[ContainerDependency],
    reached: &[ContainerReading],
) -> bool {
    depends_on.iter().all(|dependency| {
        reached
            .iter()
            .find(|candidate| candidate.name.as_deref() == Some(dependency.container_name.as_str()))
            .is_some_and(|candidate| candidate.satisfies(&dependency.condition))
    })
}

/// Run the health-check probes this store owes, and record what each of them exited with.
///
/// Call it with NO guard of this store held, from a door that has just written a task's
/// status or from the clock pump: it takes the lock itself, and the probes it starts are
/// in-process tasks that outlive the call. With no async runtime to start them on there is
/// no probe at all — nothing is reported healthy or unhealthy that was not evaluated.
pub fn settle_container_health<Eng: Send + Sync + 'static>(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    engine: &::std::sync::Arc<Eng>,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let probes = state
        .lock()
        .settle_container_health_probes(SystemTime::now());
    for probe in probes {
        let container_arn = probe.container_arn.clone();
        let probe_state = ::engine::ServiceState::clone(&state);
        let callback = callback.clone();
        let handle = runtime.spawn(async move { run_health_probe(probe_state, callback, probe).await });
        // The handle is kept with the container's health record, so a probe whose container
        // is gone is aborted rather than merely forgotten.
        state.lock().attach_health_probe(&container_arn, handle);
    }
}

/// Run ONE health-check probe to its end and record the status it exited with.
///
/// The probe's own output goes nowhere: `HealthCheck` describes a command whose EXIT CODE is
/// the result ("An exit code of 0 indicates success"), and the `awslogs` driver delivers the
/// container's output rather than its agent's probes.
///
/// The run is bounded TWICE by the same `timeout` the member documents — the session's own
/// deadline, and a wall clock around the whole of it — so this in-process task always ends,
/// and ends inside the span the definition asked for, whatever the program it was given does.
async fn run_health_probe(
    state: ::engine::ServiceState<ComAmazonawsEcsAmazonec2containerservicev20141113State>,
    callback: ::smithy_server_runtime::EmbeddedEmulator,
    probe: HealthProbeLaunch,
) {
    let output = Arc::new(ContainerOutput::new(&callback, None));
    // The probe answers no signal: `StopTask` stops the container, and the probe is bounded
    // by the `timeout` its own member documents.
    let signals = Arc::new(ContainerSignals::new());
    let deadline = Instant::now() + probe.timeout;
    let mut shell = ContainerShell::new(probe.environment, output, signals, deadline);
    let program = probe.program;
    let outcome = ::tokio::time::timeout(probe.timeout, async move { shell.run(&program).await })
        .await
        // "The time period in seconds to wait for a health check to succeed before it is
        // considered a failure": a probe the timeout ran out on produced no status, which is
        // the outcome an abandoned run reports.
        .unwrap_or(ContainerOutcome::Abandoned);
    // "An exit code of 0 indicates success, and non-zero exit code indicates failure" — and
    // a probe the timeout ran out on "is considered a failure", which is what an abandoned
    // run is here.
    let passed = outcome == ContainerOutcome::Exited(0);
    state
        .lock()
        .record_health_probe_outcome(&probe.container_arn, passed, SystemTime::now());
}
