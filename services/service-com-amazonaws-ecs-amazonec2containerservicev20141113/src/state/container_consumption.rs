// SPDX-License-Identifier: CC0-1.0

//! What ONE container's run CONSUMES, measured on the session that performs it.
//!
//! # The rule this module implements
//!
//! A container's resource consumption is a MEASUREMENT taken on the container while it runs —
//! the CPU its process is using and the memory it is occupying — and every utilization series
//! about a task or a service is that measurement over what the task definition gives the
//! container (the ECS developer guide's service utilization divides what the tasks of one
//! service use by what that service's task definition reserves). So a series of it can only
//! report what something really measured, and what this emulator can measure is what it
//! really performs: a container's program runs IN THIS PROCESS
//! ([`super::container_shell`]), on one in-process performer per container, over an in-memory
//! filesystem and an in-memory session store.
//!
//! Four kinds of quantity of that run are therefore observable, and this module is where the
//! session records them as it goes:
//!
//! * how long the container's program has spent EXECUTING — the time it held its performer,
//!   as against the time it spent waiting (a `sleep`, a wait for the stop signal, a server
//!   held in the foreground), which is time it used nothing in;
//! * how many BYTES the container is occupying — what its session holds (the variables it
//!   has set and the output line it is assembling) beside what its own writable layer holds
//!   ([`super::container_filesystem`]);
//! * how many bytes its program has READ from and WRITTEN to a filesystem — every read and
//!   write the container's own layer and the volumes mounted into it perform for it, counted
//!   where that filesystem performs them ([`super::container_filesystem`]);
//! * how many bytes it has RECEIVED from and SENT over a network — the requests its clients
//!   make to the servers of its task and the answers they get back
//!   ([`super::container_http_client`]), the requests its own servers answer
//!   ([`super::container_servers`]) and the objects its AWS CLI transfers
//!   ([`super::container_aws_cli`]).
//!
//! The last two are CUMULATIVE COUNTERS of bytes that really moved, so a series over them
//! reports the bytes moved over the interval it closes and reports ZERO for a container that
//! moved none — which is a measurement, and the one a container whose program neither reads a
//! file nor opens a connection takes.
//!
//! Both are the container's own, so they are held per RUN ([`super::container_runtime`]) and
//! shared with whatever samples them: nothing here publishes anything, and nothing here
//! depends on who called. A program that burns its performer reads as a high share of it and
//! one that sleeps as almost none, which is exactly what the utilization of a busy and of an
//! idle container is — and a container whose program this emulator is no longer performing
//! consumes nothing, which is what its series then reports.
//!
//! # What is NOT measured
//!
//! The CPU time of a process, the resident set of one, and the memory of a container's image
//! are all facts of a host and a container runtime this emulator does not have, so none of
//! them is reported: every number here is this process's own accounting of the work it
//! performed for the container. The health-check probes the agent runs in a container
//! ([`super::container_health`]) are the agent's session rather than the container's program,
//! and are accounted to neither.

use ::std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// How many bytes one mebibyte is, which is the unit a task definition states a memory
/// reservation in (`TaskDefinition$memory`, `ContainerDefinition$memory`).
const BYTES_PER_MIB: u64 = 1024 * 1024;

/// How many bytes one gibibyte is, which is the unit a task's ephemeral storage is sized in
/// (`TaskEphemeralStorage$sizeInGiB`, "the total amount, in GiB, of the ephemeral storage to
/// set for the task").
const BYTES_PER_GIB: u64 = 1024 * BYTES_PER_MIB;

