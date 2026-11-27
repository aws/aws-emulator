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
//!   `command` runs in ([`ContainerShell`]), over the container's OWN filesystem
//!   ([`super::container_filesystem::ContainerLayer`]) and reaching the servers of its own
//!   task ([`super::container_http_client`]), because the probe runs INSIDE the container —
//!   once the container is running, and again one `interval` after each execution;
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
//! What the SERVICE SCHEDULER makes of those results is
//! [`super::unhealthy_task_replacement`]'s: a task of a service whose essential container
//! reports `UNHEALTHY` is one that scheduler stops and replaces, which is the very thing
//! `CreateService$healthCheckGracePeriodSeconds` names a period for ("the period of time ...
//! that the Amazon ECS service scheduler ignores unhealthy ... container health checks after
//! a task has first started"). Nothing in THIS module moves a task or a container: a probe
//! reports on the container it ran in.
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
//! # The containers that GIVE UP
//!
//! A wait that cannot end is not waited out for ever. `ContainerDefinition$startTimeout` is
//! the rule: "time duration (in seconds) to wait before giving up on resolving dependencies
//! for a container. For example, you specify two containers in a task definition with
//! containerA having a dependency on containerB reaching a `COMPLETE`, `SUCCESS`, or
//! `HEALTHY` status. If a `startTimeout` value is specified for containerB and it doesn't
//! reach the desired status within that time then containerA gives up and not start. This
//! results in the task transitioning to a `STOPPED` state."
//!
//! So a waiting container gives up when the condition it waits for can no longer be reached —
//! at once when the container it waits on has reached a state that condition EXCLUDES (one
//! that exited non-zero can never satisfy `SUCCESS`, and one that has exited can never pass a
//! health check for `HEALTHY`), and otherwise at the end of the `startTimeout` that container
//! declared ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::give_up_on_unresolved_dependencies`]).
//! The container never starts, so the task never ran the work it was placed for: it is
//! stopped as a task whose launch could not be made ([`STOP_CODE_TASK_FAILED_TO_START`]) with
//! a `stoppedReason` naming the container that gave up and the dependency it gave up on —
//! the stop a task whose essential container EXITED earns states something else entirely
//! ([`STOP_CODE_ESSENTIAL_CONTAINER_EXITED`]), and a container that never started cannot have
//! exited.
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

