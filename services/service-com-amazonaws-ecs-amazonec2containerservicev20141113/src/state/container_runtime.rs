// SPDX-License-Identifier: CC0-1.0

//! The container DATA PLANE: the runs behind a task's containers.
//!
//! # The rule this module implements
//!
//! A task that reaches `RUNNING` is a task whose containers are running: the program each
//! container's image was given runs, what it writes goes to the log driver its definition
//! configures, and the status it ends with is the container's `exitCode` ("the exit code
//! returned from the container"). `StopTask` is what ends such a run — "the equivalent of
//! docker stop is issued to the containers running in the task. This results in a stop
//! signal value ... will default to SIGTERM. If the container handles the SIGTERM value
//! gracefully and exits within 30 seconds from receiving it, no SIGKILL value is sent" —
//! so a container that handles the signal exits with the status its own handler chose, and
//! one that does not is ended by the signal.
//!
//! The program itself runs IN THIS PROCESS, in the session [`ContainerShell`] describes:
//! there is no container image to run and nothing is started outside this process. A
//! container whose launch names no argument vector at all runs nothing — the image's own
//! entry point is what would run, and this emulator holds no image, so no output and no
//! exit code is invented for it. The same holds for a launch whose vector needs a program,
//! a command form or an expansion that session does not perform: the image holds that
//! program, so the run produced no status of the container's own and none is reported
//! ([`ContainerOutcome::Abandoned`]) — which is also why such a task is not stopped as one
//! whose essential container exited.
//!
//! # The log driver is initialised whatever the container then runs
//!
//! Starting a container also INITIALISES its log driver, and that is independent of the
//! program: the `awslogs` driver "sends container logs to Amazon CloudWatch Logs" and is
//! given its arguments when the container is created, so the stream the output would go to
//! is created as the container starts ([`super::log_driver_init`] is the gate on exactly
//! that call, and [`ContainerOutput::open`] is the call). A container of a started task
//! whose `logConfiguration` describes an `awslogs` delivery therefore has its stream in the
//! group that configuration names, whether or not this emulator holds a program for its
//! image — the stream is the driver's, while the EVENTS in it are the container's own
//! output and are only ever what something really wrote.
//!
//! What the program SEES is its definition's environment with the launch's own overrides
//! applied, and then the values the agent fetched for it — a container definition's
//! `secrets` ([`super::container_secrets`]), read before the program starts, as the task's
//! execution role. A container whose secrets cannot be read starts no program at all, so it
//! reports no exit code: nothing ran to produce one.
//!
//! What the program may CALL is the account's services as the TASK ROLE: its credentials are
//! vended for the task before any container of it starts
//! ([`super::task_role_credentials`]), the AWS CLI the session provides signs with them
//! ([`super::container_aws_cli`]), and a container of a task that resolves a task role is not
//! started until they have been vended — a call signed with nothing would be a call made as a
//! role the task was never handed.
//!
//! # Who starts and ends a run
//!
//! Runs are RECONCILED from the task map, never started by whoever happened to call:
//! [`settle_container_runtime`] compares the tasks this store holds against the runs it is
//! holding and (1) starts the run of every container of a task that is now `RUNNING`, (2)
//! raises the termination signal on the runs of every task that is no longer wanted
//! running, and (3) forgets the runs of tasks that are gone. It is called from the same
//! place the task clock is driven from ([`super::drive_task_clock`]), so a task the clock
//! has just started has its containers running without a further call, and a task
//! `StopTask` has just asked to stop has its containers signalled at once.
//!
//! A run records what the run itself produced — the container's exit code and status — and
//! the ONE task transition that follows from it: a task whose ESSENTIAL container's program
//! has ended is stopped, because "if the `essential` parameter of a container is marked as
//! `true`, and that container fails or stops for any reason, all other containers that are
//! part of the task are stopped". That stop is dated by the instant the program ended, not
//! by whoever happened to read afterwards, and a task something else has already stopped
//! keeps the stop that put it there (see
//! [`ComAmazonawsEcsAmazonec2containerservicev20141113State::record_container_outcome`]).
//!
//! # Bounds
//!
//! At most [`MAX_CONTAINER_RUNS`] runs are held at once, and each is given
//! [`CONTAINER_RUN_LIFETIME`] of wall clock; past either, a run is not started or is given
//! up on, and a run that was given up on claims nothing about its container. The rest of
//! the bounds are the session's ([`ContainerShell`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use super::*;

/// How many container runs this store holds at once. THIS NUMBER IS THE EMULATOR'S, not an
/// AWS quota: every run is an in-process task of this emulator, so without a ceiling a
/// caller placing tasks without end would start work without end. Far above what a
/// realistic scenario places.
pub const MAX_CONTAINER_RUNS: usize = 64;

/// How long ONE container's program may run for. THIS NUMBER IS THE EMULATOR'S: a
/// container's program may legitimately run until it is stopped, and this is the wall clock
/// after which this emulator gives up on one instead of holding an in-process task for the
/// life of the process. A run that is given up on records NO exit code, because none was
/// produced.
pub const CONTAINER_RUN_LIFETIME: Duration = Duration::from_secs(3_600);