/// What ONE container's run has consumed so far: written by the session performing it and read
/// by the collections that sample it, so both see one accounting of one container.
#[derive(Debug, Default)]
pub struct ContainerConsumption {
    /// How long the container's program has spent EXECUTING, in nanoseconds, cumulatively
    /// over the whole run.
    executing_nanos: AtomicU64,
    /// How many bytes the container's SESSION is holding right now — zero once that session
    /// is over, because a session that has ended holds nothing.
    held_bytes: AtomicU64,
    /// How many bytes its program has read from a filesystem, cumulatively over the run.
    storage_read_bytes: AtomicU64,
    /// How many it has written to one, under the same rule.
    storage_written_bytes: AtomicU64,
    /// How many bytes it has received over a network, cumulatively over the run.
    network_received_bytes: AtomicU64,
    /// How many it has sent over one, under the same rule.
    network_sent_bytes: AtomicU64,
}

impl ContainerConsumption {
    /// The accounting of a run that has not performed anything yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How long this container's program has spent executing, in nanoseconds.
    pub fn executing_nanos(&self) -> u64 {
        self.executing_nanos.load(Ordering::Acquire)
    }

    /// How many bytes this container's session is holding.
    pub fn held_bytes(&self) -> u64 {
        self.held_bytes.load(Ordering::Acquire)
    }

    /// How many bytes this container's program has read from a filesystem.
    pub fn storage_read_bytes(&self) -> u64 {
        self.storage_read_bytes.load(Ordering::Acquire)
    }

    /// How many it has written to one.
    pub fn storage_written_bytes(&self) -> u64 {
        self.storage_written_bytes.load(Ordering::Acquire)
    }

    /// How many bytes it has received over a network.
    pub fn network_received_bytes(&self) -> u64 {
        self.network_received_bytes.load(Ordering::Acquire)
    }

    /// How many it has sent over one.
    pub fn network_sent_bytes(&self) -> u64 {
        self.network_sent_bytes.load(Ordering::Acquire)
    }

    /// Account `span` to the time this container's program has spent executing.
    ///
    /// Saturating, so an accounting of an absurd span cannot wrap the total back to a small
    /// one: a measurement is never reported as less work than was done.
    pub fn record_execution(&self, span: Duration) {
        add_bytes(&self.executing_nanos, u64::try_from(span.as_nanos()).unwrap_or(u64::MAX));
    }

    /// Account `bytes` to what this container's program has READ from a filesystem.
    pub fn record_storage_read(&self, bytes: usize) {
        add_bytes(&self.storage_read_bytes, counted(bytes));
    }

    /// Account `bytes` to what it has WRITTEN to one.
    pub fn record_storage_write(&self, bytes: usize) {
        add_bytes(&self.storage_written_bytes, counted(bytes));
    }

    /// Account `bytes` to what it has RECEIVED over a network.
    pub fn record_network_received(&self, bytes: usize) {
        add_bytes(&self.network_received_bytes, counted(bytes));
    }

    /// Account `bytes` to what it has SENT over one.
    pub fn record_network_sent(&self, bytes: usize) {
        add_bytes(&self.network_sent_bytes, counted(bytes));
    }

    /// Record that the container's session is holding `bytes` right now.
    pub fn hold(&self, bytes: u64) {
        self.held_bytes.store(bytes, Ordering::Release);
    }

    /// Record that the session is OVER: it holds nothing more.
    ///
    /// What the container's filesystem holds is not released with it — that is the
    /// container's own layer and it outlives the session that wrote it
    /// ([`super::container_filesystem`]) — so the bytes a later sample reads for this
    /// container are the ones it really still holds.
    pub fn release(&self) {
        self.held_bytes.store(0, Ordering::Release);
    }
}