/// How long after a container starts running its FIRST health-check execution falls due.
///
/// EMULATOR-CHOSEN, and deliberately NOT the `interval`: that member is "the time period in
/// seconds between each health check execution", which is the spacing between two
/// executions of a check and says nothing about the one that has no predecessor. The agent
/// "monitors and reports on the health checks specified in the task definition" from the
/// moment the container it probes is running, and the reading until it has is the one
/// `UNKNOWN` states ("The container health check is being evaluated"), so the move out of
/// that reading is a transition of this emulator's own: it takes the same span as this
/// emulator's other transient steps, short enough that a client that starts a container and
/// then polls it learns the verdict rather than watching `UNKNOWN` for the interval the
/// definition happens to have chosen. Every execution AFTER it is one `interval` on, which
/// is the spacing the member does document ([`ContainerHealthRecord::record`]), and this
/// span is never longer than that spacing: the interval a record holds is clamped to at
/// least [`MIN_HEALTH_CHECK_INTERVAL`], the smallest the member admits.
pub const FIRST_HEALTH_PROBE_SPAN: Duration = Duration::from_secs(3);

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
    /// WHEN that reading became [`HEALTH_UNHEALTHY`] — the instant the execution that
    /// exhausted the retries ran at — and `None` whenever the reading is anything else, so
    /// the two cannot disagree.
    ///
    /// What the scheduler's own reaction to a failed check is dated by
    /// ([`super::unhealthy_task_replacement`]): the reading was reached at that instant
    /// whether or not anybody called, so the stop it earns is dated there too rather than at
    /// whichever pass noticed.
    pub unhealthy_at: Option<SystemTime>,
    /// How many executions have failed since the last one that succeeded.
    pub failures: i32,
    /// Whether any execution has succeeded yet, which is what ends the start period's
    /// suppression early ("If a health check succeeds within the `startPeriod`, then the
    /// container is considered healthy and any subsequent failures count toward the maximum
    /// number of retries").
    pub succeeded: bool,
    /// WHEN the FIRST execution succeeded — the instant this container reached `HEALTHY` —
    /// and `None` for one whose check has never passed.
    ///
    /// Kept for good once it is set, where [`Self::unhealthy_at`] holds only the reading the
    /// container reports NOW: this is the instant the container is known to have been
    /// healthy at, which is what the SCHEDULER dates its own reaction to a passing check by
    /// ([`super::circuit_breaker`], where `DeploymentCircuitBreaker$resetOnHealthyTask`
    /// resets a rollout's failure count from the instant a task of it "reaches a healthy
    /// state"). A later failing execution leaves it where it is — the container WAS healthy
    /// then — and what the rollout makes of a task it went on to LOSE is that rollout's
    /// question rather than this reading's ([`TaskRecord::reached_healthy_state_at`]).
    pub healthy_at: Option<SystemTime>,
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
            unhealthy_at: None,
            failures: 0,
            succeeded: false,
            healthy_at: None,
            start_period_ends_at: None,
            next_probe_due_at: None,
            probe_in_flight: false,
        })
    }

    /// Start this check's schedule from `now` — the instant the container started running,
    /// which is when the agent begins probing it and when the grace period the definition
    /// asked for begins.
    ///
    /// The first execution falls due one [`FIRST_HEALTH_PROBE_SPAN`] on — the `interval` is
    /// "the time period in seconds between each health check execution", so it spaces the
    /// executions that FOLLOW one rather than holding off the one that begins the schedule —
    /// and the container reports `UNKNOWN` until that execution has produced a result.
    /// Idempotent: a container whose schedule has already started keeps it.
    pub fn begin(&mut self, now: SystemTime) {
        if self.next_probe_due_at.is_some() || self.probe_in_flight {
            return;
        }
        self.start_period_ends_at = self.start_period.map(|period| now + period);
        self.next_probe_due_at = Some(now + FIRST_HEALTH_PROBE_SPAN);
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
            // Dated at the FIRST execution that passed: that is the instant the container
            // reached a healthy state, and a later execution does not move it.
            self.healthy_at = self.healthy_at.or(Some(now));
            self.status = HEALTH_HEALTHY.to_string();
            // A check that passes clears the failures it counted, so the reading is no
            // longer a failed one and nothing is owed for it.
            self.unhealthy_at = None;
        } else if self.suppresses_failure(now) {
            // The grace period: a failure here "count[s] towards" nothing, so the container
            // is still being evaluated rather than unhealthy.
            self.status = HEALTH_UNKNOWN.to_string();
            self.unhealthy_at = None;
        } else {
            self.failures = self.failures.saturating_add(1);
            if self.failures >= self.retries {
                self.status = HEALTH_UNHEALTHY.to_string();
                // Dated at the FIRST execution that made the reading a failed one: a
                // container that keeps failing has been unhealthy since then, and the
                // instant does not move with every further execution.
                self.unhealthy_at = self.unhealthy_at.or(Some(now));
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
            (!text.trim().is_empty()).then(|| ContainerProgram::shell_script(text))
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
    /// The volumes mounted into the container, for the same reason the environment is the
    /// container's: the probe runs INSIDE the container, so the paths its command names are
    /// the container's own ([`super::container_volumes`]).
    mounts: Vec<MountRequest>,
    /// `ContainerDefinition$workingDirectory`, which the probe's relative paths are resolved
    /// against — the container's own, for the same reason.
    working_directory: Option<String>,
    /// What the container's process runs as, for that same reason: the probe runs INSIDE the
    /// container, so the user, the resource limits and the storage it reports are the
    /// container's own ([`super::container_process`]).
    process: ContainerProcess,
    /// The container's OWN writable layer, for that same reason: the probe runs INSIDE the
    /// container, so the files its command names are the ones the container's own program
    /// left there ([`super::container_filesystem::ContainerLayer`]).
    layer: ContainerLayer,
    /// The private address of the interface the TASK was allocated, for that same reason: the
    /// probe runs inside the container, so the server it asks for a document is one of its own
    /// task ([`super::container_http_client`]). `None` for a task this store holds no address
    /// for, whose containers reach no server of their own.
    address: Option<String>,
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

    /// Stop every task holding a container that has GIVEN UP on a dependency, as of `now`.
    ///
    /// `ContainerDefinition$startTimeout` is the rule: a container waiting on another "gives
    /// up and not start" once the condition it waits for can no longer be reached, and "this
    /// results in the task transitioning to a `STOPPED` state". The container never started,
    /// so the task never ran: it takes the stop a launch that could not be made earns
    /// ([`TaskRecord::fail_launch`], [`STOP_CODE_TASK_FAILED_TO_START`]) with the reason that
    /// names the container and the dependency it gave up on
    /// ([`dependency_given_up_reason`]) — never the stop an essential container's EXIT earns,
    /// which a container that never ran cannot have produced.
    ///
    /// Read in one pass and applied in another, because the `startTimeout` the wait is
    /// measured against lives on the task definition's own container definition rather than
    /// on the task.
    pub(super) fn give_up_on_unresolved_dependencies(&mut self, now: SystemTime) {
        let given_up: Vec<(String, UnresolvedDependency)> = self
            .tasks
            .values()
            .filter_map(|task| {
                let unresolved = self.unresolved_dependency(task, now)?;
                (unresolved.at <= now).then(|| (task.task_arn.clone(), unresolved))
            })
            .collect();
        for (task_arn, unresolved) in given_up {
            let reason = dependency_given_up_reason(
                &unresolved.dependent,
                &unresolved.dependency,
                &unresolved.condition,
            );
            let Some(task) = self.tasks.get_mut(&task_arn) else {
                continue;
            };
            // The failure belongs to the container that gave up, which is where the task
            // records the detail of a launch it could not make
            // ([`TaskRecord::launch_failure_container`]).
            task.launch_failure_container = Some(unresolved.dependent.clone());
            task.fail_launch(&reason, unresolved.at);
        }
    }

    /// The earliest instant at which a container of this store GIVES UP on a dependency, if
    /// any — one of the instants the clock pump waits for, because the stop it earns is owed
    /// whether or not anybody calls again.
    pub(super) fn next_dependency_give_up_due_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.tasks
            .values()
            .filter_map(|task| self.unresolved_dependency(task, now))
            .map(|unresolved| unresolved.at)
            .min()
    }

    /// The dependency ONE task has given up on, or will give up on, and when — and `None`
    /// for a task every waiting container of which can still have what it waits for.
    ///
    /// Only a task that is RUNNING and wanted running has containers waiting at all: one on
    /// its way out is being stopped for the reason that put it there, and one that has not
    /// started has not begun creating its containers.
    fn unresolved_dependency(
        &self,
        task: &TaskRecord,
        now: SystemTime,
    ) -> Option<UnresolvedDependency> {
        if task.last_status != STATUS_RUNNING || task.desired_status != STATUS_RUNNING {
            return None;
        }
        let reached: Vec<ContainerReading> =
            task.containers.iter().map(ContainerReading::of).collect();
        let definition = self.revision_of_arn(&task.task_definition_arn);
        task.containers
            .iter()
            .filter(|container| container.last_status == STATUS_PENDING)
            .flat_map(|container| {
                container
                    .depends_on
                    .iter()
                    .map(move |dependency| (container, dependency))
            })
            .filter_map(|(container, dependency)| {
                let reading = reached.iter().find(|candidate| {
                    candidate.name.as_deref() == Some(dependency.container_name.as_str())
                })?;
                if reading.satisfies(&dependency.condition) {
                    return None;
                }
                // The wait is over at once where the state the dependency has REACHED
                // excludes the condition, and otherwise at the end of the `startTimeout`
                // that dependency declared — measured from the instant Amazon ECS began
                // creating the task's containers, which is the task's own start.
                let at = if reading.cannot_satisfy(&dependency.condition) {
                    now
                } else {
                    let timeout = definition
                        .and_then(|definition| {
                            definition.container_definitions.iter().find(|candidate| {
                                candidate.name.as_deref()
                                    == Some(dependency.container_name.as_str())
                            })
                        })
                        .and_then(|candidate| candidate.start_timeout)
                        .filter(|seconds| *seconds > 0)
                        .and_then(|seconds| u64::try_from(seconds).ok())
                        .map(Duration::from_secs)?;
                    SystemTime::try_from(task.started_at?).ok()? + timeout
                };
                Some(UnresolvedDependency {
                    dependent: container.name.clone().unwrap_or_default(),
                    dependency: dependency.container_name.clone(),
                    condition: dependency.condition.clone(),
                    at,
                })
            })
            .min_by_key(|unresolved| unresolved.at)
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
        let mut due: Vec<HealthProbeLaunch> = Vec::new();
        // Every container whose instant this pass CONSUMED, launched or not: each has its
        // next execution scheduled below, which is what keeps the clock's wait in the future.
        let mut consumed: Vec<String> = Vec::new();
        for task in self.tasks.values() {
            if task.last_status != STATUS_RUNNING || task.desired_status != STATUS_RUNNING {
                continue;
            }
            let definition = self.revision_of_arn(&task.task_definition_arn);
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
                // environment (`KeyValuePair` entries of the definition it was minted from),
                // and the volumes that definition mounts into it.
                let stored = definition.and_then(|definition| {
                    definition.container_definitions.iter().find(|candidate| {
                        candidate.name.is_some() && candidate.name == container.name
                    })
                });
                // The probe runs inside the container, so it reads what that container is at
                // the same TASK METADATA ENDPOINT the container's own program reads it at
                // ([`super::task_metadata_endpoint`]) — put in place BEFORE the definition's
                // own `environment`, exactly as the container's own run and an exec session put
                // it there, so a revision that sets that name itself is what every session
                // inside the container sees.
                let mut environment: BTreeMap<String, String> = BTreeMap::new();
                environment.insert(
                    TASK_METADATA_URI_VARIABLE.to_string(),
                    container_metadata_uri(container),
                );
                environment.extend(
                    stored
                        .map(|stored| health_probe_environment(stored.environment.as_deref()))
                        .unwrap_or_default(),
                );
                let mounts = match (definition, stored) {
                    (Some(definition), Some(stored)) => mount_requests(definition, stored, task),
                    _ => Vec::new(),
                };
                in_flight += 1;
                due.push(HealthProbeLaunch {
                    container_arn: container.container_arn.clone(),
                    program,
                    environment,
                    timeout: health.timeout,
                    mounts,
                    working_directory: stored
                        .and_then(|stored| stored.working_directory.clone()),
                    process: ContainerProcess::of(stored, task),
                    // The container's own filesystem: the probe runs inside the container, so
                    // it reads the layer the container's program writes rather than one of its
                    // own. A container this store holds no run of has had nothing run in it,
                    // so its layer holds nothing either.
                    layer: self
                        .container_layer(&container.container_arn)
                        .unwrap_or_default(),
                    // The address the task's own interface holds, which is where its
                    // containers' servers are reached — the one a probe written as a request
                    // to `localhost` is answered by ([`super::container_http_client`]).
                    address: task
                        .attachments
                        .iter()
                        .find_map(AttachmentRecord::private_ipv4_address)
                        .map(str::to_string),
                });
            }
        }
        let launched: ::std::collections::HashSet<String> =
            due.iter().map(|probe| probe.container_arn.clone()).collect();
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
        due
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

    /// Whether one dependency condition on this container can NEVER hold again, because the
    /// state this container has reached EXCLUDES it.
    ///
    /// A container that has exited has run its course: it can no longer exit with the zero
    /// status `SUCCESS` requires ("it also requires that the container exits with a `zero`
    /// status"), and it can no longer pass the health check `HEALTHY` requires, because
    /// Amazon ECS probes a container that is running. `START` and `COMPLETE` are reachable
    /// from every state a container that has not exited is in — one that HAS exited satisfies
    /// both — so neither is ever excluded.
    fn cannot_satisfy(&self, condition: &str) -> bool {
        match condition {
            CONTAINER_CONDITION_SUCCESS | CONTAINER_CONDITION_HEALTHY => {
                self.has_exited() && !self.satisfies(condition)
            }
            _ => false,
        }
    }
}