/// The signals one container's run can be sent.
///
/// Only the termination signal is modelled, because that is the one Amazon ECS sends: a
/// stop "results in a stop signal value ... will default to SIGTERM". The flag is what the
/// session reads, and the notification is what ends a wait the program is inside of, so a
/// container handles the signal as soon as it arrives rather than when its wait runs out.
#[derive(Debug)]
pub struct ContainerSignals {
    /// Whether the termination signal has been raised on this run.
    terminating: AtomicBool,
    /// Wakes the run out of a wait when a signal is raised.
    raised: ::tokio::sync::Notify,
}

impl ContainerSignals {
    /// A run that has been sent nothing yet.
    pub(super) fn new() -> Self {
        Self {
            terminating: AtomicBool::new(false),
            raised: ::tokio::sync::Notify::new(),
        }
    }

    /// Whether the termination signal has been raised on this run.
    pub fn terminating(&self) -> bool {
        self.terminating.load(Ordering::Acquire)
    }

    /// Raise the termination signal, waking the run if it is waiting.
    ///
    /// The permit `notify_one` leaves is what makes the wake-up safe against the race with
    /// a run that is about to wait: a signal raised just before the wait registers still
    /// ends it.
    fn terminate(&self) {
        self.terminating.store(true, Ordering::Release);
        self.raised.notify_one();
    }

    /// A future that completes when a signal is raised on this run.
    pub fn notified(&self) -> ::tokio::sync::futures::Notified<'_> {
        self.raised.notified()
    }
}