/// `bytes` as the number one of these counters carries.
fn counted(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

/// Add `bytes` to `counter`, saturating at its ceiling.
///
/// Saturating, so an accounting of an absurd quantity cannot wrap a total back to a small one:
/// a measurement is never reported as less than what really happened.
fn add_bytes(counter: &AtomicU64, bytes: u64) {
    let mut held = counter.load(Ordering::Acquire);
    while let Err(current) = counter.compare_exchange_weak(
        held,
        held.saturating_add(bytes),
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        held = current;
    }
}

/// ONE reading of what the containers of one service's RUNNING tasks consume.
///
/// Taken off the runs this store holds, so every number in it is an accounting of work this
/// emulator really performed for those containers.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ServiceConsumption {
    /// How long the programs of those containers have spent executing, in nanoseconds,
    /// cumulatively since each run began.
    pub(super) executing_nanos: u128,
    /// How many of those containers this emulator is performing a program for — the
    /// performers whose time the span above is a share of.
    pub(super) performers: u64,
    /// How many RUNNING tasks this reading covers, whether or not anything is being
    /// performed for their containers — what tells a selection there is something to measure
    /// in from one that holds no task at all ([`super::container_usage`]).
    pub(super) tasks: u64,
    /// How many bytes those containers are occupying at this instant: their sessions' own
    /// stores and their writable layers.
    pub(super) held_bytes: u128,
    /// How many bytes of memory the task definitions of those tasks RESERVE for them, and
    /// zero when none of them states a reservation at all — the denominator a memory
    /// utilization is a percentage of.
    pub(super) reserved_bytes: u128,
    /// How many CPU UNITS the task definitions of those tasks reserve for them, and zero when
    /// none of them states a reservation at all — what the share of its performers a program
    /// held stands for as a quantity of CPU ([`super::container_usage`]).
    pub(super) reserved_cpu_units: u128,
    /// How many bytes those containers' own writable LAYERS hold at this instant: the files
    /// their programs wrote, which is the storage of the task they occupy
    /// ([`super::container_filesystem`]).
    pub(super) layer_bytes: u128,
    /// How many bytes of EPHEMERAL STORAGE the placements of those tasks gave them, and zero
    /// for tasks whose placement gave them none to report — the storage a task's containers
    /// write on ([`super::resolved_ephemeral_storage_gib`]).
    pub(super) reserved_storage_bytes: u128,
    /// How many bytes those containers' programs have READ from a filesystem, cumulatively
    /// since each run began.
    pub(super) storage_read_bytes: u128,
    /// How many they have WRITTEN to one, under the same rule.
    pub(super) storage_written_bytes: u128,
    /// How many bytes they have RECEIVED over a network, under the same rule.
    pub(super) network_received_bytes: u128,
    /// How many they have SENT over one, under the same rule.
    pub(super) network_sent_bytes: u128,
}

impl ServiceConsumption {
    /// The share of ITS PERFORMERS the service's containers used over the `span` this reading
    /// closes, as a percentage, given what `previous` read at the start of that span.
    ///
    /// The capacity this emulator gives one container's program is the one in-process
    /// performer that runs it, so the CPU a container uses over the CPU it was given is the
    /// share of the span its program spent executing — zero for a service whose containers
    /// are all waiting, and the whole of it for one whose program never stops working. A span
    /// of no length, and a service nothing is performing a program for, have no share to
    /// report and read as zero rather than as a quotient of nothing.
    pub(super) fn executing_share(&self, previous: u128, span: Duration) -> f64 {
        let executed = self.executing_nanos.saturating_sub(previous);
        let given = span.as_nanos().saturating_mul(u128::from(self.performers));
        percentage(executed, given)
    }

    /// The share of what the task definitions RESERVE that the service's containers are
    /// occupying, as a percentage — and `None` for a service whose task definitions reserve
    /// no memory at all, which gives the percentage no denominator, so there is no
    /// utilization of it to report.
    pub(super) fn memory_share(&self) -> Option<f64> {
        (self.reserved_bytes > 0).then(|| percentage(self.held_bytes, self.reserved_bytes))
    }
}