/// What one container of a task is WAITING for, and when that wait is GIVEN UP on
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::give_up_on_unresolved_dependencies`]).
#[derive(Debug, Clone)]
struct UnresolvedDependency {
    /// `ContainerDefinition$name` of the container that is waiting.
    dependent: String,
    /// `ContainerDependency$containerName`: the container it waits on.
    dependency: String,
    /// `ContainerDependency$condition`: what it waits for that container to reach.
    condition: String,
    /// When the wait is given up on: the instant the dependency reached a state the condition
    /// excludes, or the end of the `startTimeout` that dependency declared.
    at: SystemTime,
}

/// `Task$stoppedReason` ("The reason that the task was stopped") of a task a container of
/// which gave up on a dependency.
///
/// The stop is [`STOP_CODE_TASK_FAILED_TO_START`], and this sentence states what
/// `ContainerDefinition$startTimeout` describes — "giving up on resolving dependencies for a
/// container" — with the two container names and the condition taken from the task definition
/// the task runs.
fn dependency_given_up_reason(dependent: &str, dependency: &str, condition: &str) -> String {
    format!(
        "Container {dependent} gave up on resolving its dependency on container {dependency} \
         reaching a {condition} status."
    )
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
    // The records the servers a probe asks for a document are published in, taken once for the
    // whole pass, exactly as a container's own run takes them
    // ([`super::container_runtime::settle_container_runtime`]).
    let addresses = ::service_common::served_http_addresses::ServedAddresses::of(&**engine);
    // And the account's own network, for the same reason: the probe runs INSIDE the container,
    // so it is on the container's network and reaches an address exactly as its program does
    // ([`super::task_reachability`]).
    let network = TaskNetwork::of(&**engine);
    for probe in probes {
        let container_arn = probe.container_arn.clone();
        let probe_state = ::engine::ServiceState::clone(&state);
        let callback = callback.clone();
        // Resolved with NO guard of this store held, exactly as a container's own run
        // resolves its mounts ([`super::container_runtime::settle_container_runtime`]).
        let filesystem = ContainerFilesystem::on_layer(
            probe.layer.clone(),
            probe.working_directory.as_deref(),
            mounted_volumes(&**engine, &state, &probe.mounts),
        );
        // The client the probe asks the server of its own task for a document with, for a task
        // reached at an address of its own ([`super::container_http_client`]) — and the task
        // metadata endpoint inside the container's own namespace, which a task this emulator
        // holds no address for has all the same ([`super::task_metadata_endpoint`]).
        let http = addresses.clone().map(|addresses| {
            let client = match probe.address.clone() {
                Some(address) => ContainerHttpClient::new(address, addresses),
                None => ContainerHttpClient::unaddressed(addresses),
            };
            client
                .on_network(network.clone())
                .with_task_metadata(TaskMetadataEndpoint::new(
                    ::engine::ServiceState::clone(&state),
                    probe.container_arn.clone(),
                ))
        });
        let handle = runtime
            .spawn(async move { run_health_probe(probe_state, callback, probe, filesystem, http).await });
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
    filesystem: ContainerFilesystem,
    http: Option<ContainerHttpClient>,
) {
    let output = Arc::new(ContainerOutput::new(&callback, None));
    // The probe answers no signal: `StopTask` stops the container, and the probe is bounded
    // by the `timeout` its own member documents.
    let signals = Arc::new(ContainerSignals::new());
    let deadline = Instant::now() + probe.timeout;
    // A probe starts no server of the container's: `HealthCheck$command` is the command the
    // container is PROBED with, and what listens in the container is what its own program
    // started ([`super::container_servers`]), so this session publishes nothing.
    let mut shell =
        ContainerShell::new(probe.environment, output, signals, deadline, filesystem, None)
            .with_process(probe.process.clone());
    // What it DOES reach is the server the container's own program started: the probe runs
    // inside the container, and the command a health check is written with is a request to it
    // ([`super::container_http_client`]).
    if let Some(http) = http {
        shell = shell.with_http_client(http);
    }
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
