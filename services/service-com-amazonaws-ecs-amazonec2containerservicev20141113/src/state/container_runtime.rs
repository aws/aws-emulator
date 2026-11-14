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
//! exit code is invented for it.
//!
//! What the program SEES is its definition's environment with the launch's own overrides
//! applied, and then the values the agent fetched for it — a container definition's
//! `secrets` ([`super::container_secrets`]), read before the program starts, as the task's
//! execution role. A container whose secrets cannot be read starts no program at all, so it
//! reports no exit code: nothing ran to produce one.
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
//! Nothing here MOVES a task: a container's run only ever records what the run itself
//! produced — the container's exit code and status — so a reporting operation that drives
//! this reconcile reports no status the clock had not already made due.
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
    /// Whether the program has ENDED. An entry stays after it has, so the container's
    /// program is performed once and never restarted, and only the entries that have not
    /// ended count against [`MAX_CONTAINER_RUNS`].
    ended: bool,
    /// The in-process task performing the run, kept so a run whose task is gone is ABORTED
    /// rather than merely forgotten — a handle dropped on the floor cannot be stopped.
    /// `None` once the run has ended, and until the spawn that starts it hands its handle
    /// over ([`attach_container_run`]).
    handle: Option<::tokio::task::JoinHandle<()>>,
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
    /// launch's own container overrides.
    program: ContainerProgram,
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
    /// The signals it answers.
    signals: Arc<ContainerSignals>,
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
            let definition = parse_task_definition_reference(&task.task_definition_arn)
                .and_then(|(family, revision)| {
                    revision.and_then(|revision| self.task_definitions.get(&(family, revision)))
                })
                .cloned();
            let Some(definition) = definition else {
                continue;
            };
            let Some(task) = self.tasks.get(&task_arn) else {
                continue;
            };
            let launch_region = super::request_arn::RequestArn::parse(&task.task_arn)
                .map(|arn| arn.region.to_string())
                .unwrap_or_else(|| region.to_string());
            let mut started: Vec<(String, Arc<ContainerSignals>)> = Vec::new();
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
                let Some(program) =
                    ContainerProgram::resolve(stored.entry_point.as_deref(), command)
                else {
                    continue;
                };
                let mut environment = environment_of(stored.environment.as_deref());
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
                let signals = Arc::new(ContainerSignals::new());
                started.push((container.container_arn.clone(), Arc::clone(&signals)));
                launches.push(ContainerLaunch {
                    container_arn: container.container_arn.clone(),
                    program,
                    environment,
                    log_target,
                    // The values the agent fetches for this container, read where the run
                    // is performed: reading one is a call to the service that holds it,
                    // which this pass cannot make under the store's lock.
                    secrets: container_environment_secrets(&definition, name),
                    signals,
                });
            }
            for (container_arn, signals) in started {
                self.container_runs.insert(
                    container_arn,
                    ContainerRun {
                        signals,
                        task_arn: task_arn.clone(),
                        ended: false,
                        handle: None,
                    },
                );
            }
        }
        launches
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

    /// The containers whose programs are still being PERFORMED — the set a task's own end
    /// waits on ([`TaskRecord::essential_programs_running`]).
    pub(super) fn performing_containers(&self) -> HashSet<String> {
        self.container_runs
            .iter()
            .filter(|(_, run)| !run.ended)
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
    /// container was running is over.
    ///
    /// A run this emulator gave up on records nothing ([`ContainerOutcome::Abandoned`]): no
    /// exit code was produced, so none is reported. The TASK is not moved here: what a
    /// stopped container does to the task around it — an essential container's exit
    /// stopping the whole task — is the scheduler's, and this store does not model it yet,
    /// so a task whose container has exited keeps the status the clock gave it and reports
    /// the container exactly as the run left it.
    pub(super) fn record_container_outcome(
        &mut self,
        container_arn: &str,
        outcome: ContainerOutcome,
    ) {
        let ContainerOutcome::Exited(status) = outcome else {
            // The run is over either way, so its entry stops counting against
            // [`MAX_CONTAINER_RUNS`] and the container's program is not restarted.
            self.end_container_run(container_arn);
            return;
        };
        self.end_container_run(container_arn);
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
            // "Every time a task experiences a change ... the version counter is
            // incremented."
            task.version += 1;
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
    for launch in launches {
        let container_arn = launch.container_arn.clone();
        let run_state = ::engine::ServiceState::clone(&state);
        let callback = callback.clone();
        let handle = runtime.spawn(async move { run_container(run_state, callback, launch).await });
        // The handle is kept with the run, so a run whose task is gone is aborted rather
        // than merely forgotten.
        state.lock().attach_container_run(&container_arn, handle);
    }
    // A container that is running is a container the agent probes: the health checks whose
    // instant has come are run beside the programs, off the same pass
    // ([`super::settle_container_health`]).
    settle_container_health(callback, engine);
}

/// Run ONE container's program to its end, delivering what it writes and recording what it
/// exited with.
async fn run_container(
    state: ::engine::ServiceState<ComAmazonawsEcsAmazonec2containerservicev20141113State>,
    callback: ::smithy_server_runtime::EmbeddedEmulator,
    launch: ContainerLaunch,
) {
    // The values the agent fetches for the container, before anything of it runs: a secret
    // it could not retrieve starts NO program, so the container reports no exit code —
    // nothing ran to produce one ([`super::container_secrets::fetched_environment`]).
    let Some(environment) =
        fetched_environment(&callback, &launch.environment, &launch.secrets).await
    else {
        state
            .lock()
            .record_container_outcome(&launch.container_arn, ContainerOutcome::Abandoned);
        return;
    };
    let output = Arc::new(ContainerOutput::new(&callback, launch.log_target));
    // The stream the driver delivers to exists before the container writes its first line,
    // which is the order the driver itself does it in.
    output.open().await;
    let deadline = Instant::now() + CONTAINER_RUN_LIFETIME;
    let mut shell = ContainerShell::new(
        environment,
        Arc::clone(&output),
        Arc::clone(&launch.signals),
        deadline,
    );
    let outcome = shell.run(&launch.program).await;
    state
        .lock()
        .record_container_outcome(&launch.container_arn, outcome);
}