/// `part` as a percentage of `whole`, inside the range a utilization carries.
///
/// A utilization is a share of what was given, so it is never negative and never past 100:
/// the accounting above is taken on a running process while that process keeps working, so a
/// reading can land a little over the span it is measured against, and what that says is that
/// the performer was busy for the whole of it.
fn percentage(part: u128, whole: u128) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    let share = 100.0 * (part as f64) / (whole as f64);
    if share.is_finite() {
        share.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// What the containers of the RUNNING tasks of one service consume, read off the runs this
    /// store holds for them.
    ///
    /// The tasks are the ones a client reads the service's `runningCount` off
    /// ([`Self::service_tally`]), so what is measured is exactly what is reported as running.
    pub(super) fn service_consumption(
        &self,
        cluster_name: &str,
        service_name: &str,
    ) -> ServiceConsumption {
        self.consumption_of(|task| {
            task.cluster_name == cluster_name && task.service_name.as_deref() == Some(service_name)
        })
    }

    /// What the containers of the RUNNING tasks `selected` picks consume, read off the runs
    /// this store holds for them — the one walk every reading of a container's consumption
    /// takes, whether the tasks selected are one service's ([`Self::service_consumption`]) or
    /// a whole cluster's ([`super::container_usage`]).
    pub(super) fn consumption_of(
        &self,
        selected: impl Fn(&TaskRecord) -> bool,
    ) -> ServiceConsumption {
        let mut reading = ServiceConsumption::default();
        for task in self.tasks.values().filter(|task| {
            task.desired_status == STATUS_RUNNING
                && task.last_status == STATUS_RUNNING
                && selected(task)
        }) {
            reading.tasks = reading.tasks.saturating_add(1);
            // What this task RESERVES of its host, read through the one reader of that
            // quantity ([`super::TaskReservation::of_task`]) — the same reading the placement
            // that put the task there was held to, so what a cluster reports as reserved and
            // what its container instances report as taken cannot disagree.
            let reserved = TaskReservation::of_task(task);
            reading.reserved_bytes = reading
                .reserved_bytes
                .saturating_add(u128::from(mebibytes_as_bytes(reserved.memory)));
            reading.reserved_cpu_units = reading
                .reserved_cpu_units
                .saturating_add(u128::from(u64::try_from(reserved.cpu).unwrap_or_default()));
            // The ephemeral storage this task's placement gave it, which is the storage its
            // containers write on ([`super::container_process`]) — and nothing at all for a
            // placement that gave it none to report.
            if let Some(gib) = task.ephemeral_storage_gib.filter(|gib| *gib > 0) {
                reading.reserved_storage_bytes = reading.reserved_storage_bytes.saturating_add(
                    u128::from(u64::try_from(gib).unwrap_or_default())
                        .saturating_mul(u128::from(BYTES_PER_GIB)),
                );
            }
            for container in &task.containers {
                let Some(run) = self.container_runs.get(&container.container_arn) else {
                    continue;
                };
                let consumption = run.consumption();
                reading.executing_nanos = reading
                    .executing_nanos
                    .saturating_add(u128::from(consumption.executing_nanos()));
                reading.held_bytes = reading.held_bytes.saturating_add(u128::from(
                    consumption.held_bytes().saturating_add(run.layer_bytes()),
                ));
                reading.layer_bytes = reading
                    .layer_bytes
                    .saturating_add(u128::from(run.layer_bytes()));
                reading.storage_read_bytes = reading
                    .storage_read_bytes
                    .saturating_add(u128::from(consumption.storage_read_bytes()));
                reading.storage_written_bytes = reading
                    .storage_written_bytes
                    .saturating_add(u128::from(consumption.storage_written_bytes()));
                reading.network_received_bytes = reading
                    .network_received_bytes
                    .saturating_add(u128::from(consumption.network_received_bytes()));
                reading.network_sent_bytes = reading
                    .network_sent_bytes
                    .saturating_add(u128::from(consumption.network_sent_bytes()));
                if run.is_performing_a_program() {
                    reading.performers = reading.performers.saturating_add(1);
                }
            }
        }
        reading
    }
}

/// `mib` mebibytes as a number of bytes.
fn mebibytes_as_bytes(mib: i64) -> u64 {
    u64::try_from(mib)
        .unwrap_or_default()
        .saturating_mul(BYTES_PER_MIB)
}