/// One container's run, as this store holds it.
#[derive(Debug)]
pub struct ContainerRun {
    /// The signals the run answers, shared with the session performing it.
    signals: Arc<ContainerSignals>,
    /// The task the container belongs to, so a run is forgotten when its task is.
    task_arn: String,
    /// Whether a PROGRAM of the container's own is being performed, as against a run that
    /// only initialises the container's log driver ([`ContainerLaunch::program`]). What
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::performing_containers`]
    /// answers: a container that runs no program holds nothing open, so the end of a
    /// standalone task's work never waits on one.
    runs_a_program: bool,
    /// Whether the program has ENDED. An entry stays after it has, so the container's
    /// program is performed once and never restarted, and only the entries that have not
    /// ended count against [`MAX_CONTAINER_RUNS`].
    ended: bool,
    /// The in-process task performing the run, kept so a run whose task is gone is ABORTED
    /// rather than merely forgotten — a handle dropped on the floor cannot be stopped.
    /// `None` once the run has ended, and until the spawn that starts it hands its handle
    /// over ([`attach_container_run`]).
    handle: Option<::tokio::task::JoinHandle<()>>,
    /// The container's OWN writable layer, held here because it belongs to the CONTAINER and
    /// not to one session of it: the program's session writes to it, and the agent's
    /// health-check probes read it back through the same handle
    /// ([`super::container_filesystem::ContainerLayer`]).
    layer: ContainerLayer,
    /// What this run CONSUMES, shared with the session performing it: how long its program
    /// has spent executing and how many bytes that session is holding
    /// ([`super::container_consumption`]). Held beside the run for the same reason the layer
    /// is — it belongs to the CONTAINER rather than to one session of it — so what a
    /// collection samples is the accounting of what this emulator really performed.
    consumption: Arc<ContainerConsumption>,
}

impl ContainerRun {
    /// What this run has consumed so far ([`super::container_consumption`]).
    pub(super) fn consumption(&self) -> &ContainerConsumption {
        &self.consumption
    }

    /// Whether a PROGRAM of the container's own is still being performed: a run that only
    /// initialises the container's log driver performs none, and one that has ended is
    /// performing nothing any longer.
    pub(super) fn is_performing_a_program(&self) -> bool {
        self.runs_a_program && !self.ended
    }

    /// How many bytes the container's own writable LAYER holds — the files its image and its
    /// program put there, which this process is really holding for it.
    pub(super) fn layer_bytes(&self) -> u64 {
        self.layer.held_bytes()
    }
}

/// One container whose run this pass is STARTING, as the entry the store keeps for it: minted
/// where the launch is assembled, so the handles the run and the session share — its signals,
/// its writable layer and its accounting — are one set rather than two.
#[derive(Debug)]
struct StartedRun {
    /// The container the run is of.
    container_arn: String,
    /// The signals it answers.
    signals: Arc<ContainerSignals>,
    /// Whether a PROGRAM of the container's own is being performed
    /// ([`ContainerRun::runs_a_program`]).
    runs_a_program: bool,
    /// The container's own writable layer.
    layer: ContainerLayer,
    /// Where what the run consumes is accounted for ([`super::container_consumption`]).
    consumption: Arc<ContainerConsumption>,
}

/// What ONE container's run needs, taken out of the store under its lock.
///
/// Everything the run uses is COPIED here, so the run itself holds no guard of this store
/// and takes it only to record the outcome it produced.
#[derive(Debug)]
pub struct ContainerLaunch {
    /// The container this run is of.
    container_arn: String,
    /// The program it runs, resolved from the revision's `entryPoint`/`command` and the
    /// launch's own container overrides, and `None` for a container whose launch names no
    /// argument vector at all — one whose image's own entry point would run, which this
    /// emulator does not hold. Such a launch still initialises the container's log driver
    /// (see this module's header) and then reports nothing of its own.
    program: Option<ContainerProgram>,
    /// The environment it runs with: the revision's `environment` with the launch's
    /// override applied.
    environment: BTreeMap<String, String>,
    /// Where its output is delivered, and `None` for a container whose `logConfiguration`
    /// describes no `awslogs` delivery.
    log_target: Option<AwsLogsTarget>,
    /// The values the agent fetches for it before its program starts — the `secrets` its
    /// definition names — each read as the task's execution role and exposed to the program
    /// as the environment variable its entry spells
    /// ([`super::container_secrets::fetched_environment`]).
    secrets: Vec<ContainerSecret>,
    /// The volumes its `mountPoints` mount into it, read off the revision
    /// ([`super::container_volumes`]) and resolved to the storage each is backed by where the
    /// run is started, because resolving one is a read of another service's store.
    mounts: Vec<MountRequest>,
    /// `ContainerDefinition$workingDirectory`, which the program's relative paths are
    /// resolved against.
    working_directory: Option<String>,
    /// The files its IMAGE ships, written into its own layer before its program runs
    /// ([`super::container_servers::image_shipped_files`]) — empty for an image this emulator
    /// holds none of.
    shipped_files: Vec<(String, String)>,
    /// The private address of the elastic network interface the TASK was allocated, which is
    /// where a server this container's program starts is reached
    /// ([`super::container_servers`]) — `None` for a task that holds no interface of its own,
    /// whose containers are reached at their container instance's address instead, which this
    /// store does not hold.
    address: Option<String>,
    /// The signals it answers.
    signals: Arc<ContainerSignals>,
    /// What the AWS calls this container's program makes PRESENT: the session of the task
    /// role, vended for the task before any container of it starts
    /// ([`super::task_role_credentials`]). It is held beside the run and never put in the
    /// session's variables, its filesystem or anything the container writes.
    credential: TaskRoleCredential,
    /// The Region those calls are addressed in, which is the Region the task was placed in.
    region: String,
    /// What the PROCESS of this container runs as: the user and the resource limits its
    /// definition gives it, and the ephemeral storage its task was given
    /// ([`super::container_process`]).
    process: ContainerProcess,
    /// The container's own writable layer, the one the store holds for it
    /// ([`ContainerRun::layer`]): the program runs over the CONTAINER's filesystem, not over
    /// one of its own.
    layer: ContainerLayer,
    /// Where the session records what this container's run CONSUMES, the one the store holds
    /// for it ([`ContainerRun::consumption`]).
    consumption: Arc<ContainerConsumption>,
    /// Whether the task this container belongs to is one a SERVICE's or a DAEMON's scheduler
    /// maintains, so nothing is waiting for its program to END
    /// ([`ContainerShell::maintained_by_a_service`]).
    maintained_by_a_service: bool,
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Bring the runs this store holds in line with the tasks it holds, and answer the runs
    /// that are to be STARTED.
    ///
    /// Three reconciles in one pass, under one lock: a run whose task is gone or STOPPED is
    /// forgotten — and ABORTED if it was still going, because nothing is left to run it for
    /// — a run whose task is no longer wanted running is signalled to terminate, and a
    /// container of a `RUNNING` task that has no run yet is one to start. Only the starting
    /// half needs anything outside this store, which is why it is answered rather than done
    /// here.
    pub(super) fn settle_container_runs(&mut self, region: &str) -> Vec<ContainerLaunch> {
        // The storage of a volume whose task or container instance is gone is gone with it,
        // settled on the same pass the runs are ([`super::container_volumes`]).
        self.prune_volume_storage();
        let forgotten: Vec<String> = self
            .container_runs
            .iter()
            .filter(|(_, run)| {
                self.tasks
                    .get(&run.task_arn)
                    .is_none_or(|task| task.last_status == STATUS_STOPPED)
            })
            .map(|(arn, _)| arn.clone())
            .collect();
        for arn in forgotten {
            if let Some(run) = self.container_runs.remove(&arn) {
                if let Some(handle) = run.handle {
                    handle.abort();
                }
            }
        }

        // A task that is no longer wanted running has its containers signalled: that is
        // what `StopTask` does to them, and what the scheduler's own stop does.
        for task in self.tasks.values() {
            if task.desired_status == STATUS_RUNNING {
                continue;
            }
            for container in &task.containers {
                if let Some(run) = self.container_runs.get(&container.container_arn) {
                    run.signals.terminate();
                }
            }
        }

        let mut launches = Vec::new();
        let task_arns: Vec<String> = self.tasks.keys().cloned().collect();
        for task_arn in task_arns {
            let Some(task) = self.tasks.get(&task_arn) else {
                continue;
            };
            if task.last_status != STATUS_RUNNING || task.desired_status != STATUS_RUNNING {
                continue;
            }
            let definition = self.revision_of_arn(&task.task_definition_arn).cloned();
            let Some(definition) = definition else {
                continue;
            };
            let Some(task) = self.tasks.get(&task_arn) else {
                continue;
            };
            let launch_region = super::request_arn::RequestArn::parse(&task.task_arn)
                .map(|arn| arn.region.to_string())
                .unwrap_or_else(|| region.to_string());
            // The credential this task's containers present on the AWS calls their programs
            // make. A task that resolves a task role waits for the step that vends one — the
            // agent has the task's credentials before any container of it runs, exactly as it
            // has the values it fetched for them — and one that resolves none holds nothing
            // ([`super::task_role_credentials`]).
            let credential = match resolved_task_role_arn(task.overrides.as_ref(), &definition) {
                Some(_) => match &task.task_role_credentials {
                    Some(vended) => vended.clone(),
                    None => continue,
                },
                None => TaskRoleCredential::Absent,
            };
            let mut started: Vec<StartedRun> = Vec::new();
            for container in &task.containers {
                // A container that has not STARTED yet runs nothing: one whose
                // `dependsOn` conditions are not met is still `PENDING`, and its program
                // begins where its start does ([`super::container_health`]).
                if container.last_status != STATUS_RUNNING {
                    continue;
                }
                if self.container_runs.contains_key(&container.container_arn) {
                    continue;
                }
                if self.active_container_runs() + started.len() >= MAX_CONTAINER_RUNS {
                    // The ceiling is this emulator's, so it is TRACED rather than passed
                    // over in silence: a container that is not run delivers nothing and
                    // reports no exit code, and the reader of that task cannot otherwise
                    // tell that from a container that produced nothing.
                    ::tracing::warn!(
                        task_arn = %task_arn,
                        container_arn = %container.container_arn,
                        limit = MAX_CONTAINER_RUNS,
                        "this emulator is already running as many containers as it holds \
                         at once, so this container's program is not run"
                    );
                    break;
                }
                let Some(name) = container.name.as_deref() else {
                    continue;
                };
                let Some(stored) = definition
                    .container_definitions
                    .iter()
                    .find(|candidate| candidate.name.as_deref() == Some(name))
                else {
                    continue;
                };
                let override_for = task
                    .overrides
                    .as_ref()
                    .and_then(|overrides| overrides.container_overrides.as_ref())
                    .and_then(|overrides| {
                        overrides
                            .iter()
                            .find(|candidate| candidate.name.as_deref() == Some(name))
                    });
                // `ContainerOverride$command`: "The command to send to the container that
                // overrides the default command from the Docker image or the task
                // definition" — so an override replaces the vector rather than adding to
                // it, and a launch that overrides none runs the revision's own.
                let command = override_for
                    .and_then(|over| over.command.as_deref())
                    .or(stored.command.as_deref());
                // A container whose revision overrides NEITHER the entry point nor the command
                // runs the command its IMAGE declares — the "default command from the Docker
                // image" that same member's own documentation names
                // ([`super::container_servers::image_declared_command`]). An image this
                // emulator holds none of leaves such a container no program at all, exactly as
                // it did before. WHICH image that is comes off the container
                // ([`ContainerRecord::running_image`]): a reference into a repository that
                // caches an upstream image names that upstream image, so what the image
                // declares is the upstream one's.
                let running_image = container.running_image().map(str::to_string);
                let declared = (stored.entry_point.is_none() && command.is_none())
                    .then(|| {
                        running_image
                            .as_deref()
                            .and_then(super::container_servers::image_declared_command)
                    })
                    .flatten();
                // The other half of that default: a container whose revision overrides no
                // ENTRY POINT is exec'd as the entry point its IMAGE declares, with the
                // command above as that program's arguments
                // ([`super::container_image_entry_points`]) — which is how a container of an
                // image whose own program takes arguments (the AWS CLI's) declares a command
                // naming no program at all. An image whose entry point this emulator knows
                // none of adds nothing, so such a container's program is its own vector.
                let declared_entry_point = stored
                    .entry_point
                    .is_none()
                    .then(|| {
                        running_image.as_deref().and_then(
                            super::container_image_entry_points::image_declared_entry_point,
                        )
                    })
                    .flatten();
                let program = ContainerProgram::resolve(
                    stored
                        .entry_point
                        .as_deref()
                        .or(declared_entry_point.as_deref()),
                    command.or(declared.as_deref()),
                );
                // The address the task's own interface holds, which is both where a server
                // this container starts is reached and what the container's HOSTNAME is read
                // from ([`container_hostname`]).
                let address = task
                    .attachments
                    .iter()
                    .find_map(AttachmentRecord::private_ipv4_address)
                    .map(str::to_string);
                // The container's own hostname, which the container runtime puts in its
                // environment under [`HOSTNAME_VARIABLE`] — before the definition's own
                // `environment`, so a revision that sets that name itself is what its process
                // sees.
                let mut environment: BTreeMap<String, String> = BTreeMap::new();
                if let Some(hostname) =
                    container_hostname(stored, address.as_deref(), &launch_region)
                {
                    environment.insert(HOSTNAME_VARIABLE.to_string(), hostname);
                }
                // And the address of the TASK METADATA ENDPOINT this container reads what it
                // is at, which Amazon ECS injects into every container it launches
                // ([`super::task_metadata_endpoint`]).
                environment.insert(
                    TASK_METADATA_URI_VARIABLE.to_string(),
                    container_metadata_uri(container),
                );
                environment.extend(environment_of(stored.environment.as_deref()));
                // `ContainerOverride$environment`: "The environment variables to send to
                // the container ... This parameter maps to Env in the docker container
                // create command" — an override of a name the revision also sets is the
                // value the container's process sees.
                environment.extend(environment_of(
                    override_for.and_then(|over| over.environment.as_deref()),
                ));
                let container_id = container
                    .container_arn
                    .rsplit('/')
                    .next()
                    .unwrap_or(container.container_arn.as_str())
                    .to_string();
                let log_target = AwsLogsTarget::resolve(
                    stored.log_configuration.as_ref(),
                    name,
                    &task.task_id,
                    &container_id,
                    &launch_region,
                );
                // Nothing to perform for this container at all: no program of its own and
                // no log driver to initialise, so no run is held for it.
                if program.is_none() && log_target.is_none() {
                    continue;
                }
                let signals = Arc::new(ContainerSignals::new());
                // The layer this container's filesystem is: minted once for the container and
                // handed to every session that runs in it — the program's below, and each
                // health-check probe's off the stored run.
                let layer = ContainerLayer::new();
                // Where the session performing this container's program accounts for what it
                // consumes, minted once for the container and shared with the run the store
                // holds ([`super::container_consumption`]).
                let consumption = Arc::new(ContainerConsumption::new());
                started.push(StartedRun {
                    container_arn: container.container_arn.clone(),
                    signals: Arc::clone(&signals),
                    runs_a_program: program.is_some(),
                    layer: layer.clone(),
                    consumption: Arc::clone(&consumption),
                });
                launches.push(ContainerLaunch {
                    container_arn: container.container_arn.clone(),
                    program,
                    environment,
                    log_target,
                    // The values the agent fetches for this container, read where the run
                    // is performed: reading one is a call to the service that holds it,
                    // which this pass cannot make under the store's lock.
                    secrets: container_environment_secrets(&definition, name),
                    // The mount points this container declares, against the volumes the
                    // revision declares: both are members of this store's own records, and
                    // the storage each names is resolved where the run is started.
                    mounts: mount_requests(&definition, stored, task),
                    working_directory: stored.working_directory.clone(),
                    // What this container's IMAGE provides, written into its layer before its
                    // program runs: a container's own layer sits over the image's filesystem,
                    // and for an image of this emulator's own catalog part of that filesystem
                    // is documented ([`super::container_servers::image_shipped_files`]).
                    shipped_files: running_image
                        .as_deref()
                        .map(super::container_servers::image_shipped_files)
                        .unwrap_or_default(),
                    // Where a server this container's program starts is reached: the address
                    // of the interface the task holds, read off the task itself.
                    address,
                    signals,
                    credential: credential.clone(),
                    region: launch_region.clone(),
                    // What this container's process runs as, read off the same stored
                    // definition its program is, and off the task's own placement.
                    process: ContainerProcess::of(Some(stored), task),
                    layer,
                    consumption,
                    // The scheduler that placed this task, read off the task itself: a task a
                    // service's or a daemon's scheduler maintains is one that scheduler keeps
                    // RUNNING, so nothing is waiting for its containers' programs to end.
                    maintained_by_a_service: task.service_name.is_some()
                        || task.daemon_arn.is_some(),
                });
            }
            for started in started {
                self.container_runs.insert(
                    started.container_arn,
                    ContainerRun {
                        signals: started.signals,
                        task_arn: task_arn.clone(),
                        runs_a_program: started.runs_a_program,
                        ended: false,
                        handle: None,
                        layer: started.layer,
                        consumption: started.consumption,
                    },
                );
            }
        }
        launches
    }

    /// The writable layer of the container `container_arn` names, and `None` for a container
    /// this store holds no run of.
    ///
    /// What a session that runs INSIDE that container is given, so it reads and writes the
    /// CONTAINER's filesystem: a health-check probe of a container whose program wrote a file
    /// finds that file ([`super::container_health`]). A container with no run has had nothing
    /// run in it, so there is no layer to share and none is invented.
    pub(super) fn container_layer(&self, container_arn: &str) -> Option<ContainerLayer> {
        self.container_runs
            .get(container_arn)
            .map(|run| run.layer.clone())
    }

    /// How many of the runs this store holds are still GOING, which is what
    /// [`MAX_CONTAINER_RUNS`] bounds: an entry whose program has ended holds nothing but the
    /// record that it ran.
    fn active_container_runs(&self) -> usize {
        self.container_runs
            .values()
            .filter(|run| !run.ended)
            .count()
    }

    /// The containers whose PROGRAMS are still being performed — the set a task's own end
    /// waits on ([`TaskRecord::essential_programs_running`]).
    ///
    /// A run that only initialises the container's log driver performs no program of the
    /// container's ([`ContainerRun::runs_a_program`]), so it holds nothing open: a
    /// standalone task whose containers run nothing ends when its work is due, exactly as it
    /// did before such a container had a run at all.
    pub(super) fn performing_containers(&self) -> HashSet<String> {
        self.container_runs
            .iter()
            .filter(|(_, run)| !run.ended && run.runs_a_program)
            .map(|(container_arn, _)| container_arn.clone())
            .collect()
    }

    /// Keep the in-process task performing one container's run, so the run can be ABORTED
    /// when the task it belongs to is gone. A run that has already ended by the time its
    /// handle arrives needs none.
    pub(super) fn attach_container_run(
        &mut self,
        container_arn: &str,
        handle: ::tokio::task::JoinHandle<()>,
    ) {
        match self.container_runs.get_mut(container_arn) {
            Some(run) if !run.ended => run.handle = Some(handle),
            _ => handle.abort(),
        }
    }

    /// Record what one container's program ENDED as: the status it exited with becomes the
    /// container's `exitCode` and the container reports `STOPPED`, because the program the
    /// container was running is over — and a task whose ESSENTIAL container that was is
    /// stopped with it.
    ///
    /// A run this emulator gave up on records no STATUS ([`ContainerOutcome::Abandoned`]): no
    /// exit code was produced, so none is reported — and what IS known about such a run is
    /// the additional detail `detail` carries, recorded on the container
    /// ([`Self::record_container_without_outcome`]).
    ///
    /// The stop is what a container definition's `essential` member means: "If the
    /// `essential` parameter of a container is marked as `true`, and that container fails or
    /// stops for any reason, all other containers that are part of the task are stopped"
    /// (`ContainerDefinition$essential`), which is the task's own
    /// [`STOP_CODE_ESSENTIAL_CONTAINER_EXITED`] ("The task stopped because an essential
    /// container exited"). Only a task something else has not already stopped takes it
    /// ([`TaskRecord::work_is_running`]), and only one whose end the clock does not already
    /// owe at an instant of its own: a STANDALONE task was placed to do a piece of work and
    /// is reported running for the span that work takes ([`CONTAINER_WORK_SPAN`], which the
    /// clock materialises once an essential container HAS exited), while a task a service's
    /// or a daemon's scheduler placed is kept in place by that scheduler and so ends here,
    /// at the instant its essential container exited — as does a standalone task whose span
    /// has already passed.
    pub(super) fn record_container_outcome(
        &mut self,
        container_arn: &str,
        outcome: ContainerOutcome,
        detail: Option<String>,
    ) {
        let ContainerOutcome::Exited(status) = outcome else {
            self.record_container_without_outcome(container_arn, detail);
            return;
        };
        self.end_container_run(container_arn);
        // The instant the program ended is the instant this records: the run has just
        // finished, which is when the exit happened.
        let at = SystemTime::now();
        for task in self.tasks.values_mut() {
            let Some(container) = task
                .containers
                .iter_mut()
                .find(|container| container.container_arn == container_arn)
            else {
                continue;
            };
            container.exit_code = Some(status);
            container.advance_to(STATUS_STOPPED);
            let essential = container.essential;
            // "Every time a task experiences a change ... the version counter is
            // incremented."
            task.version += 1;
            if essential && task.work_is_running() && task.work_done_due_at.is_none() {
                task.finish_work(at);
            }
            return;
        }
    }

    /// Record that one container's run produced NO outcome: no status is claimed for it, and
    /// the additional detail about the container is what `detail` says.
    ///
    /// `Container$reason` is "a short (1024 max characters) human-readable string to provide
    /// additional details about a running or stopped container", and the additional detail
    /// about a container whose program produced nothing is that nothing performed it
    /// ([`ContainerShell::nothing_performed_detail`]) — which is the only account of the run
    /// there will be, since no exit code was produced and the container's own exit is what
    /// stops the task it belongs to. A detail something else has already recorded for the
    /// container — the agent's own report, the reason a launch could not be made
    /// ([`TaskRecord::fail_launch`]) — is left as it was given: this adds what is known about
    /// a run, and overwrites nothing.
    ///
    /// The task keeps the status it has: an exit is what stops a task whose essential
    /// container exited, and no container of this one produced one.
    fn record_container_without_outcome(&mut self, container_arn: &str, detail: Option<String>) {
        // The run is over either way, so its entry stops counting against
        // [`MAX_CONTAINER_RUNS`] and the container's program is not restarted.
        self.end_container_run(container_arn);
        let Some(detail) = detail else {
            return;
        };
        for task in self.tasks.values_mut() {
            let Some(container) = task
                .containers
                .iter_mut()
                .find(|container| container.container_arn == container_arn)
            else {
                continue;
            };
            if container.reason.is_none() {
                container.reason = Some(detail);
                // "Every time a task experiences a change ... the version counter is
                // incremented."
                task.version += 1;
            }
            return;
        }
    }

    /// Mark one container's run ENDED: the entry stays, so the container's program is
    /// performed once and never restarted, and the in-process task that performed it is no
    /// longer one to abort.
    fn end_container_run(&mut self, container_arn: &str) {
        if let Some(run) = self.container_runs.get_mut(container_arn) {
            run.ended = true;
            run.handle = None;
        }
    }
}

/// The variable the container runtime puts a container's own HOSTNAME in, which is what a
/// program of it reads its hostname from and what the `hostname` utility answers
/// (`crate::state::container_shell`).
pub const HOSTNAME_VARIABLE: &str = "HOSTNAME";

/// The hostname ONE container carries.
///
/// `ContainerDefinition$hostname` is "the hostname to use for your container", so a definition
/// that names one is what the container is called — except that the same member states "this
/// parameter is not supported for ... tasks that use the `awsvpc` network mode", and such a
/// task is reached at its OWN elastic network interface: the hostname is then the private DNS
/// hostname Amazon EC2 gives that interface's address
/// (`::service_com_amazonaws_ec2_amazonec2::state::private_dns_name`, the one reading of that
/// name in this workspace), as the unqualified name a host answers with.
///
/// `None` for a container that is neither — one of a task this store holds no address for and
/// whose definition names no hostname — which carries no hostname rather than an invented one.
fn container_hostname(
    stored: &ContainerDefinition,
    address: Option<&str>,
    region: &str,
) -> Option<String> {
    if let Some(address) = address {
        let qualified =
            ::service_com_amazonaws_ec2_amazonec2::state::private_dns_name(address, region);
        return qualified.split('.').next().map(str::to_string);
    }
    stored
        .hostname
        .as_deref()
        .map(str::trim)
        .filter(|hostname| !hostname.is_empty())
        .map(str::to_string)
}

/// The environment a `KeyValuePair` list describes, as the variables a session holds.
///
/// `KeyValuePair$name` is "the name of the key-value pair" and `value` "the value of the
/// key-value pair"; a pair carrying no name names no variable, so it sets none.
fn environment_of(pairs: Option<&[KeyValuePair]>) -> BTreeMap<String, String> {
    pairs
        .unwrap_or_default()
        .iter()
        .filter_map(|pair| {
            let name = pair.name.clone()?;
            Some((name, pair.value.clone().unwrap_or_default()))
        })
        .collect()
}

/// Start the container runs this store owes, and signal the ones whose task is stopping.
///
/// Call it with NO guard of this store held, from a door that has just written a task's
/// status or from the clock pump: it takes the lock itself, and the runs it starts are
/// in-process tasks that outlive the call. With no async runtime to start them on there is
/// no container run at all — nothing is reported as running that is not.
pub fn settle_container_runtime<Eng: Send + Sync + 'static>(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let launches = state.lock().settle_container_runs(region);
    // The records a server a container's program starts is published in, taken once for the
    // whole pass: they live in the SAME shared engine as this store
    // ([`::service_common::served_http_addresses`]), and an engine that vends none leaves a
    // container's server reachable by nothing.
    let addresses = ::service_common::served_http_addresses::ServedAddresses::of(&**engine);
    // And the account's own network, which decides whether a request one container's program
    // makes to ANOTHER workload's address arrives ([`super::task_reachability`]). Taken once
    // for the whole pass, like the records above.
    let network = TaskNetwork::of(&**engine);
    for launch in launches {
        let container_arn = launch.container_arn.clone();
        let run_state = ::engine::ServiceState::clone(&state);
        let callback = callback.clone();
        // The shared engine the run ANNOUNCES its end through: the task a container's exit
        // stops is a task that has moved, and the run is the only component that knows the
        // instant it moved at ([`super::publish_data_plane_task_state_changes`]).
        let run_engine = ::std::sync::Arc::clone(engine);
        // WHERE a server this container starts is reached, for a task that holds an interface
        // of its own on an engine that vends the records.
        let serving = launch
            .address
            .clone()
            .zip(addresses.clone())
            .map(|(address, addresses)| {
                ContainerServing::new(address, addresses)
                    .accounting_into(Arc::clone(&launch.consumption))
            });
        // And the client its program reaches those same servers THROUGH, for a task reached at
        // an address of its own: a container's program routinely asks its own server for a
        // document ([`super::container_http_client`]) — and another workload's, which the
        // account's network decides the arrival of ([`super::task_reachability`]). A task this
        // emulator holds no address for still has the one endpoint inside its own namespace:
        // the task metadata endpoint its containers read what they are at
        // ([`super::task_metadata_endpoint`]).
        let http = addresses.clone().map(|addresses| {
            let client = match launch.address.clone() {
                Some(address) => ContainerHttpClient::new(address, addresses),
                None => ContainerHttpClient::unaddressed(addresses),
            };
            client
                .on_network(network.clone())
                .accounting_into(Arc::clone(&launch.consumption))
                .with_task_metadata(TaskMetadataEndpoint::new(
                    ::engine::ServiceState::clone(&state),
                    launch.container_arn.clone(),
                ))
        });
        // The volumes the container's `mountPoints` mount, resolved with NO guard of this
        // store held: the files of an `efsVolumeConfiguration` volume are the Amazon EFS
        // service's own, so resolving one reads that service's store
        // ([`super::container_volumes`]).
        let filesystem = ContainerFilesystem::on_layer(
            launch.layer.clone(),
            launch.working_directory.as_deref(),
            mounted_volumes(&**engine, &state, &launch.mounts),
        );
        // What the container's IMAGE provides, in place before its program runs — which is
        // what makes a container of a web server image serve its root without a program of
        // its own having written one, and what lets a program read or copy that page. A write
        // the layer refuses changes nothing: the file is simply not there, exactly as it is
        // for an image this emulator holds none of.
        let filesystem = launch.shipped_files.iter().fold(
            filesystem,
            |mut filesystem, (path, contents)| {
                use ::service_common::posix_shell_files::ShellFiles as _;
                let _ = filesystem.write(path, contents, false);
                filesystem
            },
        );
        // Where the bytes this filesystem reads and writes for the container are counted
        // ([`super::container_consumption`]) — attached AFTER the image's own files are in
        // place, because what the image ships is not something the container's program read
        // or wrote.
        let filesystem = filesystem.accounting_into(Arc::clone(&launch.consumption));
        let handle = runtime.spawn(async move {
            run_container(
                run_state,
                run_engine,
                callback,
                launch,
                filesystem,
                serving,
                http,
            )
            .await
        });
        // The handle is kept with the run, so a run whose task is gone is aborted rather
        // than merely forgotten.
        state.lock().attach_container_run(&container_arn, handle);
    }
    // A container that is running is a container the agent probes: the health checks whose
    // instant has come are run beside the programs, off the same pass
    // ([`super::settle_container_health`]).
    settle_container_health(callback, engine);
}

/// Start ONE container: initialise its log driver, then run its program to its end,
/// delivering what it writes, recording what it exited with and ANNOUNCING the task
/// transition that exit produced.
///
/// A container this emulator holds no program for still has its driver initialised — the
/// stream is created as the container starts, which is what makes the group the
/// configuration names hold one stream per started container (see this module's header) —
/// and then reports nothing of its own ([`ContainerOutcome::Abandoned`]): the image holds
/// the program whose output would have filled that stream, so no event is put in it.
///
/// The ANNOUNCEMENT is what makes the end of a run observable without another Amazon ECS
/// call: a standalone task whose essential container has exited is stopped at the instant
/// its program ended, and the `STOPPED` event that stop earns is owed to a client watching
/// the task through EventBridge, which will call nothing again
/// ([`super::publish_data_plane_task_state_changes`]). Made after the store's guard has been
/// dropped, as every other writer's is.
async fn run_container<Eng: Send + Sync + 'static>(
    state: ::engine::ServiceState<ComAmazonawsEcsAmazonec2containerservicev20141113State>,
    engine: ::std::sync::Arc<Eng>,
    callback: ::smithy_server_runtime::EmbeddedEmulator,
    launch: ContainerLaunch,
    filesystem: ContainerFilesystem,
    serving: Option<ContainerServing>,
    http: Option<ContainerHttpClient>,
) {
    // The values the agent fetches for the container, before anything of it runs: a secret
    // it could not retrieve starts NO program, so the container reports no exit code —
    // nothing ran to produce one ([`super::container_secrets::fetched_environment`]).
    let Some(environment) =
        fetched_environment(&callback, &launch.environment, &launch.secrets).await
    else {
        // What kept the program from starting is the retrieval itself, which the step that
        // made it words on the task and its container ([`super::container_secrets`]), so
        // nothing is added here.
        state.lock().record_container_outcome(
            &launch.container_arn,
            ContainerOutcome::Abandoned,
            None,
        );
        super::publish_data_plane_task_state_changes(&callback, &engine, &launch.region);
        return;
    };
    let output = Arc::new(ContainerOutput::new(&callback, launch.log_target));
    // The stream the driver delivers to exists before the container writes its first line,
    // which is the order the driver itself does it in.
    output.open().await;
    let Some(program) = launch.program.as_ref() else {
        // The driver is initialised and there is nothing of this container's own to run, which
        // is what the container reports as the detail of its run
        // ([`NO_DECLARED_PROGRAM_DETAIL`]).
        state.lock().record_container_outcome(
            &launch.container_arn,
            ContainerOutcome::Abandoned,
            Some(NO_DECLARED_PROGRAM_DETAIL.to_string()),
        );
        super::publish_data_plane_task_state_changes(&callback, &engine, &launch.region);
        return;
    };
    let deadline = Instant::now() + CONTAINER_RUN_LIFETIME;
    let mut shell = ContainerShell::new(
        environment,
        Arc::clone(&output),
        Arc::clone(&launch.signals),
        deadline,
        filesystem,
        serving,
    )
    // The AWS CLI this container's program reaches the account's services through, signing
    // as the task role ([`super::container_aws_cli`]).
    .with_aws_cli(
        ContainerAwsCli::new(
            callback.clone(),
            &launch.region,
            launch.credential.clone(),
        )
        .accounting_into(Arc::clone(&launch.consumption)),
    )
    // What the process this session IS runs as, which is what the utilities that report a
    // process's own session answer ([`super::container_process`]).
    .with_process(launch.process.clone())
    // Where this session accounts for what the container's run consumes
    // ([`super::container_consumption`]).
    .with_consumption(Arc::clone(&launch.consumption));
    // The HTTP client its program asks its own task's servers for a document with
    // ([`super::container_http_client`]), for a task this emulator holds an address for.
    if let Some(http) = http {
        shell = shell.with_http_client(http);
    }
    // Whether anything is waiting for this container's program to END, which is what decides
    // whether this emulator puts a ceiling on the waits that program names
    // ([`ContainerShell::maintained_by_a_service`]).
    if launch.maintained_by_a_service {
        shell = shell.maintained_by_a_service();
    }
    let outcome = shell.run(program).await;
    // What is known about a run that produced no status of its own, which is what such a
    // container reports as the additional detail about it
    // ([`ContainerShell::nothing_performed_detail`]).
    let detail = shell.nothing_performed_detail();
    // The session is OVER, so it holds nothing more: what the container still occupies is its
    // own layer, which outlives the session that wrote it.
    launch.consumption.release();
    state
        .lock()
        .record_container_outcome(&launch.container_arn, outcome, detail);
    // The guard that statement took is dropped with it, so the announcement — and the pump it
    // hands the publication to — holds no lock of this store. The Region is the task's own, the
    // one this run's own calls are addressed in ([`ContainerLaunch::region`]).
    super::publish_data_plane_task_state_changes(&callback, &engine, &launch.region);
}
